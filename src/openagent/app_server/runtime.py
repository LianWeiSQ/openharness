from __future__ import annotations

import asyncio
import os
import threading
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.core.agent.universal import UniversalAgent
from openagent.core.id import new_id
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction
from openagent.core.provider.base import LanguageModel
from openagent.core.provider.openai import OpenAIProvider
from openagent.core.session.session import Session
from openagent.core.session.store import DEFAULT_SESSION_STORE_ROOT, FileSessionStore
from openagent.core.types import AgentConfig, Model, StreamEvent

from .protocol import AppEvent, lifecycle_event, stream_event_to_app_event

LanguageModelFactory = Callable[[Model], LanguageModel | Awaitable[LanguageModel]]


@dataclass(frozen=True, slots=True)
class ApprovalRequest:
    request_id: str
    session_id: str
    turn_id: str
    tool_name: str
    tool_input: dict[str, Any]
    call_id: str | None = None
    created_at_ms: int = field(default_factory=lambda: int(time.time() * 1000))

    def to_dict(self) -> dict[str, Any]:
        return {
            "request_id": self.request_id,
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "tool_input": self.tool_input,
            "call_id": self.call_id,
            "created_at_ms": self.created_at_ms,
        }


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
    interrupt_requested: bool = False
    events: list[AppEvent] = field(default_factory=list)
    _pending_approvals: dict[str, _PendingApproval] = field(default_factory=dict, repr=False)
    _condition: threading.Condition = field(default_factory=threading.Condition, repr=False)

    def append(self, event: AppEvent) -> None:
        with self._condition:
            self.events.append(event)
            self._condition.notify_all()

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
            self.events.append(event)
            approvals_to_resolve = list(self._pending_approvals.values())
            self._pending_approvals.clear()
            for pending in approvals_to_resolve:
                self.events.append(
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
        request = ApprovalRequest(
            request_id=new_id("approval"),
            session_id=self.session_id,
            turn_id=self.id,
            tool_name=str(tool_call.get("name") or tool_call.get("tool") or "tool"),
            tool_input=dict(tool_call.get("input") or {}),
            call_id=str(tool_call.get("call_id") or "") or None,
        )
        loop = asyncio.get_running_loop()
        future: asyncio.Future[PermissionAction] = loop.create_future()
        with self._condition:
            if self.interrupt_requested or self.status in {"completed", "failed", "interrupted"}:
                return PermissionAction.DENY
            self._pending_approvals[request.request_id] = _PendingApproval(request=request, loop=loop, future=future)
            self.status = "waiting_approval"
            self.events.append(
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

    def resolve_approval(self, request_id: str, action: PermissionAction) -> dict[str, Any]:
        if action not in {PermissionAction.ALLOW, PermissionAction.DENY}:
            raise ValueError("approval action must be allow or deny")
        with self._condition:
            pending = self._pending_approvals.pop(request_id, None)
            if pending is None:
                raise KeyError(f"Unknown approval request: {request_id}")
            self.status = "running"
            event = self._approval_resolved_event(pending=pending, action=action, status=self.status)
            self.events.append(event)
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
    ) -> AppEvent:
        approval = {**pending.request.to_dict(), "action": action.value}
        if reason:
            approval["reason"] = reason
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
            "event_count": len(self.events),
            "interrupt_requested": self.interrupt_requested,
            "pending_approval_count": len(self._pending_approvals),
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
        self.provider = OpenAIProvider()
        self._sessions: dict[str, Session] = {}
        self._turns: dict[str, TurnRecord] = {}
        self._lock = threading.Lock()

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
                sessions[session.id] = self._session_to_dict(session)

        if self.session_store_root.exists():
            for state_path in self.session_store_root.glob("*/state.latest.json"):
                try:
                    session = self.session_store.load_session(state_path.parent.name)
                except Exception:  # noqa: BLE001
                    continue
                sessions[session.id] = self._session_to_dict(session)
        return sorted(sessions.values(), key=lambda item: str(item.get("updated_at_ms") or ""), reverse=True)

    def get_session(self, session_id: str) -> dict[str, Any]:
        return self._session_to_dict(self._get_session(session_id), include_messages=True)

    def list_models(self) -> list[dict[str, Any]]:
        async def _list() -> list[dict[str, Any]]:
            return [self._model_to_dict(model) for model in await self.provider.list_models()]

        return asyncio.run(_list())

    def start_turn(self, *, session_id: str, user_text: str) -> TurnRecord:
        text = user_text.strip()
        if not text:
            raise ValueError("user_text is required")
        session = self._get_session(session_id)
        turn = TurnRecord(id=new_id("turn"), session_id=session.id, input=text)
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

    def interrupt_turn(self, turn_id: str) -> dict[str, Any]:
        turn = self.get_turn(turn_id)
        turn.request_interrupt()
        return turn.to_dict()

    def respond_approval(self, turn_id: str, request_id: str, action: str) -> dict[str, Any]:
        turn = self.get_turn(turn_id)
        normalized = PermissionAction(str(action).strip().lower())
        return turn.resolve_approval(request_id, normalized)

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

        model_metadata = (await self.provider.list_models())[0]
        language_model = await self._language_model(model_metadata)
        agent_config = self._agent_config(model_metadata)
        agent = UniversalAgent(config=agent_config, model=language_model, system_prompt="")
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

    def _agent_config(self, model: Model) -> AgentConfig:
        return AgentConfig(
            name=os.getenv("OPENAGENT_APP_AGENT_NAME") or "openagent-app",
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

    def _session_to_dict(self, session: Session, *, include_messages: bool = False) -> dict[str, Any]:
        metadata = session.metadata if isinstance(session.metadata, dict) else {}
        store_metadata = metadata.get("session_store") if isinstance(metadata.get("session_store"), dict) else {}
        payload: dict[str, Any] = {
            "id": session.id,
            "directory": str(session.directory),
            "status": session.status.value,
            "message_count": len(session.messages),
            "updated_at_ms": store_metadata.get("updated_at_ms"),
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


def _permission_from_env() -> str:
    value = (os.getenv("OPENAGENT_APP_PERMISSION") or "FULL").strip().upper()
    if value not in {"FULL", "READONLY", "PLAN_ONLY", "NONE"}:
        return "FULL"
    return value


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
