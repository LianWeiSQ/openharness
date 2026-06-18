from __future__ import annotations

import asyncio
import difflib
import hashlib
import json
import os
import threading
import time
from collections import deque
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.core.agent.universal import UniversalAgent
from openagent.core.agent.plan import PlanAgent
from openagent.core.agent.explore import ExploreAgent
from openagent.core.id import new_id
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction
from openagent.core.provider.base import LanguageModel
from openagent.core.provider.factory import create_provider
from openagent.core.session.session import Session
from openagent.core.session.store import DEFAULT_SESSION_STORE_ROOT, SESSION_STORE_METADATA_KEY, FileSessionStore
from openagent.core.types import AgentConfig, Model, StreamEvent

from .protocol import AppEvent, TuiControlRequest, lifecycle_event, stream_event_to_app_event

LanguageModelFactory = Callable[[Model], LanguageModel | Awaitable[LanguageModel]]
MAX_TUI_CONTROL_QUEUE = 100


@dataclass(frozen=True, slots=True)
class ApprovalRequest:
    request_id: str
    session_id: str
    turn_id: str
    tool_name: str
    tool_input: dict[str, Any]
    call_id: str | None = None
    preview: dict[str, Any] | None = None
    created_at_ms: int = field(default_factory=lambda: int(time.time() * 1000))

    def to_dict(self) -> dict[str, Any]:
        payload = {
            "request_id": self.request_id,
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "tool_input": self.tool_input,
            "call_id": self.call_id,
            "created_at_ms": self.created_at_ms,
        }
        if self.preview is not None:
            payload["preview"] = self.preview
        return payload


@dataclass(slots=True)
class _PendingApproval:
    request: ApprovalRequest
    loop: asyncio.AbstractEventLoop
    future: asyncio.Future[PermissionAction]


@dataclass(slots=True)
class TurnRecord:
    id: str
    session_id: str
    input: str
    created_at_ms: int = field(default_factory=lambda: int(time.time() * 1000))
    status: str = "queued"
    final_answer: str = ""
    error: str | None = None
    trace: dict[str, Any] | None = None
    model_id: str | None = None
    provider_id: str | None = None
    agent_name: str | None = None
    variant: str | None = None
    interrupt_requested: bool = False
    events: list[AppEvent] = field(default_factory=list)
    patch_records: list[dict[str, Any]] = field(default_factory=list)
    workspace: Path | None = field(default=None, repr=False)
    approval_rules: list[dict[str, Any]] = field(default_factory=list)
    _event_publisher: Callable[[AppEvent], None] | None = field(default=None, repr=False)
    _pending_approvals: dict[str, _PendingApproval] = field(default_factory=dict, repr=False)
    _condition: threading.Condition = field(default_factory=threading.Condition, repr=False)

    def append(self, event: AppEvent) -> None:
        with self._condition:
            self._append_event_locked(event)
            self._condition.notify_all()

    def _append_event_locked(self, event: AppEvent) -> None:
        self.events.append(event)
        if self._event_publisher is not None:
            self._event_publisher(event)

    def next_sequence(self) -> int:
        with self._condition:
            return len(self.events) + 1

    def wait_for_sequence(self, sequence: int, *, timeout_s: float = 15.0) -> AppEvent | None:
        deadline = time.time() + timeout_s
        with self._condition:
            while len(self.events) < sequence and self.status not in {"completed", "failed", "interrupted"}:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self._condition.wait(timeout=remaining)
            if len(self.events) >= sequence:
                return self.events[sequence - 1]
            return None

    def wait_until_terminal(self, *, timeout_s: float = 30.0) -> bool:
        deadline = time.time() + timeout_s
        with self._condition:
            while self.status not in {"completed", "failed", "interrupted"}:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return False
                self._condition.wait(timeout=remaining)
            return True

    def request_interrupt(self) -> AppEvent | None:
        approvals_to_resolve: list[_PendingApproval] = []
        with self._condition:
            if self.status in {"completed", "failed", "interrupted"}:
                return None
            if self.interrupt_requested:
                return None
            self.interrupt_requested = True
            self.status = "interrupting"
            event = lifecycle_event(
                sequence=len(self.events) + 1,
                method="turn/interrupt_requested",
                thread_id=self.session_id,
                turn_id=self.id,
                status=self.status,
            )
            self._append_event_locked(event)
            approvals_to_resolve = list(self._pending_approvals.values())
            self._pending_approvals.clear()
            for pending in approvals_to_resolve:
                self._append_event_locked(
                    self._approval_resolved_event(
                        pending=pending,
                        action=PermissionAction.DENY,
                        status=self.status,
                        reason="interrupt",
                    )
                )
            self._condition.notify_all()
        for pending in approvals_to_resolve:
            self._set_approval_result(pending, PermissionAction.DENY)
        return event

    def is_interrupt_requested(self) -> bool:
        with self._condition:
            return self.interrupt_requested

    async def request_approval(self, tool_call: dict[str, Any]) -> PermissionAction:
        tool_name = str(tool_call.get("name") or tool_call.get("tool") or "tool")
        tool_input = dict(tool_call.get("input") or {})
        if _approval_rule_matches(self.approval_rules, tool_name=tool_name, tool_input=tool_input):
            return PermissionAction.ALLOW
        request = ApprovalRequest(
            request_id=new_id("approval"),
            session_id=self.session_id,
            turn_id=self.id,
            tool_name=tool_name,
            tool_input=tool_input,
            call_id=str(tool_call.get("call_id") or "") or None,
            preview=_approval_preview(self.workspace, tool_name, tool_input),
        )
        loop = asyncio.get_running_loop()
        future: asyncio.Future[PermissionAction] = loop.create_future()
        with self._condition:
            if self.interrupt_requested or self.status in {"completed", "failed", "interrupted"}:
                return PermissionAction.DENY
            self._pending_approvals[request.request_id] = _PendingApproval(request=request, loop=loop, future=future)
            self.status = "waiting_approval"
            self._append_event_locked(
                lifecycle_event(
                    sequence=len(self.events) + 1,
                    method="turn/approval_requested",
                    thread_id=self.session_id,
                    turn_id=self.id,
                    status=self.status,
                    approval=request.to_dict(),
                )
            )
            self._condition.notify_all()
        try:
            return await future
        finally:
            with self._condition:
                self._pending_approvals.pop(request.request_id, None)

    def resolve_approval(
        self,
        request_id: str,
        action: PermissionAction,
        *,
        scope: str | None = None,
        note: str | None = None,
    ) -> dict[str, Any]:
        if action not in {PermissionAction.ALLOW, PermissionAction.DENY}:
            raise ValueError("approval action must be allow or deny")
        normalized_scope = _normalize_approval_scope(scope, action=action)
        with self._condition:
            pending = self._pending_approvals.pop(request_id, None)
            if pending is None:
                raise KeyError(f"Unknown approval request: {request_id}")
            if action == PermissionAction.ALLOW and normalized_scope == "always":
                self.approval_rules.append(_approval_rule(pending.request))
            self.status = "running"
            event = self._approval_resolved_event(
                pending=pending,
                action=action,
                status=self.status,
                scope=normalized_scope,
                note=note,
            )
            self._append_event_locked(event)
            self._condition.notify_all()

        self._set_approval_result(pending, action)
        return event.to_dict()

    def _approval_resolved_event(
        self,
        *,
        pending: _PendingApproval,
        action: PermissionAction,
        status: str,
        reason: str | None = None,
        scope: str | None = None,
        note: str | None = None,
    ) -> AppEvent:
        approval = {**pending.request.to_dict(), "action": action.value}
        if reason:
            approval["reason"] = reason
        if scope:
            approval["scope"] = scope
        if note:
            approval["note"] = note
        return lifecycle_event(
            sequence=len(self.events) + 1,
            method="turn/approval_resolved",
            thread_id=self.session_id,
            turn_id=self.id,
            status=status,
            approval=approval,
        )

    @staticmethod
    def _set_approval_result(pending: _PendingApproval, action: PermissionAction) -> None:
        def _resolve() -> None:
            if not pending.future.done():
                pending.future.set_result(action)

        pending.loop.call_soon_threadsafe(_resolve)

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "session_id": self.session_id,
            "status": self.status,
            "created_at_ms": self.created_at_ms,
            "final_answer": self.final_answer,
            "error": self.error,
            "trace": self.trace,
            "model_id": self.model_id,
            "provider_id": self.provider_id,
            "agent_name": self.agent_name,
            "variant": self.variant,
            "patch_count": len(self.patch_records),
            "latest_patch_hash": self.patch_records[-1].get("hash") if self.patch_records else None,
            "event_count": len(self.events),
            "interrupt_requested": self.interrupt_requested,
            "pending_approval_count": len(self._pending_approvals),
            "approval_rule_count": len(self.approval_rules),
        }


class OpenAgentAppRuntime:
    """Small in-process runtime used by the local UI server."""

    def __init__(
        self,
        *,
        workspace: str | Path | None = None,
        session_store_root: str | Path | None = None,
        language_model_factory: LanguageModelFactory | None = None,
    ) -> None:
        self.workspace = Path(workspace or os.getenv("OPENAGENT_WORKSPACE") or Path.cwd()).resolve()
        raw_session_root = session_store_root or os.getenv("OPENAGENT_SESSION_ROOT") or DEFAULT_SESSION_STORE_ROOT
        self.session_store_root = Path(raw_session_root)
        if not self.session_store_root.is_absolute():
            self.session_store_root = self.workspace / self.session_store_root
        self.session_store = FileSessionStore(self.session_store_root)
        self.language_model_factory = language_model_factory
        self.provider = create_provider()
        self._sessions: dict[str, Session] = {}
        self._turns: dict[str, TurnRecord] = {}
        self._lock = threading.Lock()
        self._global_events: list[AppEvent] = []
        self._global_condition = threading.Condition()
        self._tui_control_requests: deque[TuiControlRequest] = deque()
        self._tui_control_responses: deque[Any] = deque()
        self._tui_control_condition = threading.Condition()

    def start_session(self, *, cwd: str | Path | None = None) -> dict[str, Any]:
        session = Session(directory=Path(cwd or self.workspace).resolve())
        with self._lock:
            self._sessions[session.id] = session
        return self._session_to_dict(session)

    def resume_session(self, session_id: str) -> dict[str, Any]:
        session = self._get_session(session_id)
        return self._session_to_dict(session)

    def list_sessions(self) -> list[dict[str, Any]]:
        sessions: dict[str, dict[str, Any]] = {}
        with self._lock:
            for session in self._sessions.values():
                if not _session_archived(session):
                    sessions[session.id] = self._session_to_dict(session)

        if self.session_store_root.exists():
            for state_path in self.session_store_root.glob("*/state.latest.json"):
                try:
                    session = self.session_store.load_session(state_path.parent.name)
                except Exception:  # noqa: BLE001
                    continue
                if not _session_archived(session):
                    sessions[session.id] = self._session_to_dict(session)
        return sorted(sessions.values(), key=lambda item: str(item.get("updated_at_ms") or ""), reverse=True)

    def get_session(self, session_id: str) -> dict[str, Any]:
        return self._session_to_dict(self._get_session(session_id), include_messages=True)

    def rename_session(self, session_id: str, title: str) -> dict[str, Any]:
        normalized = title.strip()
        if not normalized:
            raise ValueError("session title is required")
        session = self._get_session(session_id)
        meta = _session_tui_meta(session)
        meta["title"] = normalized
        meta["updated_at_ms"] = int(time.time() * 1000)
        self._persist_session_state(session)
        return self._session_to_dict(session)

    def archive_session(self, session_id: str) -> dict[str, Any]:
        session = self._get_session(session_id)
        meta = _session_tui_meta(session)
        meta["archived"] = True
        meta["updated_at_ms"] = int(time.time() * 1000)
        self._persist_session_state(session)
        return self._session_to_dict(session)

    def fork_session(self, session_id: str, *, title: str | None = None) -> dict[str, Any]:
        parent = self._get_session(session_id)
        child = parent.fork()
        child.metadata.pop(SESSION_STORE_METADATA_KEY, None)
        child.metadata.pop("agent_trace", None)
        parent_title = _session_title(parent) or parent.id
        meta = _session_tui_meta(child)
        meta.update(
            {
                "title": title.strip() if title and title.strip() else f"Fork of {parent_title}",
                "forked_from": parent.id,
                "archived": False,
                "updated_at_ms": int(time.time() * 1000),
            }
        )
        with self._lock:
            self._sessions[child.id] = child
        self._persist_session_state(child)
        return self._session_to_dict(child, include_messages=True)

    def list_models(self) -> list[dict[str, Any]]:
        async def _list() -> list[dict[str, Any]]:
            return [self._model_to_dict(model) for model in await self.provider.list_models()]

        return asyncio.run(_list())

    def list_agents(self) -> list[dict[str, str]]:
        return [
            {"id": "build", "name": "Build"},
            {"id": "plan", "name": "Plan"},
            {"id": "explore", "name": "Explore"},
        ]

    def start_turn(
        self,
        *,
        session_id: str,
        user_text: str,
        model_id: str | None = None,
        provider_id: str | None = None,
        agent_name: str | None = None,
        variant: str | None = None,
    ) -> TurnRecord:
        text = user_text.strip()
        if not text:
            raise ValueError("user_text is required")
        session = self._get_session(session_id)
        turn = TurnRecord(
            id=new_id("turn"),
            session_id=session.id,
            input=text,
            model_id=model_id,
            provider_id=provider_id,
            agent_name=agent_name,
            variant=variant,
            workspace=self.workspace,
            _event_publisher=self._append_global_event,
        )
        with self._lock:
            self._turns[turn.id] = turn
        thread = threading.Thread(target=self._run_turn_thread, args=(turn.id,), daemon=True)
        thread.start()
        return turn

    def get_turn(self, turn_id: str) -> TurnRecord:
        with self._lock:
            if turn_id not in self._turns:
                raise KeyError(f"Unknown turn: {turn_id}")
            return self._turns[turn_id]

    def wait_for_global_sequence(self, sequence: int, *, timeout_s: float = 15.0) -> AppEvent | None:
        deadline = time.time() + timeout_s
        with self._global_condition:
            while len(self._global_events) < sequence:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self._global_condition.wait(timeout=remaining)
            return self._global_events[sequence - 1]

    def enqueue_tui_control(self, path: str, body: Any = None) -> TuiControlRequest:
        request = TuiControlRequest(path=path, body={} if body is None else body)
        with self._tui_control_condition:
            if len(self._tui_control_requests) >= MAX_TUI_CONTROL_QUEUE:
                raise ValueError("TUI control queue is full")
            self._tui_control_requests.append(request)
            self._tui_control_condition.notify_all()
        return request

    def wait_for_tui_control(self, *, timeout_s: float = 0.25) -> TuiControlRequest | None:
        deadline = time.time() + max(0.0, timeout_s)
        with self._tui_control_condition:
            while not self._tui_control_requests:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self._tui_control_condition.wait(timeout=remaining)
            return self._tui_control_requests.popleft()

    def record_tui_control_response(self, payload: Any = None) -> Any:
        response = payload
        with self._tui_control_condition:
            self._tui_control_responses.append(response)
            self._tui_control_condition.notify_all()
        return response

    def next_tui_control_response(self, *, timeout_s: float = 0.0) -> Any | None:
        deadline = time.time() + max(0.0, timeout_s)
        with self._tui_control_condition:
            while not self._tui_control_responses:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self._tui_control_condition.wait(timeout=remaining)
            return self._tui_control_responses.popleft()

    def _append_global_event(self, event: AppEvent) -> None:
        with self._global_condition:
            if event.global_sequence is None:
                event.global_sequence = len(self._global_events) + 1
            self._global_events.append(event)
            self._global_condition.notify_all()

    def interrupt_turn(self, turn_id: str) -> dict[str, Any]:
        turn = self.get_turn(turn_id)
        turn.request_interrupt()
        return turn.to_dict()

    def revert_patch(self, turn_id: str, patch_ref: str = "last", *, target: str = "all") -> dict[str, Any]:
        turn = self.get_turn(turn_id)
        try:
            record = _resolve_patch_record(turn.patch_records, patch_ref)
            files = _patch_files_for_target(record, target)
            reverted: list[str] = []
            skipped: list[str] = []
            for item in files:
                ok, message = _revert_patch_file(self.workspace, item)
                if ok:
                    reverted.append(message)
                else:
                    skipped.append(message)
            if skipped and not reverted:
                raise ValueError("; ".join(skipped))
            method = "item/patch/reverted" if not skipped else "item/patch/revert_failed"
            event = lifecycle_event(
                sequence=turn.next_sequence(),
                method=method,
                thread_id=turn.session_id,
                turn_id=turn.id,
                patch_hash=record.get("hash"),
                target=target,
                reverted=reverted,
                skipped=skipped,
            )
            turn.append(event)
            return event.to_dict()
        except Exception as error:  # noqa: BLE001 - revert failures must surface as UI events.
            event = lifecycle_event(
                sequence=turn.next_sequence(),
                method="item/patch/revert_failed",
                thread_id=turn.session_id,
                turn_id=turn.id,
                patch_ref=patch_ref,
                target=target,
                error=str(error),
            )
            turn.append(event)
            raise

    def respond_approval(
        self,
        turn_id: str,
        request_id: str,
        action: str,
        *,
        scope: str | None = None,
        note: str | None = None,
    ) -> dict[str, Any]:
        turn = self.get_turn(turn_id)
        normalized_action, normalized_scope = _normalize_approval_action(action, scope=scope)
        return turn.resolve_approval(request_id, normalized_action, scope=normalized_scope, note=note)

    def _run_turn_thread(self, turn_id: str) -> None:
        try:
            asyncio.run(self._run_turn(turn_id))
        except Exception as error:  # noqa: BLE001
            turn = self.get_turn(turn_id)
            turn.status = "failed"
            turn.error = str(error)
            turn.append(
                lifecycle_event(
                    sequence=turn.next_sequence(),
                    method="turn/failed",
                    thread_id=turn.session_id,
                    turn_id=turn.id,
                    error=str(error),
                )
            )
            with turn._condition:
                turn._condition.notify_all()

    async def _run_turn(self, turn_id: str) -> None:
        turn = self.get_turn(turn_id)
        session = self._get_session(turn.session_id)
        turn.status = "running"
        turn.append(
            lifecycle_event(
                sequence=turn.next_sequence(),
                method="turn/started",
                thread_id=session.id,
                turn_id=turn.id,
                input=turn.input,
            )
        )

        model_metadata = await self._select_model(model_id=turn.model_id, provider_id=turn.provider_id)
        turn.model_id = model_metadata.id
        turn.provider_id = model_metadata.provider_id
        language_model = await self._language_model(model_metadata)
        agent_config = self._agent_config(model_metadata, agent_name=turn.agent_name, variant=turn.variant)
        agent = _agent_for_name(agent_config.name)(config=agent_config, model=language_model, system_prompt="")
        permission_manager = PermissionManager(ask_user_func=turn.request_approval)
        loop = AgentLoop(agent=agent, session=session, permission_manager=permission_manager)
        text_chunks: list[str] = []
        saw_error = False

        event_stream = loop.run(turn.input)
        async for event in event_stream:
            if turn.is_interrupt_requested():
                await event_stream.aclose()
                break
            self._update_turn_accumulators(turn, event, text_chunks)
            if event.get("type") == "error":
                saw_error = True
            turn.append(
                stream_event_to_app_event(
                    event,
                    sequence=turn.next_sequence(),
                    thread_id=session.id,
                    turn_id=turn.id,
                )
            )

        turn.final_answer = "".join(text_chunks)
        turn.trace = _trace_metadata(session)
        interrupted = turn.is_interrupt_requested()
        turn.status = "interrupted" if interrupted else ("failed" if saw_error else "completed")
        turn.append(
            lifecycle_event(
                sequence=turn.next_sequence(),
                method="turn/interrupted" if interrupted else ("turn/failed" if saw_error else "turn/completed"),
                thread_id=session.id,
                turn_id=turn.id,
                status=turn.status,
                final_answer=turn.final_answer,
                trace=turn.trace,
            )
        )
        with turn._condition:
            turn._condition.notify_all()

    async def _language_model(self, model: Model) -> LanguageModel:
        if self.language_model_factory is None:
            return await self.provider.get_language_model(model)
        value = self.language_model_factory(model)
        if hasattr(value, "__await__"):
            return await value  # type: ignore[no-any-return]
        return value  # type: ignore[return-value]

    async def _select_model(self, *, model_id: str | None = None, provider_id: str | None = None) -> Model:
        models = await self.provider.list_models()
        if not models:
            raise ValueError("no models available")
        if not model_id:
            return models[0]
        for model in models:
            if model.id == model_id and (not provider_id or model.provider_id == provider_id):
                return model
        for model in models:
            if model.id.startswith(model_id) and (not provider_id or model.provider_id == provider_id):
                return model
        provider_label = f"{provider_id}/" if provider_id else ""
        raise ValueError(f"unknown model: {provider_label}{model_id}")

    def _agent_config(self, model: Model, *, agent_name: str | None = None, variant: str | None = None) -> AgentConfig:
        selected_agent = (agent_name or os.getenv("OPENAGENT_APP_AGENT_NAME") or "build").strip() or "build"
        selected_variant = (variant or os.getenv("OPENAGENT_VARIANT") or "default").strip() or "default"
        return AgentConfig(
            name=selected_agent,
            model=model,
            tools=_tools_from_env(),
            permission=_permission_from_env(),
            max_steps=_env_int("OPENAGENT_APP_MAX_STEPS", _env_int("OPENAGENT_MAX_STEPS", 50)),
            options={
                "session_store": {
                    "enabled": True,
                    "root_dir": str(self.session_store_root),
                },
                "trace": {
                    "enabled": True,
                    "root_dir": os.getenv("OPENAGENT_TRACE_ROOT") or ".openagent/traces",
                },
                "runtime_warnings": {
                    "enabled": True,
                },
                "variant": selected_variant,
            },
        )

    def _get_session(self, session_id: str) -> Session:
        with self._lock:
            session = self._sessions.get(session_id)
        if session is not None:
            return session
        session = self.session_store.load_session(session_id)
        with self._lock:
            self._sessions[session.id] = session
        return session

    def _persist_session_state(self, session: Session) -> None:
        metadata = session.metadata if isinstance(session.metadata, dict) else {}
        store_metadata = metadata.get(SESSION_STORE_METADATA_KEY) if isinstance(metadata.get(SESSION_STORE_METADATA_KEY), dict) else {}
        run_id = str(store_metadata.get("run_id") or "") or None
        self.session_store.save_state(session, run_id=run_id)

    def _session_to_dict(self, session: Session, *, include_messages: bool = False) -> dict[str, Any]:
        metadata = session.metadata if isinstance(session.metadata, dict) else {}
        store_metadata = metadata.get(SESSION_STORE_METADATA_KEY) if isinstance(metadata.get(SESSION_STORE_METADATA_KEY), dict) else {}
        tui_metadata = metadata.get("tui") if isinstance(metadata.get("tui"), dict) else {}
        payload: dict[str, Any] = {
            "id": session.id,
            "directory": str(session.directory),
            "status": session.status.value,
            "title": tui_metadata.get("title"),
            "archived": bool(tui_metadata.get("archived")),
            "forked_from": tui_metadata.get("forked_from"),
            "message_count": len(session.messages),
            "updated_at_ms": tui_metadata.get("updated_at_ms") or store_metadata.get("updated_at_ms"),
            "session_store": store_metadata,
        }
        if include_messages:
            payload["messages"] = [
                {
                    "role": message.role,
                    "content": message.content,
                    "name": message.name,
                    "tool_call_id": message.tool_call_id,
                    "metadata": message.metadata,
                }
                for message in session.messages
            ]
        return payload

    def _model_to_dict(self, model: Model) -> dict[str, Any]:
        return {
            "id": model.id,
            "provider_id": model.provider_id,
            "name": model.name,
            "context_window": model.context_window,
            "max_output": model.max_output,
        }

    def _update_turn_accumulators(self, turn: TurnRecord, event: StreamEvent, text_chunks: list[str]) -> None:
        if event.get("type") == "text-delta":
            text_chunks.append(str(event.get("text") or ""))
        elif event.get("type") == "error":
            turn.error = str(event.get("error") or "")
        elif event.get("type") == "patch":
            files = event.get("files")
            if isinstance(files, list):
                turn.patch_records.append(
                    {
                        "snapshot_id": event.get("snapshot_id"),
                        "hash": event.get("hash"),
                        "files": [dict(item) for item in files if isinstance(item, dict)],
                    }
                )
                turn.patch_records = turn.patch_records[-20:]


def _permission_from_env() -> str:
    value = (os.getenv("OPENAGENT_APP_PERMISSION") or "FULL").strip().upper()
    if value not in {"FULL", "READONLY", "PLAN_ONLY", "NONE"}:
        return "FULL"
    return value


def _agent_for_name(name: str) -> type[UniversalAgent]:
    normalized = name.strip().lower()
    if normalized == "plan":
        return PlanAgent  # type: ignore[return-value]
    if normalized == "explore":
        return ExploreAgent  # type: ignore[return-value]
    return UniversalAgent


def _tools_from_env() -> list[str] | str:
    raw = (os.getenv("OPENAGENT_APP_TOOLS") or "all").strip()
    if raw in {"all", "readonly"}:
        return raw
    values = [item.strip() for item in raw.split(",") if item.strip()]
    return values or "all"


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _trace_metadata(session: Session) -> dict[str, Any]:
    metadata = session.metadata if isinstance(session.metadata, dict) else {}
    agent_trace = metadata.get("agent_trace") if isinstance(metadata.get("agent_trace"), dict) else {}
    session_store = metadata.get("session_store") if isinstance(metadata.get("session_store"), dict) else {}
    return {
        "trace_id": agent_trace.get("trace_id"),
        "run_id": agent_trace.get("run_id") or session_store.get("run_id"),
        "summary_path": agent_trace.get("summary_path"),
        "trace_path": agent_trace.get("trace_path"),
        "session_store": session_store,
    }


def _session_tui_meta(session: Session) -> dict[str, Any]:
    metadata = session.metadata if isinstance(session.metadata, dict) else {}
    session.metadata = metadata
    tui_metadata = metadata.get("tui")
    if not isinstance(tui_metadata, dict):
        tui_metadata = {}
        metadata["tui"] = tui_metadata
    return tui_metadata


def _session_title(session: Session) -> str:
    metadata = session.metadata if isinstance(session.metadata, dict) else {}
    tui_metadata = metadata.get("tui") if isinstance(metadata.get("tui"), dict) else {}
    return str(tui_metadata.get("title") or "").strip()


def _session_archived(session: Session) -> bool:
    metadata = session.metadata if isinstance(session.metadata, dict) else {}
    tui_metadata = metadata.get("tui") if isinstance(metadata.get("tui"), dict) else {}
    return bool(tui_metadata.get("archived"))


def _normalize_approval_action(action: str, *, scope: str | None = None) -> tuple[PermissionAction, str]:
    normalized = str(action or "").strip().lower().replace("-", "_")
    normalized_scope = str(scope or "").strip().lower()
    if normalized in {"allow_always", "always"}:
        return PermissionAction.ALLOW, "always"
    if normalized in {"allow_once", "allow"}:
        return PermissionAction.ALLOW, _normalize_approval_scope(normalized_scope or "once", action=PermissionAction.ALLOW)
    if normalized in {"deny", "reject", "no"}:
        return PermissionAction.DENY, _normalize_approval_scope(normalized_scope or "once", action=PermissionAction.DENY)
    raise ValueError("approval action must be allow or deny")


def _normalize_approval_scope(scope: str | None, *, action: PermissionAction) -> str:
    if action == PermissionAction.DENY:
        return "once"
    normalized = str(scope or "once").strip().lower()
    if normalized in {"", "once"}:
        return "once"
    if normalized in {"always", "session"}:
        return "always"
    raise ValueError("approval scope must be once or always")


def _approval_rule(request: ApprovalRequest) -> dict[str, Any]:
    return {
        "tool_name": request.tool_name,
        "pattern": _approval_pattern(request.tool_input),
        "created_at_ms": int(time.time() * 1000),
    }


def _approval_rule_matches(rules: list[dict[str, Any]], *, tool_name: str, tool_input: dict[str, Any]) -> bool:
    pattern = _approval_pattern(tool_input)
    for rule in reversed(rules):
        if str(rule.get("tool_name") or "") == tool_name and str(rule.get("pattern") or "") == pattern:
            return True
    return False


def _approval_pattern(payload: dict[str, Any]) -> str:
    for key in ("file_path", "filePath", "path", "pattern", "command", "name"):
        value = payload.get(key)
        if isinstance(value, str) and value:
            return value
    try:
        return json.dumps(payload, sort_keys=True, ensure_ascii=False)
    except TypeError:
        return str(payload)


def _approval_preview(workspace: Path | None, tool_name: str, tool_input: dict[str, Any]) -> dict[str, Any]:
    normalized = tool_name.strip().lower()
    if normalized == "write":
        return _write_approval_preview(workspace, tool_input)
    if normalized == "edit":
        return _edit_approval_preview(workspace, tool_input)
    if normalized in {"bash", "shell"}:
        command = str(tool_input.get("command") or "")
        return {"kind": "command", "command": command, "warnings": _command_warnings(command)}
    return {"kind": "tool", "summary": _compact_preview_json(tool_input)}


def _write_approval_preview(workspace: Path | None, tool_input: dict[str, Any]) -> dict[str, Any]:
    raw_path = str(tool_input.get("file_path") or tool_input.get("path") or "")
    after_text = str(tool_input.get("content") or "")
    path, path_error = _resolve_preview_path(workspace, raw_path)
    before_text = ""
    status = "added"
    warnings: list[str] = []
    if path_error:
        warnings.append(path_error)
    elif path is not None and path.exists():
        status = "modified"
        try:
            before_text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            warnings.append("current file is not UTF-8 text")
        except OSError as error:
            warnings.append(str(error))
    diff = _unified_preview_diff(raw_path or "-", before_text, after_text)
    return {
        "kind": "file-write",
        "path": raw_path,
        "status": status,
        "chars": len(after_text),
        "diff": diff,
        "warnings": warnings,
    }


def _edit_approval_preview(workspace: Path | None, tool_input: dict[str, Any]) -> dict[str, Any]:
    raw_path = str(tool_input.get("file_path") or tool_input.get("path") or "")
    old = str(tool_input.get("old_string") or "")
    new = str(tool_input.get("new_string") or "")
    replace_all = bool(tool_input.get("replace_all"))
    path, path_error = _resolve_preview_path(workspace, raw_path)
    warnings: list[str] = []
    before_text = ""
    after_text = new if old == "" else ""
    if path_error:
        warnings.append(path_error)
    elif path is None or not path.exists():
        warnings.append("current file is missing")
    else:
        try:
            before_text = path.read_text(encoding="utf-8")
            if old == "":
                after_text = new
            elif old not in before_text:
                after_text = before_text
                warnings.append("old_string was not found")
            else:
                after_text = before_text.replace(old, new) if replace_all else before_text.replace(old, new, 1)
        except UnicodeDecodeError:
            warnings.append("current file is not UTF-8 text")
        except OSError as error:
            warnings.append(str(error))
    return {
        "kind": "file-edit",
        "path": raw_path,
        "replace_all": replace_all,
        "diff": _unified_preview_diff(raw_path or "-", before_text, after_text),
        "warnings": warnings,
    }


def _resolve_preview_path(workspace: Path | None, raw_path: str) -> tuple[Path | None, str | None]:
    if not raw_path:
        return None, "file path is missing"
    if workspace is None:
        return None, "workspace is unavailable"
    root = workspace.expanduser().resolve()
    raw = Path(raw_path).expanduser()
    target = raw.resolve() if raw.is_absolute() else (root / raw).resolve()
    try:
        target.relative_to(root)
    except ValueError:
        return target, "path escapes workspace"
    return target, None


def _unified_preview_diff(path: str, before: str, after: str, *, max_lines: int = 120) -> str:
    diff_lines = list(
        difflib.unified_diff(
            before.splitlines(),
            after.splitlines(),
            fromfile=f"a/{path}",
            tofile=f"b/{path}",
            lineterm="",
        )
    )
    if len(diff_lines) > max_lines:
        diff_lines = [*diff_lines[:max_lines], f"... diff truncated ({len(diff_lines) - max_lines} more lines) ..."]
    return "\n".join(diff_lines)


def _command_warnings(command: str) -> list[str]:
    risky = (" rm ", "rm -", "sudo ", "chmod ", "chown ", "curl ", "wget ")
    padded = f" {command.strip()} "
    return ["command may change system or network state"] if any(token in padded for token in risky) else []


def _compact_preview_json(value: Any, *, limit: int = 1200) -> str:
    try:
        rendered = json.dumps(value, ensure_ascii=False, sort_keys=True)
    except TypeError:
        rendered = str(value)
    if len(rendered) <= limit:
        return rendered
    return rendered[:limit].rstrip() + "... truncated ..."


def _resolve_patch_record(records: list[dict[str, Any]], patch_ref: str) -> dict[str, Any]:
    if not records:
        raise ValueError("no workspace patch available")
    normalized = (patch_ref or "last").strip()
    if normalized in {"last", "latest"}:
        return records[-1]
    matches = [record for record in records if str(record.get("hash") or "").startswith(normalized)]
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise ValueError(f"patch not found: {patch_ref}")
    raise ValueError(f"patch ref is ambiguous: {patch_ref}")


def _patch_files_for_target(record: dict[str, Any], target: str) -> list[dict[str, Any]]:
    files = [dict(item) for item in record.get("files", []) if isinstance(item, dict)]
    normalized = (target or "all").strip()
    if normalized in {"all", "*", "last", "latest"}:
        if not files:
            raise ValueError("patch has no files")
        return files
    try:
        index = int(normalized)
    except ValueError:
        index = 0
    if index:
        if 1 <= index <= len(files):
            return [files[index - 1]]
        raise ValueError(f"patch file index out of range: {target}")
    matches = [item for item in files if str(item.get("path") or "") == normalized or str(item.get("path") or "").endswith(normalized)]
    if len(matches) == 1:
        return matches
    if not matches:
        raise ValueError(f"patch file not found: {target}")
    raise ValueError("patch file target is ambiguous: " + ", ".join(str(item.get("path") or "-") for item in matches[:10]))


def _revert_patch_file(workspace: Path, item: dict[str, Any]) -> tuple[bool, str]:
    rel = str(item.get("path") or "").strip()
    if not rel:
        return False, "missing patch path"
    if not bool(item.get("text_available")):
        return False, f"{rel}: text snapshot unavailable"
    try:
        path = _resolve_workspace_file(workspace, rel)
    except ValueError as error:
        return False, f"{rel}: {error}"
    before_text = item.get("before_text")
    after_text = item.get("after_text")
    if after_text is None:
        if path.exists():
            return False, f"{rel}: current file exists; expected deleted state"
    else:
        try:
            current_text = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return False, f"{rel}: current file missing"
        except UnicodeDecodeError:
            return False, f"{rel}: current file is not UTF-8 text"
        after_sha = item.get("after_sha256")
        if after_sha and _sha256_text(current_text) != str(after_sha):
            return False, f"{rel}: current content changed after patch"
        if not after_sha and current_text != after_text:
            return False, f"{rel}: current content changed after patch"
    if before_text is None:
        if path.exists():
            path.unlink()
        return True, f"{rel}: deleted"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(str(before_text), encoding="utf-8")
    return True, f"{rel}: restored"


def _resolve_workspace_file(workspace: Path, rel: str) -> Path:
    if Path(rel).is_absolute():
        raise ValueError("absolute patch paths are not allowed")
    root = workspace.expanduser().resolve()
    target = (root / rel).resolve()
    try:
        target.relative_to(root)
    except ValueError as error:
        raise ValueError("patch path escapes workspace") from error
    return target


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()
