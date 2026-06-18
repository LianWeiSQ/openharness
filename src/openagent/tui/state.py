from __future__ import annotations

import asyncio
import os
import re
import hashlib
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.app_server.runtime import TurnRecord
from openagent.cli.custom_commands import discover_commands, inject_file_references, render_command, resolve_command
from openagent.core.execution.runtime import CommandResult, LocalWorkspaceRuntime
from openagent.core.tool.builtin.shell import FORBIDDEN_COMMAND_RE

from .formatting import TimelineLine, format_event

DEFAULT_TRANSCRIPT_LIMIT = 30
MAX_TRANSCRIPT_LIMIT = 100
MAX_PROMPT_HISTORY = 100
MAX_DRAFT_STASH = 30
DEFAULT_TUI_SHELL_TIMEOUT_MS = 30_000
DEFAULT_TUI_SHELL_OUTPUT_LIMIT = 8_000

BUILTIN_COMMANDS: tuple[tuple[str, str], ...] = (
    ("!<command>", "run a shell command and send the output as context"),
    ("/help", "show TUI commands"),
    ("/sessions", "open recent session picker"),
    ("/session <rename|archive|delete|fork|info> ...", "manage sessions"),
    ("/resume <id>", "resume a session by id or unique prefix"),
    ("/models", "open model picker"),
    ("/model <id|next|prev>", "select or cycle the active model"),
    ("/agents", "open agent picker"),
    ("/agent <name>", "select the active agent profile label"),
    ("/variants", "open variant picker"),
    ("/variant <name>", "select the active model variant label"),
    ("/diff [file|index|all]", "show the latest workspace patch diff"),
    ("/revert [file|index|all]", "safely revert files from the latest patch"),
    ("/history [limit]", "show recent submitted prompts"),
    ("/stash [push <text>|pop|list|clear]", "stash or restore composer drafts"),
    ("/transcript [limit]", "show recent messages from the current session"),
    ("/new", "start a new session"),
    ("/clear", "clear the visible timeline"),
    ("/status", "show current session, turn, and model status"),
    ("/commands", "list project/global custom commands"),
)

DEFAULT_AGENT_CHOICES: tuple[str, ...] = ("build", "plan", "explore")


@dataclass(slots=True)
class TuiState:
    runtime: Any
    session_id: str | None = None
    active_turn: TurnRecord | None = None
    next_event_index: int = 0
    timeline: list[TimelineLine] = field(default_factory=list)
    input_buffer: str = ""
    prompt_history: list[str] = field(default_factory=list)
    prompt_history_index: int | None = None
    prompt_history_draft: str = ""
    draft_stash: list[str] = field(default_factory=list)
    preserve_input_after_command: bool = False
    status: str = "idle"
    scroll: int = 0
    session_picker_open: bool = False
    session_picker_index: int = 0
    session_picker_sessions: list[dict[str, object]] = field(default_factory=list)
    active_approval: dict[str, Any] | None = None
    approval_note_mode: bool = False
    approval_note: str = ""
    file_picker_open: bool = False
    file_picker_index: int = 0
    file_picker_query: str = ""
    file_picker_matches: list[str] = field(default_factory=list)
    model_picker_open: bool = False
    model_picker_index: int = 0
    model_picker_models: list[dict[str, object]] = field(default_factory=list)
    selected_model_id: str | None = None
    selected_provider_id: str | None = None
    selected_agent: str = ""
    selected_variant: str = ""
    agent_picker_open: bool = False
    agent_picker_index: int = 0
    agent_picker_agents: list[str] = field(default_factory=list)
    variant_picker_open: bool = False
    variant_picker_index: int = 0
    variant_picker_variants: list[str] = field(default_factory=list)
    patch_records: list[dict[str, Any]] = field(default_factory=list)

    def __post_init__(self) -> None:
        if not self.selected_model_id:
            self.selected_model_id = os.getenv("OPENAGENT_MODEL") or os.getenv("OPENAI_MODEL") or None
        if not self.selected_agent:
            self.selected_agent = os.getenv("OPENAGENT_APP_AGENT_NAME") or "build"
        if not self.selected_variant:
            self.selected_variant = os.getenv("OPENAGENT_VARIANT") or "default"

    def ensure_session(self) -> str:
        if self.session_id:
            return self.session_id
        session = self.runtime.start_session()
        self.session_id = str(session["id"])
        self.status = "session ready"
        return self.session_id

    @property
    def is_running(self) -> bool:
        return self.active_turn is not None and self.active_turn.status not in {"completed", "failed", "interrupted"}

    def submit(self) -> bool:
        raw_text = self.input_buffer.strip()
        text, display_text, handled = self._prepare_submission(raw_text)
        if handled:
            if self.preserve_input_after_command:
                self.preserve_input_after_command = False
            else:
                self.input_buffer = ""
            return False
        if not text or self.is_running:
            return False
        session_id = self.ensure_session()
        self.active_turn = self._start_turn(session_id=session_id, user_text=text)
        self.next_event_index = 0
        self._record_prompt_history(display_text)
        self.input_buffer = ""
        self.prompt_history_index = None
        self.prompt_history_draft = ""
        self.status = "running"
        self.timeline.append(TimelineLine("user", f"> {display_text}", important=True))
        return True

    def apply_control_request(self, request: dict[str, Any]) -> dict[str, object]:
        path = str(request.get("path") or "")
        body = request.get("body")
        if path:
            action = _normalize_control_action(path.removeprefix("/tui/").strip("/"))
            params = dict(body) if isinstance(body, dict) else {}
            if action == "publish":
                params = dict(body) if isinstance(body, dict) else {}
        else:
            action = str(request.get("action") or request.get("type") or "")
            params = request.get("params")
        params = dict(params) if isinstance(params, dict) else {}
        if action == "publish":
            action, params = self._control_publish_to_action(params)
        action = _normalize_control_action(action)

        if action == "prompt.append":
            text = str(params.get("text") or "")
            self.input_buffer += text
            self._reset_prompt_history_cursor()
            self.refresh_file_picker()
            self.status = "prompt updated"
            return {"applied": True, "action": action}
        if action == "prompt.submit":
            submitted = self.submit()
            return {"applied": submitted, "action": action}
        if action == "prompt.clear":
            self.input_buffer = ""
            self._reset_prompt_history_cursor()
            self.close_file_picker(update_status=False)
            self.status = "prompt cleared"
            return {"applied": True, "action": action}
        if action == "help.open":
            self._show_help()
            return {"applied": True, "action": action}
        if action == "sessions.open":
            opened = self.open_session_picker(announce=True)
            return {"applied": opened, "action": action}
        if action == "model.open":
            opened = self.open_model_picker(announce=True)
            return {"applied": opened, "action": action}
        if action == "model.select":
            selected = self.select_model(
                str(params.get("modelID") or params.get("model_id") or params.get("id") or ""),
                provider_id=str(params.get("providerID") or params.get("provider_id") or "") or None,
            )
            return {"applied": selected, "action": action}
        if action == "agent.select":
            selected = self.select_agent(str(params.get("agent") or params.get("name") or ""))
            return {"applied": selected, "action": action}
        if action == "agent.open":
            opened = self.open_agent_picker(announce=True)
            return {"applied": opened, "action": action}
        if action == "agent.cycle":
            self.cycle_agent()
            return {"applied": True, "action": action, "agent": self.selected_agent}
        if action == "variant.open":
            opened = self.open_variant_picker(announce=True)
            return {"applied": opened, "action": action}
        if action == "variant.select":
            selected = self.select_variant(str(params.get("variant") or params.get("name") or ""))
            return {"applied": selected, "action": action}
        if action == "session.select":
            session_id = str(params.get("sessionID") or params.get("session_id") or "")
            if not session_id:
                self.timeline.append(TimelineLine("error", "control request missing sessionID", important=True))
                self.status = "control invalid"
                return {"applied": False, "action": action, "error": "sessionID is required"}
            self._resume_session_id(session_id)
            return {"applied": self.session_id == session_id, "action": action}
        if action == "toast.show":
            message = str(params.get("message") or "")
            if not message:
                self.status = "control invalid"
                return {"applied": False, "action": action, "error": "message is required"}
            title = str(params.get("title") or "toast")
            variant = str(params.get("variant") or "status").lower()
            kind = "error" if variant in {"error", "danger"} else ("warning" if variant in {"warn", "warning"} else "status")
            self.timeline.append(TimelineLine(kind, f"{title}: {message}", important=True))
            self.status = title
            return {"applied": True, "action": action}
        if action == "command.execute":
            command = str(params.get("command") or "")
            if not command:
                self.status = "control invalid"
                return {"applied": False, "action": action, "error": "command is required"}
            self.input_buffer = command if command.startswith("/") else f"/{command}"
            submitted = self.submit()
            return {"applied": submitted, "action": action}
        if action.startswith("theme.") or action.startswith("palette."):
            self.timeline.append(TimelineLine("warning", f"TUI control unsupported: {action}", important=True))
            self.status = "control unsupported"
            return {"applied": False, "action": action, "unsupported": True}

        self.timeline.append(TimelineLine("warning", f"unknown TUI control: {action or '-'}", important=True))
        self.status = "control unknown"
        return {"applied": False, "action": action, "unsupported": True}

    def _control_publish_to_action(self, params: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        topic = str(params.get("type") or params.get("topic") or params.get("event") or params.get("method") or "")
        payload = params.get("properties")
        if payload is None:
            payload = params.get("payload")
        body = (
            dict(payload)
            if isinstance(payload, dict)
            else {key: value for key, value in params.items() if key not in {"type", "topic", "event", "method", "properties", "payload"}}
        )
        return {
            "tui.prompt.append": "prompt.append",
            "tui.command.execute": "command.execute",
            "tui.toast.show": "toast.show",
            "tui.session.select": "session.select",
            "tui.model.select": "model.select",
            "tui.agent.open": "agent.open",
            "tui.agent.select": "agent.select",
            "tui.agent.cycle": "agent.cycle",
            "tui.variant.open": "variant.open",
            "tui.variant.select": "variant.select",
        }.get(topic, topic), body

    def _prepare_submission(self, raw_text: str) -> tuple[str, str, bool]:
        if raw_text.startswith("!!"):
            text = raw_text[1:]
            return inject_file_references(text, workspace=self._workspace()), raw_text, False
        if raw_text.startswith("!"):
            return self._prepare_shell_submission(raw_text)
        if not raw_text or not raw_text.startswith("/") or raw_text.startswith("//"):
            text = raw_text[1:] if raw_text.startswith("//") else raw_text
            return inject_file_references(text, workspace=self._workspace()), raw_text, False
        command_line = raw_text[1:].strip()
        if not command_line:
            self._show_help()
            return "", raw_text, True
        name, *arguments = command_line.split()
        if self._handle_builtin_command(name, arguments):
            return "", raw_text, True
        try:
            command = resolve_command(name, workspace=self._workspace())
            rendered = render_command(command, arguments, workspace=self._workspace())
        except FileNotFoundError:
            self.timeline.append(TimelineLine("error", f"slash command not found: /{name}", important=True))
            self.status = "command not found"
            return "", raw_text, True
        except Exception as error:  # noqa: BLE001 - command rendering errors should be visible in the TUI.
            self.timeline.append(TimelineLine("error", f"slash command failed: /{name}\n{error}", important=True))
            self.status = "command failed"
            return "", raw_text, True
        self.timeline.append(TimelineLine("status", f"slash command: /{name}", important=True))
        return rendered, raw_text, False

    def _prepare_shell_submission(self, raw_text: str) -> tuple[str, str, bool]:
        command = raw_text[1:].strip()
        if not command:
            self.timeline.append(TimelineLine("warning", "usage: !<shell command>", important=True))
            self.status = "shell command invalid"
            return "", raw_text, True
        blocked = _blocked_shell_command(command)
        if blocked:
            self.timeline.append(TimelineLine("warning", f"{blocked} command is disabled for TUI shell commands", important=True))
            self.status = "shell command blocked"
            return "", raw_text, True
        timeout_ms = _tui_shell_timeout_ms()
        runtime = LocalWorkspaceRuntime(self._workspace())
        try:
            result = asyncio.run(runtime.run_command(command, None, timeout_ms))
        except subprocess.TimeoutExpired:
            self.timeline.append(TimelineLine("error", f"shell command timed out after {timeout_ms}ms: {command}", important=True))
            self.status = "shell command timed out"
            return "", raw_text, True
        except Exception as error:  # noqa: BLE001 - direct TUI shell failures should stay visible.
            self.timeline.append(TimelineLine("error", f"shell command failed: {command}\n{error}", important=True))
            self.status = "shell command failed"
            return "", raw_text, True

        output = _shell_output(result)
        trimmed = _trim_tui_shell_output(output)
        self.timeline.append(
            TimelineLine(
                "tool",
                f"$ {command}\n{trimmed}\nexit_code: {result.returncode}",
                important=True,
            )
        )
        self.status = "shell command ready" if result.returncode == 0 else "shell command failed"
        return _shell_context_prompt(command, result, trimmed), raw_text, False

    def _handle_builtin_command(self, name: str, arguments: list[str]) -> bool:
        if name in {"help", "?"}:
            self._show_help()
            return True
        if name == "commands":
            self._show_commands()
            return True
        if name == "sessions":
            self.open_session_picker(announce=True)
            return True
        if name == "session":
            self._session_from_command(arguments)
            return True
        if name == "models":
            self.open_model_picker(announce=True)
            return True
        if name == "model":
            self._model_from_command(arguments)
            return True
        if name == "agents":
            self.open_agent_picker(announce=True)
            return True
        if name == "agent":
            self._agent_from_command(arguments)
            return True
        if name == "variants":
            self.open_variant_picker(announce=True)
            return True
        if name == "variant":
            self._variant_from_command(arguments)
            return True
        if name == "diff":
            self._diff_from_command(arguments)
            return True
        if name in {"revert", "undo"}:
            self._revert_from_command(arguments)
            return True
        if name == "history":
            self._history_from_command(arguments)
            return True
        if name == "stash":
            self._stash_from_command(arguments)
            return True
        if name in {"resume", "continue"}:
            self._resume_from_command(arguments)
            return True
        if name == "transcript":
            self._transcript_from_command(arguments)
            return True
        if name == "new":
            session_id = self.new_session()
            self.timeline.append(TimelineLine("status", f"new session: {session_id}", important=True))
            return True
        if name == "clear":
            self.clear()
            return True
        if name == "status":
            self._show_status()
            return True
        return False

    @property
    def model_label(self) -> str:
        model = self.selected_model_id or "auto"
        if self.selected_provider_id:
            return f"{self.selected_provider_id}/{model}"
        return model

    @property
    def agent_label(self) -> str:
        return self.selected_agent or "build"

    @property
    def variant_label(self) -> str:
        return self.selected_variant or "default"

    def turn_options(self) -> dict[str, str]:
        options: dict[str, str] = {
            "agent_name": self.agent_label,
            "variant": self.variant_label,
        }
        if self.selected_model_id:
            options["model_id"] = self.selected_model_id
        if self.selected_provider_id:
            options["provider_id"] = self.selected_provider_id
        return options

    def _start_turn(self, *, session_id: str, user_text: str) -> TurnRecord:
        options = self.turn_options()
        try:
            return self.runtime.start_turn(session_id=session_id, user_text=user_text, **options)
        except TypeError as error:
            # Older or duck-typed test runtimes may not accept selector kwargs.
            if "unexpected keyword" not in str(error):
                raise
            return self.runtime.start_turn(session_id=session_id, user_text=user_text)

    def open_model_picker(self, *, announce: bool = False) -> bool:
        list_models = getattr(self.runtime, "list_models", None)
        if not callable(list_models):
            self.timeline.append(TimelineLine("warning", "model picker is not supported by this runtime", important=True))
            self.status = "model picker unsupported"
            return False
        try:
            models = [dict(item) for item in list_models() if isinstance(item, dict)]
        except Exception as error:  # noqa: BLE001 - picker failures should stay visible.
            self.timeline.append(TimelineLine("error", f"failed to open model picker: {error}", important=True))
            self.status = "model picker failed"
            return False
        if not models:
            self.timeline.append(TimelineLine("warning", "no models available", important=True))
            self.status = "no models"
            return False
        self._close_pickers(except_name="model", update_status=False)
        self.model_picker_models = models[:100]
        self.model_picker_index = self._current_model_picker_index()
        self.model_picker_open = True
        self.status = "model picker"
        if announce:
            self.timeline.append(TimelineLine("status", "model picker opened. Use Up/Down or j/k, Enter to select, Esc to close.", important=True))
        return True

    def close_model_picker(self) -> None:
        self.model_picker_open = False
        self.status = "model picker closed"

    def move_model_picker(self, delta: int) -> None:
        if not self.model_picker_open:
            return
        if not self.model_picker_models:
            self.open_model_picker()
            return
        self.model_picker_index = _bounded_index(self.model_picker_index + delta, len(self.model_picker_models))
        selected = self.selected_model()
        if selected:
            self.status = f"selected model {selected.get('id') or '-'}"

    def selected_model(self) -> dict[str, object] | None:
        if not self.model_picker_models:
            return None
        return self.model_picker_models[_bounded_index(self.model_picker_index, len(self.model_picker_models))]

    def select_model_from_picker(self) -> bool:
        selected = self.selected_model()
        if selected is None:
            self.status = "no model selected"
            return False
        applied = self.select_model(str(selected.get("id") or ""), provider_id=str(selected.get("provider_id") or "") or None)
        if applied:
            self.model_picker_open = False
        return applied

    def select_model(self, model_id: str, *, provider_id: str | None = None) -> bool:
        model_id = model_id.strip()
        if not model_id:
            self.timeline.append(TimelineLine("warning", "usage: /model <model-id|next|prev>", important=True))
            self.status = "model invalid"
            return False
        if model_id in {"next", "prev"}:
            return self.cycle_model(1 if model_id == "next" else -1)
        model = self._resolve_model(model_id, provider_id=provider_id)
        if model is None:
            self.timeline.append(TimelineLine("error", f"model not found: {model_id}", important=True))
            self.status = "model not found"
            return False
        self.selected_model_id = str(model.get("id") or model_id)
        self.selected_provider_id = str(model.get("provider_id") or provider_id or "") or None
        self._clear_invalid_variant()
        self.timeline.append(TimelineLine("status", f"model selected: {self.model_label}", important=True))
        self.status = "model selected"
        return True

    def cycle_model(self, delta: int) -> bool:
        if not self.model_picker_models and not self.open_model_picker(announce=False):
            return False
        if not self.model_picker_models:
            return False
        index = self._current_model_picker_index()
        self.model_picker_index = _bounded_index(index + delta, len(self.model_picker_models))
        return self.select_model_from_picker()

    def _model_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.open_model_picker(announce=True)
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /model <model-id|next|prev>", important=True))
            self.status = "model invalid"
            return
        self.select_model(arguments[0])

    def _resolve_model(self, model_id: str, *, provider_id: str | None = None) -> dict[str, object] | None:
        models = self.model_picker_models
        if not models:
            list_models = getattr(self.runtime, "list_models", None)
            if callable(list_models):
                try:
                    models = [dict(item) for item in list_models() if isinstance(item, dict)]
                except Exception:
                    models = []
        for model in models:
            raw_id = str(model.get("id") or "")
            raw_provider = str(model.get("provider_id") or "")
            if provider_id and raw_provider != provider_id:
                continue
            if raw_id == model_id or f"{raw_provider}/{raw_id}" == model_id:
                return model
        matches = [model for model in models if str(model.get("id") or "").startswith(model_id)]
        if len(matches) == 1:
            return matches[0]
        return None

    def _current_model_picker_index(self) -> int:
        for index, model in enumerate(self.model_picker_models):
            if str(model.get("id") or "") == (self.selected_model_id or "") and (
                not self.selected_provider_id or str(model.get("provider_id") or "") == self.selected_provider_id
            ):
                return index
        return 0

    def open_agent_picker(self, *, announce: bool = False) -> bool:
        agents = self._agent_choices()
        if not agents:
            self.timeline.append(TimelineLine("warning", "no agents available", important=True))
            self.status = "no agents"
            return False
        self._close_pickers(except_name="agent", update_status=False)
        self.agent_picker_agents = agents
        self.agent_picker_index = _bounded_index(agents.index(self.agent_label) if self.agent_label in agents else 0, len(agents))
        self.agent_picker_open = True
        self.status = "agent picker"
        if announce:
            self.timeline.append(TimelineLine("status", "agent picker opened. Use Up/Down or j/k, Enter to select, Esc to close.", important=True))
        return True

    def close_agent_picker(self) -> None:
        self.agent_picker_open = False
        self.status = "agent picker closed"

    def move_agent_picker(self, delta: int) -> None:
        if not self.agent_picker_open:
            return
        self.agent_picker_index = _bounded_index(self.agent_picker_index + delta, len(self.agent_picker_agents))
        self.status = f"selected agent {self.agent_picker_agents[self.agent_picker_index]}"

    def select_agent_from_picker(self) -> bool:
        if not self.agent_picker_agents:
            self.status = "no agent selected"
            return False
        applied = self.select_agent(self.agent_picker_agents[_bounded_index(self.agent_picker_index, len(self.agent_picker_agents))])
        if applied:
            self.agent_picker_open = False
        return applied

    def select_agent(self, name: str) -> bool:
        normalized = name.strip()
        choices = self._agent_choices()
        if not normalized:
            self.timeline.append(TimelineLine("warning", "usage: /agent <name|next|prev>", important=True))
            self.status = "agent invalid"
            return False
        if normalized in {"next", "prev"}:
            self.cycle_agent(1 if normalized == "next" else -1)
            return True
        matches = [agent for agent in choices if agent == normalized or agent.startswith(normalized)]
        if len(matches) != 1:
            self.timeline.append(TimelineLine("error", f"agent not found: {normalized}", important=True))
            self.status = "agent not found"
            return False
        self.selected_agent = matches[0]
        self.timeline.append(TimelineLine("status", f"agent selected: {self.selected_agent}", important=True))
        self.status = "agent selected"
        return True

    def cycle_agent(self, delta: int = 1) -> None:
        agents = self._agent_choices()
        index = agents.index(self.agent_label) if self.agent_label in agents else 0
        self.selected_agent = agents[_bounded_index(index + delta, len(agents))]
        self.timeline.append(TimelineLine("status", f"agent selected: {self.selected_agent}", important=True))
        self.status = "agent selected"

    def _agent_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.open_agent_picker(announce=True)
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /agent <name|next|prev>", important=True))
            self.status = "agent invalid"
            return
        self.select_agent(arguments[0])

    def _agent_choices(self) -> list[str]:
        list_agents = getattr(self.runtime, "list_agents", None)
        if callable(list_agents):
            try:
                raw_agents = list_agents()
            except Exception:
                raw_agents = []
            agents: list[str] = []
            for item in raw_agents:
                if isinstance(item, dict):
                    name = str(item.get("id") or item.get("name") or "").strip()
                else:
                    name = str(item).strip()
                if name:
                    agents.append(name)
            if agents:
                return agents
        return list(DEFAULT_AGENT_CHOICES)

    def open_variant_picker(self, *, announce: bool = False) -> bool:
        variants = self._variant_choices()
        if len(variants) <= 1:
            self.timeline.append(TimelineLine("warning", "no variants available for the selected model", important=True))
            self.status = "variants unavailable"
            return False
        self._close_pickers(except_name="variant", update_status=False)
        self.variant_picker_variants = variants
        self.variant_picker_index = _bounded_index(variants.index(self.variant_label) if self.variant_label in variants else 0, len(variants))
        self.variant_picker_open = True
        self.status = "variant picker"
        if announce:
            self.timeline.append(TimelineLine("status", "variant picker opened. Use Up/Down or j/k, Enter to select, Esc to close.", important=True))
        return True

    def close_variant_picker(self) -> None:
        self.variant_picker_open = False
        self.status = "variant picker closed"

    def move_variant_picker(self, delta: int) -> None:
        if not self.variant_picker_open:
            return
        self.variant_picker_index = _bounded_index(self.variant_picker_index + delta, len(self.variant_picker_variants))
        self.status = f"selected variant {self.variant_picker_variants[self.variant_picker_index]}"

    def select_variant_from_picker(self) -> bool:
        if not self.variant_picker_variants:
            self.status = "no variant selected"
            return False
        applied = self.select_variant(self.variant_picker_variants[_bounded_index(self.variant_picker_index, len(self.variant_picker_variants))])
        if applied:
            self.variant_picker_open = False
        return applied

    def select_variant(self, name: str) -> bool:
        normalized = name.strip()
        variants = self._variant_choices()
        if not normalized:
            self.timeline.append(TimelineLine("warning", "usage: /variant <name>", important=True))
            self.status = "variant invalid"
            return False
        matches = [variant for variant in variants if variant == normalized or variant.startswith(normalized)]
        if len(matches) != 1:
            self.timeline.append(TimelineLine("error", f"variant not found: {normalized}", important=True))
            self.status = "variant not found"
            return False
        self.selected_variant = matches[0]
        self.timeline.append(TimelineLine("status", f"variant selected: {self.selected_variant}", important=True))
        self.status = "variant selected"
        return True

    def _variant_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.open_variant_picker(announce=True)
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /variant <name>", important=True))
            self.status = "variant invalid"
            return
        self.select_variant(arguments[0])

    def _variant_choices(self) -> list[str]:
        model = self._selected_model_payload()
        raw_variants = model.get("variants") if isinstance(model, dict) else None
        variants = ["default"]
        if isinstance(raw_variants, list):
            for item in raw_variants:
                if isinstance(item, dict):
                    variant = str(item.get("id") or item.get("name") or "").strip()
                else:
                    variant = str(item).strip()
                if variant and variant not in variants:
                    variants.append(variant)
        return variants

    def _selected_model_payload(self) -> dict[str, object]:
        if self.selected_model_id:
            resolved = self._resolve_model(self.selected_model_id, provider_id=self.selected_provider_id)
            if resolved is not None:
                return resolved
        return {}

    def _clear_invalid_variant(self) -> None:
        variants = self._variant_choices()
        if self.variant_label not in variants:
            self.selected_variant = "default"

    def _close_pickers(self, *, except_name: str = "", update_status: bool = False) -> None:
        if except_name != "session":
            self.session_picker_open = False
        if except_name != "file":
            self.close_file_picker(update_status=False)
        if except_name != "model":
            self.model_picker_open = False
        if except_name != "agent":
            self.agent_picker_open = False
        if except_name != "variant":
            self.variant_picker_open = False
        if update_status:
            self.status = "picker closed"

    def _show_help(self) -> None:
        lines = [f"{name} - {description}" for name, description in BUILTIN_COMMANDS]
        self.timeline.append(TimelineLine("status", "built-in commands:\n" + "\n".join(lines), True))
        self.status = "help listed"

    def _show_sessions(self) -> None:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception as error:  # noqa: BLE001 - keep TUI failures visible.
            self.timeline.append(TimelineLine("error", f"failed to list sessions: {error}", important=True))
            self.status = "sessions failed"
            return
        if not sessions:
            self.timeline.append(TimelineLine("status", "no sessions found", important=True))
            self.status = "no sessions"
            return
        lines: list[str] = []
        for session in sessions[:20]:
            sid = str(session.get("id") or "-")
            marker = "*" if sid == self.session_id else " "
            status = str(session.get("status") or "-")
            message_count = session.get("message_count") or 0
            title = str(session.get("title") or "").strip()
            label = f"{title} ({sid})" if title else sid
            lines.append(f"{marker} {label}  {status}  {message_count} msg")
        self.timeline.append(TimelineLine("status", "sessions:\n" + "\n".join(lines), important=True))
        self.status = "sessions listed"

    def open_session_picker(self, *, announce: bool = False) -> bool:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception as error:  # noqa: BLE001 - keep picker failures visible.
            self.timeline.append(TimelineLine("error", f"failed to open session picker: {error}", important=True))
            self.status = "session picker failed"
            return False
        self.session_picker_sessions = sessions[:50]
        if not self.session_picker_sessions:
            self.session_picker_open = False
            self.session_picker_index = 0
            self.timeline.append(TimelineLine("status", "no sessions found", important=True))
            self.status = "no sessions"
            return False
        self.session_picker_index = self._current_session_picker_index()
        self.session_picker_open = True
        self.status = "session picker"
        if announce:
            self.timeline.append(
                TimelineLine(
                    "status",
                    "session picker opened. Use Up/Down or j/k, Enter to resume, Esc to close.",
                    important=True,
                )
            )
        return True

    def close_session_picker(self) -> None:
        self.session_picker_open = False
        self.status = "session picker closed"

    def move_session_picker(self, delta: int) -> None:
        if not self.session_picker_open:
            return
        if not self.session_picker_sessions:
            self.open_session_picker()
            return
        count = len(self.session_picker_sessions)
        self.session_picker_index = max(0, min(count - 1, self.session_picker_index + delta))
        selected = self.selected_session()
        if selected is not None:
            self.status = f"selected {selected.get('id') or '-'}"

    def selected_session(self) -> dict[str, object] | None:
        if not self.session_picker_sessions:
            return None
        index = max(0, min(len(self.session_picker_sessions) - 1, self.session_picker_index))
        return self.session_picker_sessions[index]

    def select_session_from_picker(self) -> bool:
        if not self.session_picker_open:
            return False
        selected = self.selected_session()
        if selected is None:
            self.status = "no session selected"
            return False
        session_id = str(selected.get("id") or "")
        if not session_id:
            self.status = "invalid session"
            return False
        self._resume_session_id(session_id)
        self.session_picker_open = False
        return True

    def resume_session(self, session_id: str) -> None:
        self._resume_session_id(session_id)

    def refresh_file_picker(self) -> None:
        span = self._active_file_mention_span()
        if span is None:
            self.close_file_picker(update_status=False)
            return
        _, _, query = span
        if query != self.file_picker_query:
            self.file_picker_index = 0
        self.file_picker_query = query
        self.file_picker_matches = self._search_file_mentions(query)
        self.file_picker_open = bool(self.file_picker_matches)
        if self.file_picker_index >= len(self.file_picker_matches):
            self.file_picker_index = max(0, len(self.file_picker_matches) - 1)
        if self.file_picker_open:
            self.status = "file picker"

    def close_file_picker(self, *, update_status: bool = True) -> None:
        self.file_picker_open = False
        self.file_picker_index = 0
        self.file_picker_query = ""
        self.file_picker_matches = []
        if update_status:
            self.status = "file picker closed"

    def move_file_picker(self, delta: int) -> None:
        if not self.file_picker_open:
            return
        count = len(self.file_picker_matches)
        if count == 0:
            self.close_file_picker()
            return
        self.file_picker_index = max(0, min(count - 1, self.file_picker_index + delta))
        self.status = f"selected @{self.file_picker_matches[self.file_picker_index]}"

    def selected_file_mention(self) -> str | None:
        if not self.file_picker_matches:
            return None
        index = max(0, min(len(self.file_picker_matches) - 1, self.file_picker_index))
        return self.file_picker_matches[index]

    def select_file_mention(self) -> bool:
        selected = self.selected_file_mention()
        span = self._active_file_mention_span()
        if selected is None or span is None:
            self.close_file_picker()
            return False
        start, end, _query = span
        self.input_buffer = self.input_buffer[:start] + "@" + selected + " " + self.input_buffer[end:]
        self.close_file_picker(update_status=False)
        self.status = f"inserted @{selected}"
        return True

    def _active_file_mention_span(self) -> tuple[int, int, str] | None:
        match = re.search(r"(?<!\S)@([A-Za-z0-9_./~+-]*)$", self.input_buffer)
        if not match:
            return None
        return match.start(), match.end(), match.group(1)

    def _search_file_mentions(self, query: str, *, limit: int = 30) -> list[str]:
        workspace = self._workspace()
        query_lower = query.lower()
        skip_dirs = {
            ".git",
            ".hg",
            ".svn",
            ".openagent",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
            ".venv",
            "__pycache__",
            "build",
            "dist",
            "node_modules",
        }
        matches: list[str] = []
        visited = 0
        for root, dirs, files in os.walk(workspace):
            dirs[:] = [name for name in dirs if name not in skip_dirs and not name.startswith(".tox")]
            for filename in files:
                if filename.startswith(".DS_Store"):
                    continue
                path = Path(root) / filename
                try:
                    rel = path.relative_to(workspace).as_posix()
                except ValueError:
                    continue
                visited += 1
                if visited > 5000:
                    break
                rel_lower = rel.lower()
                name_lower = filename.lower()
                if not query_lower or query_lower in rel_lower or name_lower.startswith(query_lower):
                    matches.append(rel)
            if visited > 5000:
                break
        return sorted(matches, key=lambda item: _file_match_score(item, query_lower))[:limit]

    def _current_session_picker_index(self) -> int:
        if self.session_id:
            for index, session in enumerate(self.session_picker_sessions):
                if str(session.get("id") or "") == self.session_id:
                    return index
        return 0

    def _resume_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.timeline.append(TimelineLine("warning", "usage: /resume <session-id-or-prefix>", important=True))
            self._show_sessions()
            return
        query = arguments[0]
        match = self._resolve_session_id(query)
        if match is None:
            self.timeline.append(TimelineLine("error", f"session not found: {query}", important=True))
            self.status = "session not found"
            return
        if isinstance(match, list):
            self.timeline.append(
                TimelineLine(
                    "error",
                    "session prefix is ambiguous:\n" + "\n".join(match[:10]),
                    important=True,
                )
            )
            self.status = "session ambiguous"
            return
        self._resume_session_id(match)

    def _session_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.open_session_picker(announce=True)
            return
        action, *rest = arguments
        if action == "rename":
            self._rename_session_from_command(rest)
            return
        if action in {"archive", "delete"}:
            self._archive_session_from_command(rest, label=action)
            return
        if action == "fork":
            self._fork_session_from_command(rest)
            return
        if action == "info":
            self._session_info_from_command(rest)
            return
        self.timeline.append(TimelineLine("warning", "usage: /session <rename|archive|delete|fork|info> ...", important=True))
        self.status = "session invalid"

    def _rename_session_from_command(self, arguments: list[str]) -> None:
        if len(arguments) < 2:
            self.timeline.append(TimelineLine("warning", "usage: /session rename <session-id|current> <title>", important=True))
            self.status = "session invalid"
            return
        session_id = self._session_arg_to_id(arguments[0])
        if session_id is None:
            return
        title = " ".join(arguments[1:]).strip()
        rename = getattr(self.runtime, "rename_session", None)
        if not callable(rename):
            self.timeline.append(TimelineLine("warning", "session rename is not supported by this runtime", important=True))
            self.status = "session unsupported"
            return
        try:
            session = rename(session_id, title)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to rename session: {error}", important=True))
            self.status = "session rename failed"
            return
        self._refresh_session_picker_cache()
        self.timeline.append(TimelineLine("status", f"session renamed: {session.get('title') or title}", important=True))
        self.status = "session renamed"

    def _archive_session_from_command(self, arguments: list[str], *, label: str) -> None:
        if len(arguments) != 1:
            self.timeline.append(TimelineLine("warning", f"usage: /session {label} <session-id|current>", important=True))
            self.status = "session invalid"
            return
        session_id = self._session_arg_to_id(arguments[0])
        if session_id is None:
            return
        archive = getattr(self.runtime, "archive_session", None)
        if not callable(archive):
            self.timeline.append(TimelineLine("warning", "session archive is not supported by this runtime", important=True))
            self.status = "session unsupported"
            return
        try:
            archive(session_id)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to archive session: {error}", important=True))
            self.status = "session archive failed"
            return
        self._refresh_session_picker_cache()
        if session_id == self.session_id:
            self.session_id = None
            self.active_turn = None
            self.next_event_index = 0
        self.timeline.append(TimelineLine("status", f"session archived: {session_id}", important=True))
        self.status = "session archived"

    def _fork_session_from_command(self, arguments: list[str]) -> None:
        title_parts = arguments
        if not arguments:
            session_id = self._session_arg_to_id("current")
        elif arguments[0] in {"current", "."}:
            session_id = self._session_arg_to_id(arguments[0])
            title_parts = arguments[1:]
        else:
            match = self._resolve_session_id(arguments[0])
            if isinstance(match, str):
                session_id = match
                title_parts = arguments[1:]
            elif isinstance(match, list):
                self.timeline.append(TimelineLine("error", "session prefix is ambiguous:\n" + "\n".join(match[:10]), important=True))
                self.status = "session ambiguous"
                return
            elif self.session_id:
                session_id = self.session_id
            else:
                self.timeline.append(TimelineLine("error", f"session not found: {arguments[0]}", important=True))
                self.status = "session not found"
                return
        if session_id is None:
            return
        title = " ".join(title_parts).strip() or None
        fork = getattr(self.runtime, "fork_session", None)
        if not callable(fork):
            self.timeline.append(TimelineLine("warning", "session fork is not supported by this runtime", important=True))
            self.status = "session unsupported"
            return
        try:
            session = fork(session_id, title=title)
        except TypeError as error:
            if "unexpected keyword" not in str(error):
                raise
            session = fork(session_id)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to fork session: {error}", important=True))
            self.status = "session fork failed"
            return
        new_id = str(session.get("id") or "")
        if new_id:
            self._resume_session_id(new_id)
            self.timeline.append(TimelineLine("status", f"session forked: {session_id} -> {new_id}", important=True))
            self.status = "session forked"
        self._refresh_session_picker_cache()

    def _session_info_from_command(self, arguments: list[str]) -> None:
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /session info [session-id|current]", important=True))
            self.status = "session invalid"
            return
        session_id = self._session_arg_to_id(arguments[0] if arguments else "current")
        if session_id is None:
            return
        try:
            session = self.runtime.get_session(session_id)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to load session: {error}", important=True))
            self.status = "session info failed"
            return
        lines = [
            f"id: {session.get('id') or session_id}",
            f"title: {session.get('title') or '-'}",
            f"status: {session.get('status') or '-'}",
            f"messages: {session.get('message_count') or 0}",
            f"archived: {bool(session.get('archived'))}",
            f"forked_from: {session.get('forked_from') or '-'}",
            f"directory: {session.get('directory') or '-'}",
        ]
        self.timeline.append(TimelineLine("status", "session info:\n" + "\n".join(lines), important=True))
        self.status = "session info"

    def _session_arg_to_id(self, query: str) -> str | None:
        if query in {"current", "."}:
            if not self.session_id:
                self.timeline.append(TimelineLine("error", "no active session", important=True))
                self.status = "no session"
                return None
            return self.session_id
        match = self._resolve_session_id(query)
        if match is None:
            self.timeline.append(TimelineLine("error", f"session not found: {query}", important=True))
            self.status = "session not found"
            return None
        if isinstance(match, list):
            self.timeline.append(TimelineLine("error", "session prefix is ambiguous:\n" + "\n".join(match[:10]), important=True))
            self.status = "session ambiguous"
            return None
        return match

    def _refresh_session_picker_cache(self) -> None:
        if not self.session_picker_open:
            return
        try:
            self.session_picker_sessions = list(self.runtime.list_sessions())[:50]
        except Exception:
            self.session_picker_sessions = []
        self.session_picker_index = self._current_session_picker_index()

    def _transcript_from_command(self, arguments: list[str]) -> None:
        limit = self._parse_transcript_limit(arguments)
        if limit is None:
            return
        if not self.session_id:
            self.timeline.append(TimelineLine("error", "no active session for transcript", important=True))
            self.status = "no session"
            return
        self._append_session_messages(self.session_id, limit=limit, announce=True, report_errors=True)

    def _parse_transcript_limit(self, arguments: list[str]) -> int | None:
        if not arguments:
            return DEFAULT_TRANSCRIPT_LIMIT
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /transcript [limit]", important=True))
            self.status = "transcript invalid"
            return None
        raw_limit = arguments[0]
        try:
            limit = int(raw_limit)
        except ValueError:
            self.timeline.append(TimelineLine("error", f"invalid transcript limit: {raw_limit}", important=True))
            self.status = "transcript invalid"
            return None
        if limit < 1 or limit > MAX_TRANSCRIPT_LIMIT:
            self.timeline.append(
                TimelineLine(
                    "error",
                    f"transcript limit must be between 1 and {MAX_TRANSCRIPT_LIMIT}",
                    important=True,
                )
            )
            self.status = "transcript invalid"
            return None
        return limit

    def _resume_session_id(self, session_id: str) -> None:
        try:
            session = self.runtime.resume_session(session_id)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to resume session {session_id}: {error}", important=True))
            self.status = "resume failed"
            return
        self.session_id = str(session.get("id") or session_id)
        self.active_turn = None
        self.next_event_index = 0
        self.input_buffer = ""
        self.timeline.clear()
        self._load_session_messages(self.session_id)
        self.timeline.append(TimelineLine("status", f"resumed session: {self.session_id}", important=True))
        self.status = "session resumed"

    def _resolve_session_id(self, query: str) -> str | list[str] | None:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception:
            return query
        ids = [str(session.get("id") or "") for session in sessions if session.get("id")]
        if query in ids:
            return query
        matches = [sid for sid in ids if sid.startswith(query)]
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            return matches
        return None

    def _load_session_messages(self, session_id: str) -> None:
        self._append_session_messages(session_id, limit=DEFAULT_TRANSCRIPT_LIMIT, announce=False, report_errors=False)

    def _append_session_messages(
        self,
        session_id: str,
        *,
        limit: int,
        announce: bool,
        report_errors: bool,
    ) -> None:
        get_session = getattr(self.runtime, "get_session", None)
        if not callable(get_session):
            if report_errors:
                self.timeline.append(TimelineLine("warning", "transcript is not supported by this runtime", important=True))
                self.status = "transcript unsupported"
            return
        try:
            payload = get_session(session_id)
        except Exception as error:  # noqa: BLE001 - transcript failures should be visible when requested.
            if report_errors:
                self.timeline.append(TimelineLine("error", f"failed to load transcript: {error}", important=True))
                self.status = "transcript failed"
            return
        messages = payload.get("messages") if isinstance(payload, dict) else None
        if not isinstance(messages, list):
            if report_errors:
                self.timeline.append(TimelineLine("error", "session transcript is unavailable", important=True))
                self.status = "transcript unavailable"
            return
        lines = self._session_message_lines(messages[-limit:])
        if not lines:
            if report_errors:
                self.timeline.append(TimelineLine("status", f"transcript is empty for session: {session_id}", important=True))
                self.status = "transcript empty"
            return
        if announce:
            shown_count = min(limit, len(messages))
            self.timeline.append(
                TimelineLine(
                    "status",
                    f"transcript: {session_id} (last {shown_count} of {len(messages)} messages)",
                    important=True,
                )
            )
        self.timeline.extend(lines)
        if report_errors:
            self.status = "transcript shown"

    def _session_message_lines(self, messages: list[object]) -> list[TimelineLine]:
        lines: list[TimelineLine] = []
        for message in messages:
            if not isinstance(message, dict):
                continue
            role = str(message.get("role") or "message")
            content = str(message.get("content") or "").strip()
            if not content:
                continue
            if role == "user":
                lines.append(TimelineLine("user", f"> {content}", important=True))
            elif role == "assistant":
                lines.append(TimelineLine("assistant", content, important=False))
            elif role == "tool":
                lines.append(TimelineLine("tool", f"tool result: {content}", important=False))
            else:
                lines.append(TimelineLine("event", f"{role}: {content}", important=False))
        return lines

    def _show_status(self) -> None:
        turn = self.active_turn
        lines = [
            f"session: {self.session_id or '-'}",
            f"turn: {getattr(turn, 'id', '-') if turn is not None else '-'}",
            f"turn_status: {turn.status if turn is not None else '-'}",
            f"model: {self.model_label}",
            f"agent: {self.agent_label}",
            f"variant: {self.variant_label}",
            f"patches: {len(self.patch_records)}",
            f"prompt_history: {len(self.prompt_history)}",
            f"draft_stash: {len(self.draft_stash)}",
            f"events: {len(turn.events) if turn is not None else 0}",
        ]
        workspace = getattr(self.runtime, "workspace", None)
        if workspace is not None:
            lines.append(f"workspace: {workspace}")
        self.timeline.append(TimelineLine("status", "status:\n" + "\n".join(lines), important=True))
        self.status = "status shown"

    def _show_commands(self) -> None:
        commands = discover_commands(workspace=self._workspace())
        builtin_lines = [f"{name} - {description}" for name, description in BUILTIN_COMMANDS]
        if not commands:
            self.timeline.append(
                TimelineLine(
                    "status",
                    "built-in commands:\n"
                    + "\n".join(builtin_lines)
                    + "\n\nno custom commands found in .openagent/commands or ~/.config/openagent/commands",
                    True,
                )
            )
            self.status = "commands listed"
            return
        lines = ["/" + command.name + (f" - {command.description}" if command.description else "") for command in commands]
        self.timeline.append(
            TimelineLine(
                "status",
                "built-in commands:\n" + "\n".join(builtin_lines) + "\n\ncustom commands:\n" + "\n".join(lines),
                True,
            )
        )
        self.status = "commands listed"

    def _workspace(self) -> Path:
        workspace = getattr(self.runtime, "workspace", None)
        if workspace is None:
            return Path.cwd()
        return Path(workspace).expanduser().resolve()

    def new_session(self) -> str:
        session = self.runtime.start_session()
        self.session_id = str(session["id"])
        self.active_turn = None
        self.next_event_index = 0
        self.input_buffer = ""
        self._reset_prompt_history_cursor()
        self.active_approval = None
        self.approval_note_mode = False
        self.approval_note = ""
        self._close_pickers(update_status=False)
        self.timeline.clear()
        self.status = "new session"
        return self.session_id

    def clear(self) -> None:
        self.timeline.clear()
        self.scroll = 0
        self._close_pickers(update_status=False)
        self.status = "cleared"

    def prompt_history_previous(self) -> bool:
        if not self.prompt_history:
            self.status = "history empty"
            return False
        if self.prompt_history_index is None:
            self.prompt_history_draft = self.input_buffer
            self.prompt_history_index = len(self.prompt_history) - 1
        else:
            self.prompt_history_index = max(0, self.prompt_history_index - 1)
        self.input_buffer = self.prompt_history[self.prompt_history_index]
        self.refresh_file_picker()
        self.status = f"history {self.prompt_history_index + 1}/{len(self.prompt_history)}"
        return True

    def prompt_history_next(self) -> bool:
        if self.prompt_history_index is None:
            self.status = "history current"
            return False
        if self.prompt_history_index >= len(self.prompt_history) - 1:
            self.input_buffer = self.prompt_history_draft
            self._reset_prompt_history_cursor()
            self.refresh_file_picker()
            self.status = "history current"
            return True
        self.prompt_history_index += 1
        self.input_buffer = self.prompt_history[self.prompt_history_index]
        self.refresh_file_picker()
        self.status = f"history {self.prompt_history_index + 1}/{len(self.prompt_history)}"
        return True

    def append_input_char(self, value: str) -> None:
        self.input_buffer += value
        self._reset_prompt_history_cursor()
        self.refresh_file_picker()

    def backspace_input(self) -> None:
        self.input_buffer = self.input_buffer[:-1]
        self._reset_prompt_history_cursor()
        self.refresh_file_picker()

    def stash_current_draft(self) -> bool:
        draft = self.input_buffer.strip()
        if not draft:
            self.timeline.append(TimelineLine("warning", "no draft to stash", important=True))
            self.status = "stash empty"
            return False
        self._stash_draft(self.input_buffer)
        self.input_buffer = ""
        self._reset_prompt_history_cursor()
        self.close_file_picker(update_status=False)
        self.timeline.append(TimelineLine("status", f"draft stashed ({len(self.draft_stash)})", important=True))
        self.status = "draft stashed"
        return True

    def pop_draft_stash(self) -> bool:
        if not self.draft_stash:
            self.timeline.append(TimelineLine("warning", "draft stash is empty", important=True))
            self.status = "stash empty"
            return False
        if self.input_buffer.strip():
            self.timeline.append(TimelineLine("warning", "clear or stash the current draft before popping", important=True))
            self.status = "stash blocked"
            return False
        self.input_buffer = self.draft_stash.pop()
        self._reset_prompt_history_cursor()
        self.refresh_file_picker()
        self.timeline.append(TimelineLine("status", f"draft restored ({len(self.draft_stash)} remaining)", important=True))
        self.status = "draft restored"
        return True

    def _stash_draft(self, value: str) -> None:
        self.draft_stash.append(value)
        self.draft_stash = self.draft_stash[-MAX_DRAFT_STASH:]

    def _history_from_command(self, arguments: list[str]) -> None:
        limit = self._parse_history_limit(arguments)
        if limit is None:
            return
        if not self.prompt_history:
            self.timeline.append(TimelineLine("status", "prompt history is empty", important=True))
            self.status = "history empty"
            return
        start = max(0, len(self.prompt_history) - limit)
        lines = [f"{index + 1}. {item}" for index, item in enumerate(self.prompt_history[start:], start=start)]
        self.timeline.append(TimelineLine("status", "prompt history:\n" + "\n".join(lines), important=True))
        self.status = "history listed"

    def _parse_history_limit(self, arguments: list[str]) -> int | None:
        if not arguments:
            return 20
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /history [limit]", important=True))
            self.status = "history invalid"
            return None
        try:
            limit = int(arguments[0])
        except ValueError:
            self.timeline.append(TimelineLine("error", f"invalid history limit: {arguments[0]}", important=True))
            self.status = "history invalid"
            return None
        if limit < 1 or limit > MAX_PROMPT_HISTORY:
            self.timeline.append(TimelineLine("error", f"history limit must be between 1 and {MAX_PROMPT_HISTORY}", important=True))
            self.status = "history invalid"
            return None
        return limit

    def _stash_from_command(self, arguments: list[str]) -> None:
        action = arguments[0] if arguments else "list"
        if action not in {"push", "pop", "list", "clear"}:
            self.timeline.append(TimelineLine("warning", "usage: /stash [push <text>|pop|list|clear]", important=True))
            self.status = "stash invalid"
            return
        if action == "push":
            if len(arguments) == 1:
                self.timeline.append(TimelineLine("warning", "use Ctrl-S to stash the current draft or /stash push <text>", important=True))
                self.status = "stash invalid"
                return
            self._stash_draft(" ".join(arguments[1:]))
            self.timeline.append(TimelineLine("status", f"draft stashed ({len(self.draft_stash)})", important=True))
            self.status = "draft stashed"
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /stash [push <text>|pop|list|clear]", important=True))
            self.status = "stash invalid"
            return
        if action == "pop":
            self.input_buffer = ""
            self.pop_draft_stash()
            self.preserve_input_after_command = True
            return
        if action == "clear":
            count = len(self.draft_stash)
            self.draft_stash.clear()
            self.timeline.append(TimelineLine("status", f"cleared {count} stashed draft(s)", important=True))
            self.status = "stash cleared"
            return
        if not self.draft_stash:
            self.timeline.append(TimelineLine("status", "draft stash is empty", important=True))
            self.status = "stash empty"
            return
        lines = [f"{index}. {draft}" for index, draft in enumerate(self.draft_stash, start=1)]
        self.timeline.append(TimelineLine("status", "draft stash:\n" + "\n".join(lines), important=True))
        self.status = "stash listed"

    def _record_prompt_history(self, value: str) -> None:
        normalized = value.strip()
        if not normalized:
            return
        if self.prompt_history and self.prompt_history[-1] == normalized:
            return
        self.prompt_history.append(normalized)
        self.prompt_history = self.prompt_history[-MAX_PROMPT_HISTORY:]

    def _reset_prompt_history_cursor(self) -> None:
        self.prompt_history_index = None
        self.prompt_history_draft = ""

    def poll_events(self) -> None:
        turn = self.active_turn
        if turn is None:
            return
        while self.next_event_index < len(turn.events):
            event = turn.events[self.next_event_index]
            self.timeline.extend(format_event(event))
            self._apply_control_event(event)
            self._apply_patch_event(event)
            self.next_event_index += 1
        if turn.status in {"completed", "failed", "interrupted"}:
            self.active_approval = None
            self.approval_note_mode = False
            self.approval_note = ""
        self.status = "approval required" if self.active_approval is not None else turn.status

    def _apply_control_event(self, event: AppEvent) -> None:
        if event.method == "turn/approval_requested":
            approval = event.params.get("approval")
            if isinstance(approval, dict):
                self.active_approval = dict(approval)
                self.approval_note_mode = False
                self.approval_note = ""
            return
        if event.method == "turn/approval_resolved":
            approval = event.params.get("approval")
            if not isinstance(approval, dict):
                self.active_approval = None
                self.approval_note_mode = False
                self.approval_note = ""
                return
            request_id = str(approval.get("request_id") or "")
            active_id = str((self.active_approval or {}).get("request_id") or "")
            if not request_id or request_id == active_id:
                self.active_approval = None
                self.approval_note_mode = False
                self.approval_note = ""

    def _apply_patch_event(self, event: AppEvent) -> None:
        raw = event.params.get("event") if isinstance(event.params.get("event"), dict) else {}
        if str(raw.get("type") or event.params.get("event_type") or "") != "patch":
            return
        files = raw.get("files")
        if not isinstance(files, list):
            return
        record = {
            "snapshot_id": raw.get("snapshot_id"),
            "hash": raw.get("hash"),
            "files": [dict(item) for item in files if isinstance(item, dict)],
        }
        if not record["files"]:
            return
        self.patch_records.append(record)
        self.patch_records = self.patch_records[-20:]

    def _diff_from_command(self, arguments: list[str]) -> None:
        record = self._latest_patch_record()
        if record is None:
            self.timeline.append(TimelineLine("warning", "no workspace patch available", important=True))
            self.status = "no patch"
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /diff [file|index|all]", important=True))
            self.status = "diff invalid"
            return
        target = arguments[0] if arguments else "all"
        files = self._patch_files_for_target(record, target)
        if files is None:
            return
        lines = [f"diff: {len(files)} file(s) hash={short_patch_hash(record)}"]
        for item in files:
            path = str(item.get("path") or "-")
            status = str(item.get("status") or "modified")
            diff = str(item.get("diff") or "").strip()
            lines.append(f"\n--- {status} {path}")
            lines.append(diff or "(no text diff available)")
        self.timeline.append(TimelineLine("patch", _trim_patch_text("\n".join(lines)), important=True))
        self.status = "diff shown"

    def _revert_from_command(self, arguments: list[str]) -> None:
        record = self._latest_patch_record()
        if record is None:
            self.timeline.append(TimelineLine("warning", "no workspace patch available", important=True))
            self.status = "no patch"
            return
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /revert [file|index|all]", important=True))
            self.status = "revert invalid"
            return
        target = arguments[0] if arguments else "all"
        if target in {"last", "latest"}:
            target = "all"
        files = self._patch_files_for_target(record, target)
        if files is None:
            return
        revert_patch = getattr(self.runtime, "revert_patch", None)
        if callable(revert_patch) and self.active_turn is not None:
            try:
                before_count = len(self.active_turn.events)
                result = revert_patch(self.active_turn.id, "last", target=target)
                self.poll_events()
                if len(self.active_turn.events) == before_count:
                    self.timeline.append(TimelineLine("status", "patch revert requested", important=True))
                method = str(result.get("method") if isinstance(result, dict) else "")
                self.status = "revert failed" if method == "item/patch/revert_failed" else "revert complete"
            except Exception as error:  # noqa: BLE001 - revert failures must be visible in the TUI.
                self.timeline.append(TimelineLine("error", f"revert failed: {error}", important=True))
                self.status = "revert failed"
            return
        reverted: list[str] = []
        skipped: list[str] = []
        for item in files:
            ok, message = self._revert_patch_file(item)
            if ok:
                reverted.append(message)
            else:
                skipped.append(message)
        lines = []
        if reverted:
            lines.append("reverted:\n" + "\n".join(f"- {item}" for item in reverted))
        if skipped:
            lines.append("not reverted:\n" + "\n".join(f"- {item}" for item in skipped))
        self.timeline.append(TimelineLine("patch", "\n\n".join(lines) or "nothing reverted", important=True))
        self.status = "revert complete" if reverted and not skipped else ("revert partial" if reverted else "revert failed")

    def _latest_patch_record(self) -> dict[str, Any] | None:
        if not self.patch_records:
            return None
        return self.patch_records[-1]

    def _patch_files_for_target(self, record: dict[str, Any], target: str) -> list[dict[str, Any]] | None:
        files = [dict(item) for item in record.get("files", []) if isinstance(item, dict)]
        if target in {"all", "*", "last", "latest"}:
            return files
        try:
            index = int(target)
        except ValueError:
            index = 0
        if index:
            if 1 <= index <= len(files):
                return [files[index - 1]]
            self.timeline.append(TimelineLine("error", f"patch file index out of range: {target}", important=True))
            self.status = "patch not found"
            return None
        matches = [item for item in files if str(item.get("path") or "") == target or str(item.get("path") or "").endswith(target)]
        if len(matches) == 1:
            return matches
        if not matches:
            self.timeline.append(TimelineLine("error", f"patch file not found: {target}", important=True))
            self.status = "patch not found"
            return None
        self.timeline.append(TimelineLine("error", "patch file target is ambiguous:\n" + "\n".join(str(item.get("path") or "-") for item in matches[:10]), important=True))
        self.status = "patch ambiguous"
        return None

    def _revert_patch_file(self, item: dict[str, Any]) -> tuple[bool, str]:
        rel = str(item.get("path") or "").strip()
        if not rel:
            return False, "missing patch path"
        if not bool(item.get("text_available")):
            return False, f"{rel}: text snapshot unavailable"
        try:
            path = _resolve_workspace_file(self._workspace(), rel)
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

    def request_interrupt(self) -> None:
        turn = self.active_turn
        if turn is None:
            self.status = "no active turn"
            return
        interrupt_turn = getattr(self.runtime, "interrupt_turn", None)
        if not callable(interrupt_turn):
            self.timeline.append(TimelineLine("warning", "interrupt is not supported by this runtime", important=True))
            self.status = "interrupt unsupported"
            return
        interrupt_turn(turn.id)
        self.status = "interrupting"

    def start_approval_note(self) -> None:
        if self.active_approval is None:
            self.status = "no approval"
            return
        self.approval_note_mode = True
        self.approval_note = ""
        self.status = "approval note"

    def cancel_approval_note(self) -> None:
        self.approval_note_mode = False
        self.approval_note = ""
        self.status = "approval required" if self.active_approval is not None else self.status

    def respond_approval(self, action: str, *, scope: str | None = None, note: str | None = None) -> bool:
        approval = self.active_approval
        if approval is None:
            self.status = "no approval"
            return False
        request_id = str(approval.get("request_id") or "")
        turn_id = str(approval.get("turn_id") or getattr(self.active_turn, "id", "") or "")
        if not request_id or not turn_id:
            self.timeline.append(TimelineLine("error", "approval request is missing an id", important=True))
            self.status = "approval invalid"
            return False
        respond_approval = getattr(self.runtime, "respond_approval", None)
        if not callable(respond_approval):
            self.timeline.append(TimelineLine("warning", "approval is not supported by this runtime", important=True))
            self.status = "approval unsupported"
            return False
        try:
            try:
                respond_approval(turn_id, request_id, action, scope=scope, note=note)
            except TypeError as error:
                if "unexpected keyword" not in str(error):
                    raise
                respond_approval(turn_id, request_id, action)
        except Exception as error:  # noqa: BLE001 - approval failures should stay visible in the TUI.
            self.timeline.append(TimelineLine("error", f"approval failed: {error}", important=True))
            self.status = "approval failed"
            return False
        self.active_approval = None
        self.approval_note_mode = False
        self.approval_note = ""
        suffix = f" {scope}" if scope else ""
        self.status = f"approval {action}{suffix} sent"
        return True


def _file_match_score(path: str, query: str) -> tuple[int, int, str]:
    lowered = path.lower()
    name = Path(path).name.lower()
    if query and lowered.startswith(query):
        rank = 0
    elif query and name.startswith(query):
        rank = 1
    elif query and query in lowered:
        rank = 2
    else:
        rank = 3
    return rank, len(path), path


def short_patch_hash(record: dict[str, Any]) -> str:
    value = str(record.get("hash") or "")
    return value[:12] + ("..." if len(value) > 12 else "")


def _trim_patch_text(value: str, *, limit: int = 12000) -> str:
    if len(value) <= limit:
        return value
    return value[:limit].rstrip() + "\n... diff truncated ..."


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


def _blocked_shell_command(command: str) -> str | None:
    match = FORBIDDEN_COMMAND_RE.search(command)
    return match.group(1) if match else None


def _tui_shell_timeout_ms() -> int:
    return _positive_env_int("OPENAGENT_TUI_SHELL_TIMEOUT_MS", DEFAULT_TUI_SHELL_TIMEOUT_MS)


def _tui_shell_output_limit() -> int:
    return _positive_env_int("OPENAGENT_TUI_SHELL_OUTPUT_LIMIT", DEFAULT_TUI_SHELL_OUTPUT_LIMIT)


def _positive_env_int(name: str, default: int) -> int:
    try:
        value = int(os.getenv(name, str(default)))
    except ValueError:
        return default
    return value if value > 0 else default


def _shell_output(result: CommandResult) -> str:
    output = ((result.stdout or "") + (result.stderr or "")).strip()
    return output or f"Command exited with return code {result.returncode}."


def _trim_tui_shell_output(value: str) -> str:
    limit = _tui_shell_output_limit()
    if len(value) <= limit:
        return value
    head = max(0, limit // 2)
    tail = max(0, limit - head)
    omitted = len(value) - head - tail
    return value[:head].rstrip() + f"\n... output truncated ({omitted} chars omitted) ...\n" + value[-tail:].lstrip()


def _shell_context_prompt(command: str, result: CommandResult, output: str) -> str:
    return (
        "A shell command was run from the TUI before this message. "
        "Use the command output as context for the next response.\n\n"
        f"$ {command}\n"
        f"cwd: {result.cwd}\n"
        f"exit_code: {result.returncode}\n"
        "output:\n"
        f"{output}"
    )


def _bounded_index(index: int, count: int) -> int:
    if count <= 0:
        return 0
    return max(0, min(count - 1, index))


def _normalize_control_action(action: str) -> str:
    return {
        "append-prompt": "prompt.append",
        "submit-prompt": "prompt.submit",
        "clear-prompt": "prompt.clear",
        "open-help": "help.open",
        "open-sessions": "sessions.open",
        "open-themes": "theme.open",
        "open-models": "model.open",
        "open-agents": "agent.open",
        "open-variants": "variant.open",
        "select-session": "session.select",
        "select-model": "model.select",
        "select-agent": "agent.select",
        "select-variant": "variant.select",
        "show-toast": "toast.show",
        "execute-command": "command.execute",
    }.get(action, action)
