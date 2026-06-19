#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import io
import json
import os
import shutil
import sys
import urllib.error
from dataclasses import asdict, dataclass, field, fields, is_dataclass
from enum import Enum
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Literal
from unittest.mock import patch


REPO_ROOT = Path(__file__).resolve().parents[2]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from openagent.core.context_state import build_compaction_record, render_work_state
from openagent.app_server.protocol import (
    AppEvent,
    TuiControlRequest,
    lifecycle_event,
    stream_event_to_app_event,
    stream_event_to_app_method,
)
from openagent.app_server.server import (
    _is_authenticated_app_path,
    _parse_turn_approval_path,
    _publish_to_control,
)
from openagent.app_server.runtime import TurnRecord
from openagent.cli.main import (
    build_run_prompt,
    command_text_from_args,
    emit_app_bridge_events,
    format_http_error,
    join_server_url,
    normalize_server_url,
    parse_sse_response,
    quote_path,
    apply_model_env,
    build_parser,
    run_auth_command,
    run_config_command,
    run_custom_command,
    run_doctor_command,
    run_mcp_command,
    run_serve,
    select_client_session,
)
from openagent.core.context_budget import (
    check_context_budget,
    format_context_budget_error,
    load_context_budget_options,
)
from openagent.tui.remote_runtime import (
    RemoteTurnRecord,
    _app_event_from_dict,
    _event_session_id,
    _event_turn_id,
    _remote_event_key,
)
from openagent.tui.state import TuiState, _normalize_control_action
from openagent.sdk import http_runtime as sdk_http_runtime
from openagent.core.context_pack import ContextItem, ContextPackBuildOptions, ContextPackBuilder, estimate_text_tokens
from openagent.core.agent.universal import UniversalAgent
from openagent.core.instructions import InstructionContextLoader, InstructionLoadOptions
from openagent.core.loop.processor import AgentLoop, AgentLoopConfig
from openagent.core.message_materializer import RUNTIME_OPTION_KEYS, materialize_openai_compatible_payload
from openagent.core.mcp.bridge import register_mcp_tools
from openagent.core.mcp.config import load_mcp_config, load_mcp_config_from_sources
from openagent.core.mcp.runtime import (
    RemoteMcpManager,
    _build_result_metadata,
    _build_tool_descriptors,
    _dynamic_tool_name,
    _render_tool_result_output,
    _timeout_seconds,
    _tool_allowed,
    _transport_candidates,
)
from openagent.core.mcp.types import McpConfig, RemoteMcpToolCallResult
from openagent.core.observability import (
    ObservationConfig,
    ObservationEvent,
    TraceRecord,
    input_preview,
    output_stats,
    sanitize_observation_value,
)
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction, PermissionRule
from openagent.core.permission.ruleset import PermissionRuleset, ruleset
from openagent.core.provider.anthropic import AnthropicLanguageModel
from openagent.core.provider.metadata import (
    default_env_mapping,
    known_provider_ids,
    provider_auth_methods,
    provider_default_base_url,
    provider_default_model,
    provider_label,
    provider_requires_api_key,
)
from openagent.core.provider.openai import OpenAILanguageModel, _parse_tool_arguments, _summarize_http_error_body
from openagent.core.runtime_logging import RuntimeLogRecord, RuntimeLoggingConfig
from openagent.core.runtime_warnings import (
    RuntimeWarningConfig,
    RuntimeWarningRecord,
    format_runtime_warning_event,
)
from openagent.core.session.session import Session
from openagent.core.session.todo import TodoItem
from openagent.core.tool.definition import ToolContext, ToolDefinition, ToolExecutionSchema, ToolOutput
from openagent.core.tool.builtin import file as file_tools
from openagent.core.tool.builtin import memory as memory_tools
from openagent.core.tool.builtin import question as question_tools
from openagent.core.tool.builtin import search as search_tools
from openagent.core.tool.builtin import shell as shell_tools
from openagent.core.tool.builtin import todo as todo_tools
from openagent.core.tool.builtin.file import _format_read_output_from_text
from openagent.core.tool.builtin.shell import _blocked_command
from openagent.core.tool.registry import ToolRegistry
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.tool.truncation import Truncate
from openagent.core.tool.utils import ensure_within_root
from openagent.core.skill import SkillRegistry
from openagent.core.trace import render_trace_summary
from openagent.core.trace.recorder import sanitize_trace_value
from openagent.core.trace.schema import RunRecord, TraceConfig, TraceEvent
from openagent.core.types import (
    AgentConfig,
    ChatMessage,
    Model,
    ModelCapabilities,
    ModelPricing,
    SessionStatus,
    ToolCall,
    ToolResult,
    ToolSchema,
    Usage,
)
from mcp.types import TextContent
from swarm.protocol import (
    AgentDescriptor,
    AgentResult,
    AgentSpec,
    ArtifactRef,
    FanoutBudget,
    RunContext,
    RunLimits,
    usage_from_mapping,
)


@dataclass
class FixtureToolParams:
    path: str = field(metadata={"description": "Relative path inside the workspace."})
    mode: Literal["preview", "apply"] = "preview"
    max_lines: int = 40
    include_hidden: bool = False
    labels: list[str] = field(default_factory=list)
    weights: dict[str, int] = field(default_factory=dict)


@dataclass
class FixtureEchoParams:
    value: str = "ok"


@dataclass(slots=True)
class _FixtureScriptedLanguageModel:
    script: list[list[dict[str, Any]] | Exception]
    call_index: int = 0
    seen_tools_by_call: list[list[str]] = field(default_factory=list)
    seen_messages_by_call: list[list[ChatMessage]] = field(default_factory=list)
    seen_max_output_tokens_by_call: list[int | None] = field(default_factory=list)

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ):
        del system, temperature, options
        index = self.call_index
        self.call_index += 1
        self.seen_tools_by_call.append([getattr(tool, "name", str(tool)) for tool in tools])
        self.seen_messages_by_call.append(list(messages))
        self.seen_max_output_tokens_by_call.append(max_output_tokens)
        item = self.script[index] if index < len(self.script) else [{"type": "finish", "finish_reason": "stop", "usage": {}}]
        if isinstance(item, Exception):
            raise item
        for event in item:
            yield event


class _FixtureOpenAIResponse:
    def __init__(self, lines: list[bytes]) -> None:
        self._lines = lines
        self.status = 200
        self.headers: dict[str, str] = {"Content-Type": "application/json"}

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb) -> bool:
        return False

    def __iter__(self):
        return iter(self._lines)

    def read(self) -> bytes:
        return b"".join(self._lines)


class _FixtureAnthropicMessages:
    def __init__(self, client: "_FixtureAnthropicClient") -> None:
        self._client = client

    def create(self, **payload):
        self._client.requests.append(payload)
        return list(self._client.events)


class _FixtureAnthropicClient:
    def __init__(self, events: list[dict[str, Any]]) -> None:
        self.events = events
        self.requests: list[dict[str, Any]] = []
        self.messages = _FixtureAnthropicMessages(self)


class _FixtureMcpBridgeClient:
    def __init__(self, descriptors: list[Any], result: RemoteMcpToolCallResult) -> None:
        self.descriptors = descriptors
        self.result = result
        self.calls: list[dict[str, Any]] = []

    def list_tool_descriptors(self) -> list[Any]:
        return self.descriptors

    async def call_tool(self, dynamic_name: str, arguments: dict[str, object] | None) -> RemoteMcpToolCallResult:
        self.calls.append({"dynamic_name": dynamic_name, "arguments": arguments or {}})
        return self.result


def _stable(value: Any) -> Any:
    if isinstance(value, Enum):
        return value.value
    if is_dataclass(value) and not isinstance(value, type):
        result: dict[str, Any] = {}
        for item in fields(value):
            item_value = getattr(value, item.name)
            if callable(item_value):
                continue
            result[item.name] = _stable(item_value)
        return result
    if isinstance(value, Path):
        return value.as_posix()
    if isinstance(value, dict):
        return {str(key): _stable(value[key]) for key in sorted(value)}
    if isinstance(value, (list, tuple)):
        return [_stable(item) for item in value]
    return value


def _write_json(output_dir: Path, name: str, payload: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    path = output_dir / name
    path.write_text(json.dumps(_stable(payload), indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _core_protocol_fixture() -> dict[str, Any]:
    model = Model(
        id="gpt-fixture",
        provider_id="openai",
        name="Fixture Model",
        context_window=128000,
        max_output=4096,
        capabilities=ModelCapabilities(vision=True, tools=True, streaming=True, reasoning=False),
        pricing=ModelPricing(input_per_1m=1.25, output_per_1m=10.0),
    )
    tool_schema = ToolSchema(
        name="read",
        description="Read a workspace file.",
        schema={
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
        group="workspace",
        dangerous=False,
    )
    tool_call = ToolCall(name="read", input={"path": "README.md"}, call_id="call_fixture_read")
    tool_result = ToolResult(
        call_id=tool_call.call_id,
        output="OpenAgent fixture output",
        metadata={"tool": "read", "bytes": 24},
    )
    messages = [
        ChatMessage(role="user", content="Inspect README.md."),
        ChatMessage(
            role="assistant",
            content="",
            metadata={
                "tool_calls": [
                    {
                        "id": tool_call.call_id,
                        "type": "function",
                        "function": {
                            "name": tool_call.name,
                            "arguments": json.dumps(tool_call.input, sort_keys=True),
                        },
                    }
                ]
            },
        ),
        ChatMessage(role="tool", content=tool_result.output, tool_call_id=tool_result.call_id),
        ChatMessage(role="assistant", content="README.md was inspected."),
    ]
    payload = materialize_openai_compatible_payload(
        system="You are OpenAgent.",
        messages=messages,
        tools=[tool_schema],
        model=model,
        options={
            "temperature": 0.2,
            "trace": {"enabled": True},
            "runtime_warnings": {"enabled": True},
        },
    )
    return {
        "schema_version": 1,
        "model": model,
        "tool_schema": tool_schema,
        "tool_call": tool_call,
        "tool_call_key": tool_call.key(),
        "tool_result": tool_result,
        "usage": Usage(input_tokens=123, output_tokens=45, cost=0.00123),
        "stream_events": [
            {"type": "step-start", "snapshot_id": "snapshot_fixture"},
            {"type": "text-start", "id": "text_fixture", "metadata": {"channel": "final"}},
            {"type": "text-delta", "id": "text_fixture", "text": "hello"},
            {"type": "text-end", "id": "text_fixture"},
            {"type": "tool-call", "name": tool_call.name, "input": tool_call.input, "call_id": tool_call.call_id},
            {
                "type": "tool-result",
                "call_id": tool_result.call_id,
                "output": tool_result.output,
                "error": None,
                "metadata": tool_result.metadata,
            },
            {"type": "step-finish", "tokens": {"input": 123, "output": 45}, "cost": 0.00123, "finish_reason": "stop"},
        ],
        "runtime_option_keys": sorted(RUNTIME_OPTION_KEYS),
        "openai_payload": payload,
    }


def _permission_rulesets_fixture() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "rulesets": {
            item.value: ruleset(item)
            for item in (
                PermissionRuleset.FULL,
                PermissionRuleset.READONLY,
                PermissionRuleset.PLAN_ONLY,
                PermissionRuleset.NONE,
            )
        },
    }


def _tool_definition_schema_fixture() -> dict[str, Any]:
    definition = ToolDefinition(
        id="fixture_tool",
        description="Fixture tool for Rust schema parity.",
        parameters=FixtureToolParams,
        execute=lambda _params, _ctx: None,
        group="fixture",
        execution_scope="workspace",
        execution_schema=ToolExecutionSchema.readonly(batch_group="fixture", max_parallelism=2),
    )
    return {
        "schema_version": 1,
        "tool_id": definition.id,
        "description": definition.description,
        "group": definition.group,
        "execution_scope": definition.execution_scope,
        "execution_schema": definition.execution_schema.as_dict(),
        "parameters_schema": definition.parameters_schema(),
    }


def _tool_runtime_fixture() -> dict[str, Any]:
    registry = ToolRegistry()
    file_tools.register(registry)
    shell_tools.register(registry)
    search_tools.register(registry)
    memory_tools.register(registry)
    todo_tools.register(registry)
    question_tools.register(registry)

    selected = [
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "ls",
        "bash",
        "code_search",
        "memory_read",
        "memory_write",
        "todowrite",
        "todoread",
        "question",
    ]
    tools: dict[str, Any] = {}
    for tool_id in selected:
        tool = registry.get(tool_id)
        if tool is None:
            raise AssertionError(f"missing fixture tool: {tool_id}")
        parameter_schema = tool.parameters_schema()
        tools[tool_id] = {
            "group": tool.group,
            "dangerous": tool.dangerous,
            "execution_scope": tool.execution_scope,
            "execution_schema": tool.execution_schema.as_dict(),
            "parameter_schema": {
                "required": parameter_schema.get("required", []),
                "properties": sorted((parameter_schema.get("properties") or {}).keys()),
            },
        }

    read_output, read_preview, read_truncated = _format_read_output_from_text(
        "alpha\nbeta\ngamma\n",
        offset=1,
        limit=1,
    )
    line_truncation = Truncate.output("L1\nL2\nL3", max_lines=2, max_bytes=999)
    byte_truncation = Truncate.output("abcdef", max_lines=999, max_bytes=4)
    try:
        ensure_within_root(Path("/tmp/openagent-fixture"), Path("/tmp/outside.txt"))
    except PermissionError as error:
        path_escape_error = str(error)
    else:
        raise AssertionError("path escape fixture did not fail")

    return {
        "schema_version": 1,
        "tools": tools,
        "registry_namespace": {"default": "fixture", "custom": "fixture_custom"},
        "execution_schemas": {
            "readonly": ToolExecutionSchema.readonly(batch_group="workspace-read", mutates_session=True).as_dict(),
            "exclusive": ToolExecutionSchema.exclusive(
                batch_group="workspace-write",
                mutates_workspace=True,
                mutates_session=True,
                conflict_key_template="file:{file_path}",
            ).as_dict(),
        },
        "read_format": {
            "output": read_output,
            "preview": read_preview,
            "truncated": read_truncated,
        },
        "truncation": {
            "line": asdict(line_truncation),
            "byte": asdict(byte_truncation),
        },
        "path_escape_error": path_escape_error,
        "blocked_shell_command": _blocked_command("printf ok; rm -rf tmp"),
        "todo_output": json.dumps(
            [{"content": "port tools", "status": "in_progress", "priority": "high", "id": "todo-fixture"}],
            ensure_ascii=False,
            indent=2,
        ),
        "memory_outputs": {"missing": "null", "write": "ok"},
        "question_output": (
            'User has answered your questions: "Pick a mode"="Fast". '
            "You can now continue with the user's answers in mind."
        ),
    }


def _swarm_protocol_fixture() -> dict[str, Any]:
    spec = AgentSpec(
        role="reviewer",
        objective="Review a deterministic fixture.",
        context="The fixture should be stable and network-free.",
        boundaries="Do not modify files.",
        output_schema={
            "type": "object",
            "properties": {"summary": {"type": "string"}, "confidence": {"type": "number"}},
            "required": ["summary"],
        },
        inputs={"path": "README.md"},
        limits=RunLimits(max_steps=4, max_input_tokens=2048, max_output_tokens=512, max_cost=0.25, timeout_seconds=30),
        permissions="READONLY",
        metadata={"fixture": True},
    )
    spec.validate()
    result = AgentResult(
        status="completed",
        summary="Fixture review completed.",
        evidence=["README.md fixture evidence"],
        open_questions=[],
        confidence=0.92,
        artifacts=[ArtifactRef(kind="trace", uri="runs/fixture/trace.jsonl", title="Trace")],
        usage=usage_from_mapping({"input_tokens": 10, "output_tokens": 5, "cost": 0.0001, "steps": 1, "latency_ms": 12}),
        metadata={"runner": "fixture"},
    )
    return {
        "schema_version": 1,
        "budget": FanoutBudget(max_concurrent=0, max_total_workers=0, max_total_tokens=100, max_total_cost=1.5).normalized(),
        "descriptor": AgentDescriptor(
            id="fixture-runner",
            roles=["reviewer", "*"],
            tool_groups=["readonly"],
            model_tier="worker",
            max_context=8192,
            supports_streaming=True,
            kind="function",
            metadata={"fixture": True},
        ),
        "run_context": RunContext(run_id="run_fixture", parent_span_id="span_parent", metadata={"fixture": True}),
        "spec": spec,
        "result": result,
    }


def _context_state_fixture() -> dict[str, Any]:
    state = {
        "task": "Freeze Python behavior for Rust rewrite.",
        "progress": ["Captured protocol fixtures."],
        "decisions": ["Fixtures must be deterministic."],
        "files": [{"path": "doc/rust-rewrite-plan.md", "status": "created", "note": "Goal 0 contract."}],
        "tool_findings": ["No live network calls are required."],
        "todos": ["Compare Rust serde output against fixtures."],
        "open_questions": [],
        "blockers": [],
        "next_steps": ["Implement Rust protocol crate."],
        "risks": ["Later live-provider smoke tests need credentials."],
    }
    raw_text = json.dumps(state, sort_keys=True)
    return {
        "schema_version": 1,
        "rendered": render_work_state(state),
        "compaction_record": build_compaction_record(
            raw_text=raw_text,
            compacted_until=7,
            updated_at=1781840000000,
        ),
    }


async def _permission_decisions() -> dict[str, Any]:
    readonly = PermissionManager()
    readonly.set_ruleset(PermissionRuleset.READONLY)
    plan_only = PermissionManager()
    plan_only.set_ruleset(PermissionRuleset.PLAN_ONLY)
    custom = PermissionManager()
    custom.set_ruleset(PermissionRuleset.NONE)
    custom.add_rule(PermissionRule(tool="skill", action=PermissionAction.ALLOW, pattern="code-review"))
    return {
        "readonly_write": await readonly.check({"name": "write", "input": {"file_path": "a.txt", "content": "x"}}),
        "readonly_ls": await readonly.check({"name": "ls", "input": {}}),
        "readonly_skill": await readonly.check({"name": "skill", "input": {"name": "code-review"}}),
        "readonly_todowrite": await readonly.check({"name": "todowrite", "input": {"todos": []}}),
        "plan_only_todowrite": await plan_only.check({"name": "todowrite", "input": {"todos": []}}),
        "custom_skill": await custom.check({"name": "skill", "input": {"name": "code-review"}}),
        "pattern_for_file": PermissionManager._pattern_for({"file_path": "src/lib.rs", "command": "ignored"}),
        "pattern_for_name": PermissionManager._pattern_for({"name": "code-review"}),
        "pattern_for_json": PermissionManager._pattern_for({"b": 2, "a": 1}),
    }


def _write_skill_fixture(base: Path, relative: str, *, name: str, description: str, body: str = "") -> Path:
    path = base / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "\n".join(
            [
                "---",
                f"name: {name}",
                f"description: {description}",
                "---",
                "",
                body or f"# {name}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    return path


def _skill_issue_summary(issue: Any) -> dict[str, Any]:
    return {
        "kind": issue.kind,
        "path": Path(issue.path).name,
        "duplicate_of": Path(issue.duplicate_of).name if issue.duplicate_of else None,
    }


def _scrub_fixture_root(value: Any, root: Path) -> Any:
    stable = root.as_posix()
    replacements = {root.as_posix(): stable, root.resolve().as_posix(): stable}
    if stable.startswith("/tmp/"):
        replacements["/private" + stable] = stable
    if isinstance(value, (str, Path)):
        result = value.as_posix() if isinstance(value, Path) else value
        for needle, replacement in replacements.items():
            result = result.replace(needle, replacement)
        return result
    if isinstance(value, dict):
        return {key: _scrub_fixture_root(item, root) for key, item in value.items()}
    if isinstance(value, list):
        return [_scrub_fixture_root(item, root) for item in value]
    return value


def _run_cli_fixture(fn: Any, args: Any, **kwargs: Any) -> dict[str, Any]:
    stdout = io.StringIO()
    stderr = io.StringIO()
    code = fn(args, stdout=stdout, stderr=stderr, **kwargs)
    return {"exit_code": code, "stdout": stdout.getvalue(), "stderr": stderr.getvalue()}


def _run_cli_json_fixture(fn: Any, args: Any, **kwargs: Any) -> dict[str, Any]:
    result = _run_cli_fixture(fn, args, **kwargs)
    result["json"] = json.loads(result["stdout"]) if result["stdout"].strip() else None
    return result


def _namespace_subset(args: Any, keys: list[str]) -> dict[str, Any]:
    return {key: getattr(args, key, None) for key in keys}


def _core_context_policy_fixture() -> dict[str, Any]:
    fixture_root = Path("/tmp/openagent-rust-rewrite-fixture-goal6")
    shutil.rmtree(fixture_root, ignore_errors=True)
    workspace = fixture_root / "repo" / "project" / "workspace"
    user_dir = fixture_root / "user"
    workspace.mkdir(parents=True, exist_ok=True)
    user_dir.mkdir(parents=True, exist_ok=True)

    (fixture_root / "repo" / "AGENTS.md").write_text("Parent instruction", encoding="utf-8")
    (workspace / "OPENAGENT.md").write_text("Workspace rule", encoding="utf-8")
    rules_dir = workspace / ".openagent" / "rules"
    rules_dir.mkdir(parents=True, exist_ok=True)
    (rules_dir / "b.md").write_text("Rule B", encoding="utf-8")
    (rules_dir / "a.md").write_text("Rule A", encoding="utf-8")
    (user_dir / "OPENAGENT.md").write_text("User instruction", encoding="utf-8")

    _write_skill_fixture(workspace, ".openagent/skills/code-review/SKILL.md", name="code-review", description="Review code carefully", body="Inspect diffs and tests.")
    _write_skill_fixture(workspace, ".openagent/skills/research/SKILL.md", name="research", description="Research external sources", body="Collect evidence.")
    _write_skill_fixture(workspace, ".claude/skills/code-review/SKILL.md", name="code-review", description="duplicate", body="Duplicate should not win.")
    broken = workspace / ".openagent" / "skills" / "broken" / "SKILL.md"
    broken.parent.mkdir(parents=True, exist_ok=True)
    broken.write_text("# no frontmatter\n", encoding="utf-8")

    model = Model(
        id="context-fixture",
        provider_id="fixture",
        name="Context Fixture",
        context_window=96,
        max_output=24,
    )
    budget_messages = [
        ChatMessage(role="user", content="find matches"),
        ChatMessage(role="tool", name="code_search", content="x" * 1200),
    ]
    budget_result = check_context_budget(
        system="You are helpful.",
        messages=budget_messages,
        tools=[
            ToolSchema(
                name="large_tool",
                description="A" * 120,
                schema={"type": "object", "properties": {"query": {"type": "string", "description": "B" * 80}}},
            )
        ],
        model=model,
        options={"context_budget": {"strategy": "compact", "bytes_per_token": 4}},
        fallback_stage="goal6",
    )
    assert budget_result is not None
    try:
        load_context_budget_options({"context_budget": {"strategy": ""}}, model=model)
    except Exception as error:  # noqa: BLE001
        invalid_strategy = str(error)
    else:
        raise AssertionError("invalid strategy fixture did not fail")
    try:
        load_context_budget_options({"compaction": {"auto": "yes"}}, model=model)
    except Exception as error:  # noqa: BLE001
        invalid_compaction = str(error)
    else:
        raise AssertionError("invalid compaction fixture did not fail")

    context_pack = ContextPackBuilder(ContextPackBuildOptions(token_budget=24, bytes_per_token=4)).build(
        messages=[
            ChatMessage(role="user", content="old request"),
            ChatMessage(role="tool", name="grep", tool_call_id="call-grep", content="grep preview"),
            ChatMessage(role="user", content="new request"),
        ],
        metadata={
            "context_compaction": {
                "schema_version": 1,
                "format": "structured_work_state",
                "state": {"task": "Continue Rust rewrite", "next_steps": ["Port context"]},
                "summary": "ignored",
                "compacted_until": 2,
                "updated_at": 1781841000000,
            },
            "execution": {
                "mode": "opensandbox",
                "sandbox_id": "sbx_fixture",
                "remote_workdir": "/workspace/project",
                "connection": {"token": "secret"},
            },
        },
        todos=[TodoItem(content="port context", status="in_progress", priority="high", id="todo-context")],
        runtime_context="[Runtime]\nGoal 6 fixture",
        extra_items=[
            ContextItem(id="diag", kind="diagnostic", source="fixture", content="low", priority=1),
            ContextItem(id="diag", kind="diagnostic", source="fixture", content="high", priority=9),
        ],
    )

    instructions = InstructionContextLoader(
        workspace,
        InstructionLoadOptions(user_config_dir=user_dir, max_file_bytes=8, max_total_bytes=64),
    ).load()
    instruction_context_items = instructions.to_context_items()

    registry = SkillRegistry(session_root=workspace, home_dir=fixture_root / "home")
    report = registry.report(query="review", limit=5)
    loaded_skill = registry.get("code-review")
    assert loaded_skill is not None

    payload = {
        "schema_version": 1,
        "permission": asyncio.run(_permission_decisions()),
        "context_budget": {
            "config": load_context_budget_options(
                {
                    "compaction": {"auto": False, "prune": False, "reserved": 16},
                    "context_budget": {"strategy": "compact", "input_safety_margin_tokens": 8},
                },
                model=model,
            ),
            "result": budget_result,
            "error": format_context_budget_error(budget_result),
            "invalid_strategy": invalid_strategy,
            "invalid_compaction": invalid_compaction,
        },
        "context_pack": {
            "estimated_input_tokens": context_pack.estimated_input_tokens,
            "items": [
                {
                    "id": item.id,
                    "kind": item.kind,
                    "source": item.source,
                    "content": item.content,
                    "priority": item.priority,
                    "token_estimate": item.token_estimate,
                    "pinned": item.pinned,
                    "stable_prefix": item.stable_prefix,
                    "metadata": item.metadata,
                }
                for item in context_pack.items
            ],
            "trace": context_pack.trace_dicts(),
            "estimate_text_tokens": estimate_text_tokens("abcd", bytes_per_token=3),
        },
        "instructions": {
            "total_bytes": instructions.total_bytes,
            "truncated": instructions.truncated,
            "issues": instructions.issues,
            "items": [
                {
                    "display_path": item.display_path,
                    "source": item.source,
                    "scope": item.scope,
                    "content": item.content,
                    "bytes_read": item.bytes_read,
                    "truncated": item.truncated,
                }
                for item in instructions.items
            ],
            "context_items": [
                {
                    "kind": item.kind,
                    "source": item.source,
                    "content": item.content,
                    "priority": item.priority,
                    "pinned": item.pinned,
                    "stable_prefix": item.stable_prefix,
                    "metadata": item.metadata,
                }
                for item in instruction_context_items
            ],
        },
        "skills": {
            "report": {
                "skill_count": len(report.skills),
                "loaded_count": report.loaded_count,
                "scanned_files": report.scanned_files,
                "invalid_count": report.invalid_count,
                "duplicate_count": report.duplicate_count,
                "skills": [asdict(skill) for skill in report.skills],
                "issues": [_skill_issue_summary(issue) for issue in report.issues],
            },
            "loaded": asdict(loaded_skill),
            "search_all": [asdict(skill) for skill in registry.search("external evidence")],
        },
    }
    return _scrub_fixture_root(payload, fixture_root)


async def _openai_chat_stream_fixture() -> dict[str, Any]:
    chunks = [
        {"choices": [{"index": 0, "delta": {"content": "Hello "}, "finish_reason": None}]},
        {"choices": [{"index": 0, "delta": {"content": "world"}, "finish_reason": None}]},
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": "ls", "arguments": '{"path":'},
                            }
                        ]
                    },
                    "finish_reason": None,
                }
            ]
        },
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {"tool_calls": [{"index": 0, "function": {"arguments": '"."}'}}]},
                    "finish_reason": None,
                }
            ]
        },
        {
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
        },
    ]
    sse_lines = [f"data: {json.dumps(chunk, ensure_ascii=False)}\n".encode("utf-8") for chunk in chunks] + [b"data: [DONE]\n"]
    seen_payload: dict[str, Any] = {}
    seen_headers: dict[str, str] = {}

    def _fake_urlopen(req, timeout=None):
        nonlocal seen_payload, seen_headers
        del timeout
        seen_payload = json.loads((getattr(req, "data", None) or b"{}").decode("utf-8"))
        seen_headers = {key.lower(): value for key, value in req.header_items()}
        return _FixtureOpenAIResponse(sse_lines)

    model = OpenAILanguageModel(
        api_key="test",
        model_id="glm47",
        base_url="https://gateway.example.test/v1",
        host_header="model-gateway.example.test",
    )
    messages = [
        ChatMessage(role="user", content="show files"),
        ChatMessage(
            role="assistant",
            content="",
            metadata={
                "tool_calls": [
                    {
                        "id": "prior_call",
                        "type": "function",
                        "function": {"name": "ls", "arguments": '{"path":"."}'},
                    }
                ]
            },
        ),
        ChatMessage(role="tool", name="ls", tool_call_id="prior_call", content="[Tool result] ls"),
    ]
    tools = [
        ToolSchema(
            name="ls",
            description="List directory",
            schema={"type": "object", "properties": {"path": {"type": "string"}}},
        )
    ]
    events: list[dict[str, Any]] = []
    with patch("urllib.request.urlopen", new=_fake_urlopen):
        async for event in model.stream(system="You are helpful.", messages=messages, tools=tools):
            events.append(event)

    return {
        "chunks": chunks,
        "messages": messages,
        "tools": tools,
        "payload": seen_payload,
        "headers": seen_headers,
        "events": events,
    }


async def _openai_responses_fixture() -> dict[str, Any]:
    response_payload = {
        "output": [
            {
                "type": "function_call",
                "call_id": "call_hello",
                "name": "bash",
                "arguments": '{"command":"printf hello > hello.txt","timeout":10000}',
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "running"}],
            },
        ],
        "usage": {"input_tokens": 7, "output_tokens": 11},
    }
    seen_payload: dict[str, Any] = {}

    def _fake_urlopen(req, timeout=None):
        nonlocal seen_payload
        del timeout
        seen_payload = json.loads((getattr(req, "data", None) or b"{}").decode("utf-8"))
        return _FixtureOpenAIResponse([json.dumps(response_payload).encode("utf-8")])

    model = OpenAILanguageModel(
        api_key="test",
        model_id="gpt-5.4",
        base_url="https://example.invalid",
        wire_api="responses",
        reasoning_effort="xhigh",
        disable_response_storage=True,
    )
    messages = [
        ChatMessage(role="user", content="create hello"),
        ChatMessage(
            role="assistant",
            content="",
            metadata={
                "tool_calls": [
                    {
                        "id": "prior_call",
                        "type": "function",
                        "function": {"name": "bash", "arguments": '{"command":"ls"}'},
                    }
                ]
            },
        ),
        ChatMessage(role="tool", name="bash", tool_call_id="prior_call", content="[Tool result] bash"),
    ]
    tools = [
        ToolSchema(
            name="bash",
            description="Run a shell command",
            schema={"type": "object", "properties": {"command": {"type": "string"}}},
        )
    ]
    events: list[dict[str, Any]] = []
    with patch("urllib.request.urlopen", new=_fake_urlopen):
        async for event in model.stream(system="Use tools.", messages=messages, tools=tools):
            events.append(event)

    return {
        "response": response_payload,
        "messages": messages,
        "tools": tools,
        "payload": seen_payload,
        "events": events,
    }


async def _anthropic_stream_fixture() -> dict[str, Any]:
    anthropic_events = [
        {"type": "message_start", "message": {"usage": {"input_tokens": 12}}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello "}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "world"}},
        {
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {}},
        },
        {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": '{"command":"ls"'}},
        {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": ',"timeout":10}'}},
        {"type": "content_block_stop", "index": 1},
        {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 7}},
        {"type": "message_stop"},
    ]
    client = _FixtureAnthropicClient(anthropic_events)
    model = AnthropicLanguageModel(
        api_key="test",
        model_id="claude-test",
        client_factory=lambda **_: client,
    )
    messages = [
        ChatMessage(role="user", content="inspect repo"),
        ChatMessage(
            role="assistant",
            content="I'll list files.",
            metadata={
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "ls", "arguments": '{"path":"."}'},
                    }
                ]
            },
        ),
        ChatMessage(role="tool", name="ls", tool_call_id="call_1", content="[Tool result] ls"),
    ]
    tools = [
        ToolSchema(
            name="ls",
            description="List directory",
            schema={"type": "object", "properties": {"path": {"type": "string"}}},
        )
    ]
    events: list[dict[str, Any]] = []
    async for event in model.stream(
        system="Use tools.",
        messages=messages,
        tools=tools,
        temperature=0.2,
        max_output_tokens=123,
        options={"top_k": 4, "trace": {"enabled": True}},
    ):
        events.append(event)

    return {
        "source_events": anthropic_events,
        "messages": messages,
        "tools": tools,
        "payload": client.requests[0],
        "events": events,
    }


async def _provider_adapters_fixture_async() -> dict[str, Any]:
    with patch.dict("os.environ", {"OPENROUTER_API_KEY": "test"}, clear=True):
        auth_methods = provider_auth_methods("openrouter")
    return {
        "schema_version": 1,
        "metadata": {
            "known_provider_ids": known_provider_ids(),
            "openrouter_env": default_env_mapping("openrouter"),
            "custom_env": default_env_mapping("custom.gateway"),
            "anthropic_label": provider_label("anthropic"),
            "unknown_label": provider_label("custom.gateway"),
            "openrouter_default_base_url": provider_default_base_url("openrouter"),
            "anthropic_default_model": provider_default_model("anthropic"),
            "ollama_requires_api_key": provider_requires_api_key("ollama"),
            "openrouter_auth_methods": auth_methods,
        },
        "openai": {
            "tool_arguments": {
                "dict": _parse_tool_arguments({"path": "."}),
                "list": _parse_tool_arguments(["one", "two"]),
                "malformed": _parse_tool_arguments(
                    '{"query":"climate tipping points","num_results":8,"timeout":60'
                    '{"query":"climate tipping points","num_results":8,"timeout":60}'
                ),
                "raw": _parse_tool_arguments('{"path":'),
            },
            "http_errors": {
                "html": _summarize_http_error_body("<html><title>Bad Gateway</title></html>", "text/html"),
                "empty": _summarize_http_error_body("", "application/json"),
                "json": _summarize_http_error_body('{"error": {"message": "bad request"}}', "application/json"),
            },
            "chat_stream": await _openai_chat_stream_fixture(),
            "responses": await _openai_responses_fixture(),
        },
        "anthropic": await _anthropic_stream_fixture(),
    }


def _provider_adapters_fixture() -> dict[str, Any]:
    return asyncio.run(_provider_adapters_fixture_async())


def _loop_model_metadata() -> Model:
    return Model(
        id="loop-model",
        provider_id="test",
        name="Loop Test Model",
        context_window=8192,
        max_output=256,
    )


def _loop_toolkit() -> ToolkitAdapter:
    toolkit = ToolkitAdapter()

    async def _run_echo(args: FixtureEchoParams, _ctx: ToolContext) -> ToolOutput:
        return ToolOutput(title="Echo", output=f"echo:{args.value}", metadata={"kind": "fixture_echo"})

    toolkit.registry.define_tool(
        tool_id="fixture_echo",
        parameters=FixtureEchoParams,
        description="Echo a deterministic fixture value.",
        execution_scope="agnostic",
        execution_schema=ToolExecutionSchema.readonly(batch_group="fixture"),
    )(_run_echo)
    return toolkit


def _loop_tool_call_step(call_id: str, *, name: str = "fixture_echo", input: dict[str, Any] | None = None) -> list[dict[str, Any]]:
    return [
        {"type": "tool-call", "call_id": call_id, "name": name, "input": input or {"value": "alpha"}},
        {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 2, "output_tokens": 1, "cost": 0.0}},
    ]


def _loop_text_step(text: str, *, input_tokens: int = 3, output_tokens: int = 4, finish_reason: str = "stop") -> list[dict[str, Any]]:
    return [
        {"type": "text-delta", "id": "ignored", "text": text},
        {
            "type": "finish",
            "finish_reason": finish_reason,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens, "cost": 0.0},
        },
    ]


def _normalize_loop_event(event: dict[str, Any], state: dict[str, Any]) -> dict[str, Any]:
    normalized = _stable(event)
    event_type = normalized.get("type")
    if event_type == "step-start":
        state["snapshot_count"] = int(state.get("snapshot_count", 0)) + 1
        normalized["snapshot_id"] = f"snapshot_{state['snapshot_count']}"
    if event_type in {"text-start", "text-delta", "text-end"}:
        text_ids = state.setdefault("text_ids", {})
        text_id = str(normalized.get("id") or "")
        if text_id not in text_ids:
            text_ids[text_id] = f"text_{len(text_ids) + 1}"
        normalized["id"] = text_ids[text_id]
    if event_type == "question-request":
        question_ids = state.setdefault("question_ids", {})
        request_id = str(normalized.get("request_id") or "")
        if request_id not in question_ids:
            question_ids[request_id] = f"question_{len(question_ids) + 1}"
        normalized["request_id"] = question_ids[request_id]
        normalized["session_id"] = "session_fixture"
    if event_type == "tool-result":
        metadata = dict(normalized.get("metadata") or {})
        if metadata.get("request_id"):
            question_ids = state.setdefault("question_ids", {})
            request_id = str(metadata["request_id"])
            metadata["request_id"] = question_ids.get(request_id, "question_1")
        normalized["metadata"] = metadata
    return normalized


def _normalize_loop_message(message: ChatMessage, state: dict[str, Any]) -> dict[str, Any]:
    metadata = _stable(message.metadata)
    if isinstance(metadata, dict):
        metadata.pop("message_id", None)
    if isinstance(metadata, dict) and metadata.get("request_id"):
        metadata["request_id"] = state.get("question_ids", {}).get(str(metadata["request_id"]), "question_1")
    return {
        "role": message.role,
        "content": message.content,
        "name": message.name,
        "tool_call_id": message.tool_call_id,
        "metadata": metadata,
    }


async def _collect_loop_scenario(
    *,
    user_text: str,
    script: list[list[dict[str, Any]] | Exception],
    tools: list[str],
    options: dict[str, Any] | None = None,
    max_steps: int = 5,
    doom_loop_threshold: int = 3,
    reply_questions: bool = False,
    toolkit: ToolkitAdapter | None = None,
) -> dict[str, Any]:
    fixture_root = Path("/tmp/openagent-rust-rewrite-fixture-goal8")
    workspace = fixture_root / f"workspace_{len(script)}_{len(user_text)}_{len(tools)}"
    if workspace.exists():
        shutil.rmtree(workspace)
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("fixture workspace\n", encoding="utf-8")

    model = _FixtureScriptedLanguageModel(script=script)
    cfg = AgentConfig(
        name="fixture-agent",
        permission="FULL",
        max_steps=max_steps,
        tools=tools,
        model=_loop_model_metadata(),
        options=options or {},
    )
    agent = UniversalAgent(config=cfg, model=model, system_prompt="Fixture system prompt.")
    session = Session(id="session_fixture", directory=workspace)
    loop = AgentLoop(
        agent=agent,
        session=session,
        permission_manager=PermissionManager(),
        toolkit=toolkit or _loop_toolkit(),
        config=AgentLoopConfig(
            max_steps=max_steps,
            doom_loop_threshold=doom_loop_threshold,
            max_retry=1,
            retry_base_delay_s=0.0,
        ),
    )
    state: dict[str, Any] = {}
    pause_statuses: list[str] = []
    events: list[dict[str, Any]] = []
    async for event in loop.run(user_text):
        normalized = _normalize_loop_event(event, state)
        events.append(normalized)
        if event.get("type") == "question-request":
            pause_statuses.append(loop.session.status.value if isinstance(loop.session.status, SessionStatus) else str(loop.session.status))
            if reply_questions:
                loop.question_manager.reply(str(event["request_id"]), [["Fast path"]])

    return {
        "input": {
            "user_text": user_text,
            "script": [
                {"error": str(item)} if isinstance(item, Exception) else {"events": item}
                for item in script
            ],
            "tools": tools,
            "options": options or {},
            "max_steps": max_steps,
            "doom_loop_threshold": doom_loop_threshold,
            "reply_questions": reply_questions,
        },
        "events": events,
        "event_types": [event["type"] for event in events],
        "model_call_count": model.call_index,
        "seen_tools_by_call": model.seen_tools_by_call,
        "seen_max_output_tokens_by_call": model.seen_max_output_tokens_by_call,
        "pause_statuses": pause_statuses,
        "final_session_status": loop.session.status.value if isinstance(loop.session.status, SessionStatus) else str(loop.session.status),
        "session_messages": [_normalize_loop_message(message, state) for message in session.messages],
    }


async def _agent_loop_fixture_async() -> dict[str, Any]:
    question_step = [
        {
            "type": "tool-call",
            "call_id": "q1",
            "name": "question",
            "input": {
                "questions": [
                    {
                        "header": "Plan",
                        "question": "Which option should we use?",
                        "options": [
                            {"label": "Fast path", "description": "Move quickly"},
                            {"label": "Safe path", "description": "Be conservative"},
                        ],
                        "multiple": False,
                    }
                ]
            },
        },
        {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
    ]
    return {
        "schema_version": 1,
        "scenarios": {
            "multi_step_tool": await _collect_loop_scenario(
                user_text="run fixture tool",
                script=[
                    _loop_tool_call_step("echo_1", input={"value": "alpha"}),
                    _loop_text_step("Final answer after tool.", input_tokens=5, output_tokens=6),
                ],
                tools=["fixture_echo"],
                max_steps=5,
            ),
            "runtime_warning": await _collect_loop_scenario(
                user_text="warn on usage",
                script=[_loop_text_step("done", input_tokens=7, output_tokens=6)],
                tools=[],
                options={"runtime_warnings": {"enabled": True, "max_step_total_tokens": 10}},
                max_steps=5,
            ),
            "question_pause_reply": await _collect_loop_scenario(
                user_text="Need a choice",
                script=[
                    question_step,
                    _loop_text_step("Continuing with the chosen plan.", input_tokens=1, output_tokens=1),
                ],
                tools=["question"],
                max_steps=5,
                reply_questions=True,
            ),
            "model_retry": await _collect_loop_scenario(
                user_text="retry once",
                script=[
                    RuntimeError("temporary model outage"),
                    _loop_text_step("Recovered after retry.", input_tokens=2, output_tokens=2),
                ],
                tools=[],
                max_steps=5,
            ),
            "doom_loop": await _collect_loop_scenario(
                user_text="repeat tool",
                script=[
                    _loop_tool_call_step("echo_1", input={"value": "same"}),
                    _loop_tool_call_step("echo_2", input={"value": "same"}),
                    _loop_tool_call_step("echo_3", input={"value": "same"}),
                ],
                tools=["fixture_echo"],
                max_steps=6,
                doom_loop_threshold=3,
            ),
        },
    }


def _agent_loop_fixture() -> dict[str, Any]:
    return asyncio.run(_agent_loop_fixture_async())


def _session_trace_observability_fixture() -> dict[str, Any]:
    session_event = {
        "schema_version": "openagent.session_event.v1",
        "seq": 1,
        "event": "model.usage",
        "timestamp_ms": 1781840000100,
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "kind": "model",
        "status": "ok",
        "duration_ms": 12,
        "attributes": {
            "input_tokens": 11,
            "output_tokens": 7,
            "cost": 0.001,
            "authorization": "secret",
        },
    }
    session_part = {
        "schema_version": "openagent.session_part.v1",
        "part_id": "part_fixture",
        "seq": 1,
        "type": "usage",
        "timestamp_ms": 1781840000110,
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "step_index": 1,
        "status": "ok",
        "attributes": {"input_tokens": 11, "output_tokens": 7},
    }
    session_summary = {
        "schema_version": "openagent.run_summary.v1",
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "event_count": 2,
        "part_count": 1,
        "part_type_counts": {"usage": 1},
        "message_count": 1,
        "step_count": 0,
        "tool_call_count": 0,
        "runtime_warning_count": 0,
        "patch_count": 0,
        "total_input_tokens": 11,
        "total_output_tokens": 7,
        "total_cost": 0.001,
        "status": "completed",
    }
    session_state = {
        "schema_version": "openagent.session_state.v1",
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "workspace": "/tmp/openagent-fixture",
        "status": "idle",
        "updated_at_ms": 1781840000120,
        "messages": [
            {
                "message_id": "msg_fixture",
                "index": 0,
                "role": "user",
                "content": "Remember this fixture.",
                "name": None,
                "tool_call_id": None,
                "metadata": {"message_id": "msg_fixture"},
            }
        ],
        "todos": [TodoItem(content="port session store", status="in_progress", priority="high", id="todo-fixture")],
        "metadata": {
            "session_store": {
                "enabled": True,
                "type": "file",
                "root_dir": "/tmp/openagent-fixture/.openagent/sessions",
                "session_id": "session_fixture",
                "run_id": "run_fixture",
            }
        },
    }

    run = RunRecord(
        run_id="run_fixture",
        trace_id="trace_fixture",
        session_id="session_fixture",
        agent_name="fixture-agent",
        model_id="fixture-model",
        provider_id="fixture-provider",
        workspace="/tmp/openagent-fixture",
        started_at_ms=1781840000000,
    )
    trace_event = TraceEvent(
        seq=1,
        event="model.call.finished",
        event_id="event_fixture",
        timestamp_ms=1781840000200,
        run_id=run.run_id,
        trace_id=run.trace_id,
        session_id=run.session_id,
        kind="model",
        status="ok",
        span_id="span_model",
        parent_span_id="span_step",
        duration_ms=25,
        attributes=sanitize_trace_value(
            {
                "api_key": "secret",
                "input_tokens": 11,
                "output_tokens": 7,
                "cost": 0.001,
                "prompt": "P" * 4100,
            }
        ),
    )
    trace_summary = {
        **run.to_dict(),
        "status": "completed",
        "started_at_ms": run.started_at_ms,
        "ended_at_ms": 1781840000300,
        "duration_ms": 300,
        "event_count": 2,
        "step_count": 1,
        "model_call_count": 1,
        "tool_call_count": 0,
        "mcp_call_count": 0,
        "skill_call_count": 0,
        "local_tool_call_count": 0,
        "artifact_count": 0,
        "error_count": 0,
        "runtime_warning_count": 1,
        "total_latency_ms": 25,
        "total_input_tokens": 11,
        "total_output_tokens": 7,
        "total_reasoning_tokens": 0,
        "total_cache_read_tokens": 0,
        "total_cache_write_tokens": 0,
        "total_cost": 0.001,
        "errors": [],
        "paths": {
            "run_dir": "/tmp/openagent-fixture/.openagent/runs/run_fixture",
            "trace": "/tmp/openagent-fixture/.openagent/runs/run_fixture/trace.jsonl",
            "summary": "/tmp/openagent-fixture/.openagent/runs/run_fixture/summary.json",
            "process": "/tmp/openagent-fixture/.openagent/runs/run_fixture/process.md",
            "artifacts": "/tmp/openagent-fixture/.openagent/runs/run_fixture/artifacts",
        },
    }

    observation_trace = TraceRecord(
        trace_id="trace_fixture",
        session_id="session_fixture",
        run_id="run_fixture",
        agent_name="fixture-agent",
        model_id="fixture-model",
        provider_id="fixture-provider",
        workspace="/tmp/openagent-fixture",
        started_at_ms=1781840000000,
    )
    observation_event = ObservationEvent(
        event_id="event_observation",
        trace_id=observation_trace.trace_id,
        run_id=observation_trace.run_id,
        session_id=observation_trace.session_id,
        span_id="span_tool",
        parent_span_id="span_step",
        name="tool.call.finished",
        kind="tool",
        timestamp_ms=1781840000400,
        duration_ms=9,
        status="ok",
        attributes=sanitize_observation_value({"token": "secret", "output_lines": 2, "result_summary": "ok"}),
    )
    log_record = RuntimeLogRecord(
        log_id="log_fixture",
        timestamp_ms=1781840000500,
        level="WARNING",
        message="Tool output was truncated.",
        category="tool",
        session_id="session_fixture",
        run_id="run_fixture",
        trace_id="trace_fixture",
        span_id="span_tool",
        attributes=sanitize_observation_value({"authorization": "secret", "output_lines": 2}),
    )
    warning_record = RuntimeWarningRecord(
        code="step_total_tokens_exceeded",
        severity="warning",
        message="Step total tokens exceeded budget: 18 > 12.",
        metrics={"step_index": 1, "input_tokens": 11, "output_tokens": 7, "total_tokens": 18, "threshold": 12},
    )
    warning_event = warning_record.to_event()

    return {
        "schema_version": 1,
        "session": {
            "todo": TodoItem(content="port session store", status="in_progress", priority="high", id="todo-fixture"),
            "message": ChatMessage(role="user", content="Remember this fixture.", metadata={"message_id": "msg_fixture"}),
            "event": session_event,
            "part": session_part,
            "state": session_state,
            "summary": session_summary,
        },
        "trace": {
            "config": TraceConfig(root_dir="runs", max_events=12, exporters={"langfuse": {"enabled": False}}),
            "run": run,
            "event": trace_event,
            "summary": trace_summary,
            "rendered_summary": render_trace_summary(trace_summary),
        },
        "observability": {
            "config": ObservationConfig(jsonl=True, jsonl_dir="observability", max_events=3),
            "trace": observation_trace,
            "event": observation_event,
            "input_preview": input_preview({"api_key": "secret", "path": "README.md"}, max_chars=80),
            "output_stats": output_stats("one\ntwo\n"),
        },
        "runtime_logging": {
            "config": RuntimeLoggingConfig(jsonl=True, jsonl_dir="logs", level="WARNING", python_logging=False),
            "record": log_record,
        },
        "runtime_warnings": {
            "config": RuntimeWarningConfig(enabled=True, max_step_total_tokens=12),
            "record": warning_record,
            "event": warning_event,
            "formatted": format_runtime_warning_event(warning_event),
        },
    }


async def _execute_fixture_tool(tool: ToolDefinition, args: dict[str, Any]) -> ToolOutput:
    result = tool.execute(
        args,
        ToolContext(
            session_id="session_mcp",
            session_root=Path("/tmp/openagent-mcp-fixture"),
            call_id="call_mcp",
        ),
    )
    if asyncio.iscoroutine(result):
        return await result
    return result


def _mcp_call_result_fixture(descriptor: Any, result: Any, *, transport: str | None) -> RemoteMcpToolCallResult:
    output, non_text_blocks = _render_tool_result_output(result)
    is_error = bool(getattr(result, "isError", False))
    metadata = _build_result_metadata(
        descriptor,
        transport=transport,  # type: ignore[arg-type]
        is_error=is_error,
        non_text_blocks=non_text_blocks,
    )
    error = None
    if is_error:
        error = output or "Remote MCP tool returned an error."
        output = ""
    elif not output:
        output = "(Remote MCP tool completed with no textual output.)"
    return RemoteMcpToolCallResult(output=output, error=error, metadata=metadata)


def _mcp_runtime_fixture() -> dict[str, Any]:
    raw_config = {
        "refresh_ttl_s": "12.5",
        "mcp": {
            "Demo Server": {
                "type": "remote",
                "url": "https://mcp.example.test/demo",
                "transport": "auto",
                "enabled": True,
                "headers": {
                    "Authorization": "Bearer secret-token",
                    "X-Token": "token-secret",
                    "X-Client": "openagent-fixture",
                },
                "timeout_ms": 500,
                "tools": {
                    "allow": ["Weather*", "weather search", "Data-*"],
                    "deny": ["Data-secret"],
                },
            },
            "Event Server": {
                "type": "sse",
                "url": "https://mcp.example.test/sse",
                "enabled": False,
            },
            "Stream Server": {
                "type": "streamableHttp",
                "url": "https://mcp.example.test/stream",
                "headers": {"Authorization": "Bearer stream-secret"},
            },
        },
    }
    env_config = {
        "mcpServers": {
            "Env Server": {
                "type": "streamable_http",
                "url": "https://mcp.example.test/env",
                "transport": "http",
            }
        }
    }
    parsed = load_mcp_config(raw_config)
    source_cli = load_mcp_config_from_sources(
        cli_value=json.dumps(raw_config, sort_keys=True),
        env={"OPENAGENT_MCP_CONFIG": json.dumps(env_config, sort_keys=True)},
    )
    source_env = load_mcp_config_from_sources(
        env={"OPENAGENT_MCP_CONFIG": json.dumps(env_config, sort_keys=True)}
    )

    errors: dict[str, str] = {}
    for key, source in {
        "invalid_type": {"mcp": {"bad": {"type": "stdio", "url": "https://example.test"}}},
        "invalid_transport": {"mcp": {"bad": {"url": "https://example.test", "transport": "websocket"}}},
        "invalid_headers": {"mcp": {"bad": {"url": "https://example.test", "headers": ["nope"]}}},
        "invalid_tools": {"mcp": {"bad": {"url": "https://example.test", "tools": ["nope"]}}},
    }.items():
        try:
            load_mcp_config(source)
        except ValueError as error:
            errors[key] = str(error)
        else:
            raise AssertionError(f"{key} fixture did not fail")

    primary_server = parsed.servers[0]
    raw_tools = [
        SimpleNamespace(
            name="Weather.Search",
            title="Weather Search",
            description="Find a forecast for a city.",
            inputSchema={
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            },
            annotations={"readOnlyHint": True},
            execution=SimpleNamespace(read_only=True),
        ),
        SimpleNamespace(
            name="weather search",
            title=None,
            description="Duplicate sanitized name.",
            inputSchema=None,
            annotations=None,
            execution=None,
        ),
        SimpleNamespace(
            name="Data-List",
            title="Data List",
            description="Schema starts as an array and must be wrapped.",
            inputSchema={"type": "array", "items": {"type": "string"}},
            annotations={"dangerous": False},
            execution={"external_io": True},
        ),
        SimpleNamespace(
            name="Data-secret",
            title="Denied",
            description="This tool is denied by filter.",
            inputSchema={"type": "object"},
            annotations={},
            execution=None,
        ),
        SimpleNamespace(
            name="",
            title="Empty",
            description="This tool is ignored because it has no name.",
            inputSchema={"type": "object"},
            annotations={},
            execution=None,
        ),
    ]
    descriptors = _build_tool_descriptors(primary_server, raw_tools)
    manager = RemoteMcpManager(McpConfig(servers=(primary_server,)))
    state = manager._servers[primary_server.name]
    state.status = "ready"
    state.selected_transport = "http"
    state.last_error = None
    state.last_refreshed_at = 1781840000.25
    state.tools_by_dynamic_name = {descriptor.dynamic_name: descriptor for descriptor in descriptors}

    text_result = SimpleNamespace(
        content=[
            TextContent(type="text", text="Weather summary\nCloudy with light wind."),
            SimpleNamespace(type="image"),
            SimpleNamespace(type="image"),
            SimpleNamespace(type="resource"),
            SimpleNamespace(type="blob"),
            SimpleNamespace(type="weird"),
        ],
        structuredContent={"city": "Shanghai", "temperature": 24},
        isError=False,
    )
    empty_result = SimpleNamespace(content=[], structuredContent={"only": "structured"}, isError=False)
    error_result = SimpleNamespace(
        content=[TextContent(type="text", text="Remote MCP rejected the request.")],
        structuredContent={"debug": "ignored"},
        isError=True,
    )
    descriptor = descriptors[0]
    normalized_text = _mcp_call_result_fixture(descriptor, text_result, transport="http")
    normalized_empty = _mcp_call_result_fixture(descriptor, empty_result, transport="http")
    normalized_error = _mcp_call_result_fixture(descriptor, error_result, transport="sse")
    unavailable = asyncio.run(RemoteMcpManager(McpConfig()).call_tool("mcp_tool_missing", {}))

    registry = ToolRegistry()
    bridge_client = _FixtureMcpBridgeClient(
        [descriptor],
        RemoteMcpToolCallResult(
            output="Bridge output",
            metadata={"mcp_transport": "sse", "custom": "kept"},
        ),
    )
    register_mcp_tools(registry, bridge_client, group="remote-mcp")  # type: ignore[arg-type]
    bridge_tool = registry.get(descriptor.dynamic_name)
    if bridge_tool is None:
        raise AssertionError("MCP bridge did not register the fixture tool")
    bridge_output = asyncio.run(_execute_fixture_tool(bridge_tool, {"city": "Shanghai"}))

    auth_payload = {
        "headers": {
            "Authorization": "Bearer secret-token",
            "X-Token": "token-secret",
            "X-Client": "openagent-fixture",
        },
        "api_key": "secret-api-key",
        "nested": {
            "session_token": "secret-session-token",
            "input_tokens": 123,
            "prompt": "visible",
        },
    }

    return {
        "schema_version": 1,
        "config": {
            "parsed": parsed,
            "enabled": parsed.enabled,
            "source_cli": source_cli,
            "source_env": source_env,
            "source_empty": load_mcp_config_from_sources(env={}),
            "errors": errors,
        },
        "discovery": {
            "descriptors": descriptors,
            "listed": manager.list_tool_descriptors(),
            "snapshot": manager.snapshot().to_dict(),
            "helpers": {
                "dynamic_name": _dynamic_tool_name("Demo Server", "Weather.Search"),
                "transport_auto": _transport_candidates("auto"),
                "transport_http": _transport_candidates("http"),
                "transport_sse": _transport_candidates("sse"),
                "timeout_floor": _timeout_seconds(500),
                "timeout_regular": _timeout_seconds(45000),
                "tool_allowed_weather": _tool_allowed("Weather.Search", primary_server.tools),
                "tool_allowed_denied": _tool_allowed("Data-secret", primary_server.tools),
            },
        },
        "tool_call": {
            "text_non_text": normalized_text,
            "empty": normalized_empty,
            "error": normalized_error,
            "unavailable": unavailable,
        },
        "bridge": {
            "definition": {
                "id": bridge_tool.id,
                "description": bridge_tool.description,
                "parameters_schema": bridge_tool.parameters_schema(),
                "dangerous": bridge_tool.dangerous,
                "group": bridge_tool.group,
                "execution_scope": bridge_tool.execution_scope,
                "execution_schema": bridge_tool.execution_schema.as_dict(),
            },
            "output": bridge_output,
            "calls": bridge_client.calls,
        },
        "redaction": {
            "trace": sanitize_trace_value(auth_payload),
            "observability": sanitize_observation_value(auth_payload),
        },
    }


def _app_bridge_tui_fixture() -> dict[str, Any]:
    fixture_root = Path("/tmp/openagent-rust-rewrite-fixture-goal11")
    shutil.rmtree(fixture_root, ignore_errors=True)
    workspace = fixture_root / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)

    event_types = [
        "step-start",
        "step-finish",
        "text-start",
        "text-delta",
        "text-end",
        "tool-call",
        "tool-result",
        "runtime-warning",
        "patch",
        "question-request",
        "error",
        "unknown",
    ]
    wrapped = stream_event_to_app_event(
        {"type": "tool-call", "name": "ls", "input": {"path": "."}, "call_id": "call_1"},
        sequence=3,
        thread_id="session_1",
        turn_id="turn_1",
    )
    wrapped.created_at_ms = 1781842000003
    lifecycle = lifecycle_event(
        sequence=1,
        method="turn/started",
        thread_id="session_1",
        turn_id="turn_1",
        status="running",
        input="hello",
    )
    lifecycle.created_at_ms = 1781842000001

    global_events = [
        AppEvent(
            sequence=1,
            method="turn/started",
            params={"thread_id": "session_1", "turn_id": "turn_1", "status": "running"},
            created_at_ms=1781842000101,
            global_sequence=1,
        ),
        AppEvent(
            sequence=2,
            method="turn/completed",
            params={"thread_id": "session_1", "turn_id": "turn_1", "status": "completed", "final_answer": "done"},
            created_at_ms=1781842000102,
            global_sequence=2,
        ),
        AppEvent(
            sequence=1,
            method="turn/started",
            params={"thread_id": "session_1", "turn_id": "turn_2", "status": "running"},
            created_at_ms=1781842000103,
            global_sequence=3,
        ),
    ]
    replay_after_query = [
        {"id": str(event.global_sequence), "event": event.method, "data": event.to_dict()}
        for event in global_events
        if (event.global_sequence or 0) > 1
    ]
    replay_after_header = [
        {"id": str(event.global_sequence), "event": event.method, "data": event.to_dict()}
        for event in global_events
        if (event.global_sequence or 0) > 2
    ]

    control_cases = [
        ("/tui/append-prompt", {"text": "hello"}, {"text": "hello"}),
        ("/tui/submit-prompt", {}, {}),
        ("/tui/clear-prompt", {}, {}),
        ("/tui/open-help", {}, {}),
        ("/tui/open-sessions", {}, {}),
        ("/tui/open-themes", {}, {}),
        ("/tui/open-models", {}, {}),
        ("/tui/execute-command", {"command": "status"}, {"command": "status"}),
        (
            "/tui/show-toast",
            {"title": "Hi", "message": "Saved", "variant": "success", "duration": 1.5},
            {"title": "Hi", "message": "Saved", "variant": "success", "duration": 1.5},
        ),
        (
            "/tui/publish",
            {"type": "tui.command.execute", "properties": {"command": "help"}},
            {"type": "tui.command.execute", "properties": {"command": "help"}},
        ),
        ("/tui/select-session", {"sessionID": "session_existing"}, {"sessionID": "session_existing"}),
    ]
    publish_samples: dict[str, Any] = {}
    for name, payload in {
        "append": {"type": "tui.prompt.append", "properties": {"text": "hello"}},
        "command": {"topic": "tui.command.execute", "payload": {"command": "status"}},
        "toast": {"event": "tui.toast.show", "payload": {"title": "Saved", "message": "Done", "variant": "success", "duration": 1.25}},
        "session": {"method": "tui.session.select", "payload": {"sessionID": "session_existing"}},
    }.items():
        action, params = _publish_to_control(payload)
        publish_samples[name] = {"action": action, "params": params}
    try:
        _publish_to_control({"type": "tui.unknown", "properties": {}})
    except ValueError as error:
        unsupported_publish = str(error)
    else:
        raise AssertionError("unsupported publish fixture did not fail")
    try:
        _parse_turn_approval_path("/api/turns//approvals/")
    except ValueError as error:
        invalid_approval_path = str(error)
    else:
        raise AssertionError("invalid approval path fixture did not fail")

    turn = TurnRecord(
        id="turn_1",
        session_id="session_1",
        input="hello",
        created_at_ms=1781842000200,
        status="running",
    )
    interrupt_event = turn.request_interrupt()
    assert interrupt_event is not None
    interrupt_event.created_at_ms = 1781842000201

    requested_approval = lifecycle_event(
        sequence=2,
        method="turn/approval_requested",
        thread_id="session_1",
        turn_id="turn_1",
        status="waiting_approval",
        approval={
            "request_id": "approval_1",
            "session_id": "session_1",
            "turn_id": "turn_1",
            "tool_name": "write",
            "tool_input": {"file_path": "blocked.txt"},
            "call_id": "call_1",
            "created_at_ms": 1781842000202,
        },
    )
    requested_approval.created_at_ms = 1781842000202
    resolved_approval = lifecycle_event(
        sequence=3,
        method="turn/approval_resolved",
        thread_id="session_1",
        turn_id="turn_1",
        status="running",
        approval={
            "request_id": "approval_1",
            "session_id": "session_1",
            "turn_id": "turn_1",
            "tool_name": "write",
            "tool_input": {"file_path": "blocked.txt"},
            "call_id": "call_1",
            "created_at_ms": 1781842000202,
            "action": "deny",
        },
    )
    resolved_approval.created_at_ms = 1781842000203

    parsed_event = _app_event_from_dict(
        {
            "sequence": 4,
            "global_sequence": 12,
            "method": "turn/completed",
            "params": {
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "completed",
                "final_answer": "hello remote",
                "trace": {"id": "trace_1"},
            },
            "created_at_ms": 1781842000304,
        },
        default_sequence=99,
    )
    remote_turn = RemoteTurnRecord(id="turn_remote", session_id="session_existing")
    remote_events = [
        AppEvent(
            sequence=1,
            global_sequence=10,
            method="turn/started",
            params={"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"},
            created_at_ms=1781842000301,
        ),
        AppEvent(
            sequence=2,
            global_sequence=11,
            method="turn/approval_requested",
            params={
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "waiting_approval",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "tool_name": "write"},
            },
            created_at_ms=1781842000302,
        ),
        AppEvent(
            sequence=3,
            method="turn/approval_resolved",
            params={
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "running",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "action": "deny"},
            },
            created_at_ms=1781842000303,
        ),
        parsed_event,
    ]
    append_results = [remote_turn.append_event(event) for event in remote_events]
    duplicate_result = remote_turn.append_event(
        AppEvent(
            sequence=1,
            global_sequence=10,
            method="turn/started",
            params={"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"},
            created_at_ms=1781842000301,
        )
    )

    class _FixtureTuiRuntime:
        def __init__(self) -> None:
            self.workspace = workspace
            self.sessions = [
                {"id": "session_existing", "status": "ready", "message_count": 2},
                {"id": "session_other", "status": "ready", "message_count": 0},
            ]

        def start_session(self):
            return {"id": "session_new", "status": "ready", "message_count": 0}

        def list_sessions(self):
            return list(self.sessions)

        def resume_session(self, session_id: str):
            return {"id": session_id, "status": "ready", "message_count": 0, "messages": []}

        def get_session(self, session_id: str):
            return {
                "id": session_id,
                "status": "ready",
                "message_count": 2,
                "messages": [
                    {"role": "user", "content": "old question"},
                    {"role": "assistant", "content": "old answer"},
                ],
            }

    state = TuiState(runtime=_FixtureTuiRuntime())
    tui_steps: list[dict[str, Any]] = []
    for request in [
        {"path": "/tui/append-prompt", "body": {"text": "hello"}},
        {"path": "/tui/publish", "body": {"type": "tui.prompt.append", "properties": {"text": " next"}}},
        {"path": "/tui/show-toast", "body": {"title": "Saved", "message": "Session selected", "variant": "success"}},
        {"path": "/tui/execute-command", "body": {"command": "help"}},
        {"path": "/tui/open-themes", "body": {}},
        {"path": "/tui/clear-prompt", "body": {}},
    ]:
        result = state.apply_control_request(request)
        tui_steps.append(
            {
                "request": request,
                "result": result,
                "status": state.status,
                "input_buffer": state.input_buffer,
                "timeline": [asdict(line) for line in state.timeline],
            }
        )
    invalid_state = TuiState(runtime=_FixtureTuiRuntime())
    invalid_select = invalid_state.apply_control_request({"path": "/tui/select-session", "body": {}})

    payload = {
        "schema_version": 1,
        "protocol": {
            "method_map": {event_type: stream_event_to_app_method(event_type) for event_type in event_types},
            "wrapped_tool_call": wrapped.to_dict(),
            "lifecycle_started": lifecycle.to_dict(),
            "tui_control_request": TuiControlRequest(path="/tui/append-prompt", body={"text": "hello"}).to_dict(),
        },
        "server": {
            "health": {"ok": True, "service": "openagent-app-server", "ui_enabled": False, "auth_required": True},
            "auth": {
                "authenticated_paths": {
                    "/api/health": _is_authenticated_app_path("/api/health"),
                    "/tui/append-prompt": _is_authenticated_app_path("/tui/append-prompt"),
                    "/": _is_authenticated_app_path("/"),
                },
                "expected_header": "Bearer server-secret",
                "unauthorized": {
                    "status": 401,
                    "headers": {"WWW-Authenticate": 'Bearer realm="openagent-app-bridge"'},
                    "json": {"error": "unauthorized"},
                },
            },
            "sse": {
                "replay_after_query_sequence_1": replay_after_query,
                "replay_after_last_event_id_2": replay_after_header,
                "ping_comment": ": ping\n\n",
            },
            "approval_path": {
                "valid": list(_parse_turn_approval_path("/api/turns/turn_123/approvals/approval_456")),
                "invalid_error": invalid_approval_path,
            },
            "control_routes": {
                "cases": [
                    {"path": path, "payload": body, "queued": TuiControlRequest(path=path, body=expected).to_dict()}
                    for path, body, expected in control_cases
                ],
                "publish_samples": publish_samples,
                "unsupported_publish_error": unsupported_publish,
                "empty_next": {"path": "", "body": None},
                "record_response": {"ok": True, "response": ["ok", {"applied": True}]},
            },
            "runtime": {
                "interrupt_event": interrupt_event.to_dict(),
                "turn_after_interrupt": turn.to_dict(),
                "approval_requested": requested_approval.to_dict(),
                "approval_resolved": resolved_approval.to_dict(),
            },
        },
        "client": {
            "helpers": {
                "normalize": normalize_server_url("http://127.0.0.1:8787/"),
                "join": join_server_url("http://127.0.0.1:8787/", "/api/sessions"),
                "quote": quote_path("turn/a b"),
                "auth_header": "Bearer secret",
            },
            "parsed_event": parsed_event.to_dict(),
            "event_ids": {
                "turn": _event_turn_id(parsed_event),
                "session": _event_session_id(parsed_event),
                "key": list(_remote_event_key(parsed_event, default_turn_id="turn_remote")),
            },
            "remote_turn": {
                "append_results": append_results,
                "duplicate_result": duplicate_result,
                "status": remote_turn.status,
                "final_answer": remote_turn.final_answer,
                "trace": remote_turn.trace,
                "events": [event.to_dict() for event in remote_turn.events],
            },
            "request_shapes": {
                "start_session": {"method": "POST", "path": "/api/sessions", "payload": {"cwd": workspace.as_posix()}},
                "start_turn": {"method": "POST", "path": "/api/sessions/session_existing/turns", "payload": {"input": "hello"}},
                "interrupt": {"method": "POST", "path": "/api/turns/turn_remote/interrupt", "payload": {}},
                "approval": {"method": "POST", "path": "/api/turns/turn_remote/approvals/approval_1", "payload": {"action": "deny"}},
                "control_next": {"method": "GET", "path": "/tui/control/next?timeout=0.25"},
                "control_response": {"method": "POST", "path": "/tui/control/response", "payload": {"ok": True, "result": {"applied": True}}},
            },
        },
        "tui": {
            "action_map": {
                name: _normalize_control_action(name)
                for name in [
                    "append-prompt",
                    "submit-prompt",
                    "clear-prompt",
                    "open-help",
                    "open-sessions",
                    "open-themes",
                    "open-models",
                    "select-session",
                    "show-toast",
                    "execute-command",
                    "custom.action",
                ]
            },
            "steps": tui_steps,
            "invalid_select": {
                "result": invalid_select,
                "status": invalid_state.status,
                "timeline": [asdict(line) for line in invalid_state.timeline],
            },
        },
    }
    return _scrub_fixture_root(payload, fixture_root)


def _http_runtime_fixture() -> dict[str, Any]:
    fixture_root = Path("/tmp/openagent-rust-rewrite-fixture-goal12")
    shutil.rmtree(fixture_root, ignore_errors=True)
    workspace = fixture_root / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)
    attached_file = workspace / "notes.txt"
    attached_file.write_text("alpha\nbeta\n", encoding="utf-8")

    parser = build_parser()
    serve_calls: list[dict[str, Any]] = []

    def _fake_serve(**kwargs: Any) -> None:
        serve_calls.append(kwargs)

    serve_args = parser.parse_args(
        [
            "serve",
            "--host",
            "0.0.0.0",
            "--port",
            "8787",
            "--workspace",
            str(workspace),
            "--session-root",
            str(workspace / ".openagent" / "sessions"),
            "--headless",
            "--auth-token",
            "server-secret",
        ]
    )
    run_serve(serve_args, serve_fn=_fake_serve)

    class _FixtureStdin:
        def __init__(self, text: str, *, is_tty: bool) -> None:
            self.text = text
            self._is_tty = is_tty

        def isatty(self) -> bool:
            return self._is_tty

        def read(self) -> str:
            return self.text

    message_text = command_text_from_args(SimpleNamespace(message=["hello", "runtime"]), stdin=_FixtureStdin("", is_tty=True))
    stdin_text = command_text_from_args(SimpleNamespace(message=[]), stdin=_FixtureStdin("from stdin\n", is_tty=False))
    empty_tty_text = command_text_from_args(SimpleNamespace(message=[]), stdin=_FixtureStdin("", is_tty=True))
    prompt_with_file = build_run_prompt("summarize", files=["notes.txt"], workspace=workspace)

    events = [
        {
            "sequence": 1,
            "method": "item/agentMessage/delta",
            "params": {"event": {"text": "hello from server"}},
        },
        {
            "sequence": 2,
            "method": "turn/completed",
            "params": {"status": "completed", "final_answer": "hello from server"},
        },
    ]
    sse_lines = [
        b": ping\n",
        b"\n",
        b"id: 1\n",
        b"event: item/agentMessage/delta\n",
        b'data: {"sequence": 1, "method": "item/agentMessage/delta", "params": {"event": {"text": "hello from server"}}}\n',
        b"\n",
        b"id: 2\n",
        b"event: turn/completed\n",
        b'data: {"sequence": 2, "method": "turn/completed", "params": {"status": "completed", "final_answer": "hello from server"}}\n',
        b"\n",
    ]
    parsed_sse = list(parse_sse_response(sse_lines))

    text_stdout = io.StringIO()
    text_stderr = io.StringIO()
    json_stdout = io.StringIO()
    json_stderr = io.StringIO()
    with patch("openagent.cli.main.stream_app_bridge_events", return_value=iter(events)):
        text_exit = emit_app_bridge_events(
            "http://app.test",
            "turn_1",
            output_format="text",
            verbose=True,
            stdout=text_stdout,
            stderr=text_stderr,
            auth_token="server-secret",
        )
    with patch("openagent.cli.main.stream_app_bridge_events", return_value=iter(events)):
        json_exit = emit_app_bridge_events(
            "http://app.test",
            "turn_1",
            output_format="json",
            verbose=False,
            stdout=json_stdout,
            stderr=json_stderr,
            auth_token=None,
        )

    http_error = urllib.error.HTTPError(
        "http://app.test/api/health",
        401,
        "Unauthorized",
        hdrs={},
        fp=io.BytesIO(b'{"error":"unauthorized"}'),
    )

    select_records: list[dict[str, Any]] = []

    def _fake_get(server_url: str, path: str, *, auth_token: str | None = None) -> dict[str, Any]:
        select_records.append({"method": "GET", "server_url": server_url, "path": path, "auth_token": auth_token})
        if path == "/api/sessions":
            return {"sessions": [{"id": "session_latest"}]}
        return {"session": {"id": path.rsplit("/", 1)[-1]}}

    def _fake_post(server_url: str, path: str, payload: dict[str, object], *, auth_token: str | None = None) -> dict[str, Any]:
        select_records.append({"method": "POST", "server_url": server_url, "path": path, "payload": payload, "auth_token": auth_token})
        return {"session": {"id": "session_new"}}

    with (
        patch("openagent.cli.main.app_bridge_get_json", new=_fake_get),
        patch("openagent.cli.main.app_bridge_post_json", new=_fake_post),
    ):
        explicit_session = select_client_session(
            "http://app.test",
            SimpleNamespace(session="session_existing", continue_last=False),
            workspace=workspace,
            auth_token="server-secret",
        )
        continue_session = select_client_session(
            "http://app.test",
            SimpleNamespace(session=None, continue_last=True),
            workspace=workspace,
            auth_token="server-secret",
        )
        new_session = select_client_session(
            "http://app.test",
            SimpleNamespace(session=None, continue_last=False),
            workspace=workspace,
            auth_token="server-secret",
        )

    dockerfile_lines = [
        "FROM rust:1.85-bookworm AS builder",
        "WORKDIR /app",
        "COPY . .",
        "RUN cargo build --release -p openagent-http-runtime",
        "FROM debian:bookworm-slim",
        "COPY --from=builder /app/target/release/openagent-http-runtime /usr/local/bin/openagent-http-runtime",
        "EXPOSE 8787",
        'HEALTHCHECK CMD ["openagent-http-runtime", "--health-json"]',
        'ENTRYPOINT ["openagent-http-runtime"]',
        'CMD ["--host", "0.0.0.0", "--port", "8787", "--headless"]',
    ]

    payload = {
        "schema_version": 1,
        "sdk": {
            "http_runtime_exports": list(sdk_http_runtime.__all__),
        },
        "serve": {
            "args": {
                "host": serve_args.host,
                "port": serve_args.port,
                "workspace": str(workspace),
                "session_root": str(workspace / ".openagent" / "sessions"),
                "headless": serve_args.headless,
            },
            "call": serve_calls[0],
        },
        "prompt": {
            "message_text": message_text,
            "stdin_text": stdin_text,
            "empty_tty_text": empty_tty_text,
            "with_file": prompt_with_file,
        },
        "client": {
            "select_sessions": {
                "records": select_records,
                "explicit": explicit_session,
                "continue": continue_session,
                "new": new_session,
            },
            "sse_parse": parsed_sse,
            "emit_text": {
                "exit_code": text_exit,
                "stdout": text_stdout.getvalue(),
                "stderr": text_stderr.getvalue(),
            },
            "emit_json": {
                "exit_code": json_exit,
                "stdout_lines": json_stdout.getvalue().splitlines(),
                "stderr": json_stderr.getvalue(),
            },
            "http_error": format_http_error("GET", "/api/health", http_error),
        },
        "runtime": {
            "config": {
                "host": "0.0.0.0",
                "port": 8787,
                "serve_static": False,
                "workspace": workspace.as_posix(),
                "session_store_root": (workspace / ".openagent" / "sessions").as_posix(),
                "auth_required": True,
            },
            "health": {
                "ok": True,
                "service": "openagent-http-runtime",
                "app_bridge": "openagent-app-server",
                "ui_enabled": False,
                "auth_required": True,
            },
            "routes": {
                "health": {"status": 200, "content_type": "application/json; charset=utf-8"},
                "unauthorized": {"status": 401, "body": {"error": "unauthorized"}},
                "options": {"status": 204, "headers": {"Access-Control-Allow-Methods": "GET, POST, OPTIONS"}},
                "unknown": {"status": 404, "body": {"error": "unknown endpoint"}},
            },
        },
        "docker": {
            "dockerfile": dockerfile_lines,
            "smoke_command": ["docker", "run", "--rm", "openagent-http-runtime:goal12", "--health-json"],
            "expected_stdout_json": {
                "ok": True,
                "service": "openagent-http-runtime",
                "app_bridge": "openagent-app-server",
                "ui_enabled": False,
                "auth_required": False,
            },
            "daemon_required": True,
        },
    }
    return _scrub_fixture_root(payload, fixture_root)


def _cli_commands_fixture() -> dict[str, Any]:
    fixture_root = Path("/tmp/openagent-rust-rewrite-fixture-goal10")
    shutil.rmtree(fixture_root, ignore_errors=True)
    workspace = fixture_root / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)
    commands_dir = workspace / ".openagent" / "commands"
    commands_dir.mkdir(parents=True, exist_ok=True)
    (workspace / "notes.txt").write_text("Alpha note\nBeta note\n", encoding="utf-8")
    (commands_dir / "review.md").write_text(
        "\n".join(
            [
                "---",
                "description: Review a target file.",
                "agent: reviewer",
                "model: gpt-command",
                "---",
                "Review $1 with all args: $ARGUMENTS.",
                "",
                "@notes.txt",
                "",
            ]
        ),
        encoding="utf-8",
    )

    parser = build_parser()
    parser_cases: dict[str, Any] = {}
    for name, argv, keys in [
        ("default", [], ["command", "base_url", "model", "wire_api", "max_steps", "workspace", "skip_doctor"]),
        ("doctor_json", ["doctor", "--format", "json"], ["command", "format", "base_url", "model"]),
        (
            "run_json",
            ["run", "--workspace", str(workspace), "--skip-doctor", "--format", "json", "hello", "world"],
            ["command", "workspace", "skip_doctor", "format", "message"],
        ),
        (
            "mcp_add",
            ["mcp", "add", "demo", "--config", str(fixture_root / "mcp.json"), "--url", "https://example.com/mcp"],
            ["command", "mcp_command", "name", "url", "transport", "timeout_ms", "format"],
        ),
    ]:
        args = parser.parse_args(argv)
        parser_cases[name] = {"argv": argv, "namespace": _namespace_subset(args, keys)}

    with patch.dict(os.environ, {}, clear=True):
        default_args = parser.parse_args([])
        apply_model_env(default_args)
        default_env = {
            key: os.environ.get(key)
            for key in ("OPENAI_BASE_URL", "OPENAI_MODEL", "OPENAI_WIRE_API", "OPENAGENT_APP_MAX_STEPS")
        }

    with patch.dict(os.environ, {"OPENAI_MODEL": "env-model"}, clear=True):
        override_args = parser.parse_args(
            [
                "tui",
                "--base-url",
                "http://127.0.0.1:9999",
                "--model",
                "gpt-test",
                "--wire-api",
                "chat",
                "--max-steps",
                "8",
            ]
        )
        apply_model_env(override_args)
        override_env = {
            key: os.environ.get(key)
            for key in ("OPENAI_BASE_URL", "OPENAI_MODEL", "OPENAI_WIRE_API", "OPENAGENT_APP_MAX_STEPS")
        }

    doctor_text_stdout = io.StringIO()
    with patch.dict(
        os.environ,
        {
            "OPENAI_BASE_URL": "http://gateway.test",
            "OPENAI_MODEL": "gpt-test",
            "OPENAI_WIRE_API": "chat",
        },
        clear=True,
    ), patch("openagent.cli.main.check_models_endpoint", return_value=(True, "http://gateway.test/v1/models")):
        doctor_text_code = run_doctor_command(parser.parse_args(["doctor"]), stdout=doctor_text_stdout)

    doctor_json_stdout = io.StringIO()
    with patch.dict(
        os.environ,
        {
            "OPENAI_API_KEY": "private-key",
            "OPENAI_BASE_URL": "http://gateway.test",
            "OPENAI_MODEL": "gpt-test",
            "OPENAI_WIRE_API": "responses",
        },
        clear=True,
    ), patch("openagent.cli.main.check_models_endpoint", return_value=(False, "connection refused")):
        doctor_json_code = run_doctor_command(parser.parse_args(["doctor", "--format", "json"]), stdout=doctor_json_stdout)

    doctor_anthropic_stdout = io.StringIO()
    with patch.dict(
        os.environ,
        {
            "OPENAGENT_PROVIDER": "anthropic",
            "ANTHROPIC_API_KEY": "anthropic-private-key",
            "ANTHROPIC_MODEL": "claude-test",
        },
        clear=True,
    ), patch("openagent.cli.main.check_models_endpoint") as check_endpoint:
        with patch("openagent.cli.main._native_provider_dependency_status", return_value=(True, "optional dependency 'anthropic' is installed")):
            anthropic_args = parser.parse_args(["doctor", "--format", "json"])
            apply_model_env(anthropic_args)
            doctor_anthropic_code = run_doctor_command(anthropic_args, stdout=doctor_anthropic_stdout)
        doctor_anthropic_checked_openai = check_endpoint.called

    auth_file = fixture_root / "auth.json"
    with patch.dict(os.environ, {}, clear=True), patch("time.time", return_value=1781842000.123):
        auth_login = _run_cli_json_fixture(
            run_auth_command,
            parser.parse_args(
                [
                    "auth",
                    "login",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "Groq",
                    "--api-key",
                    "groq-secret",
                    "--model",
                    "llama-fixture",
                    "--base-url",
                    "https://api.groq.example/v1",
                ]
            ),
        )
    with patch.dict(os.environ, {}, clear=True):
        auth_list = _run_cli_json_fixture(
            run_auth_command,
            parser.parse_args(["providers", "list", "--auth-file", str(auth_file), "--format", "json"]),
        )
        auth_methods = _run_cli_json_fixture(
            run_auth_command,
            parser.parse_args(["auth", "methods", "openrouter", "--format", "json"]),
        )

    command_list = _run_cli_json_fixture(
        run_custom_command,
        parser.parse_args(["command", "list", "--workspace", str(workspace), "--format", "json"]),
    )
    command_show = _run_cli_json_fixture(
        run_custom_command,
        parser.parse_args(["command", "show", "review", "--workspace", str(workspace), "--format", "json"]),
    )
    command_render_text = _run_cli_fixture(
        run_custom_command,
        parser.parse_args(["command", "render", "review", "notes.txt", "carefully", "--workspace", str(workspace), "--no-shell"]),
    )
    command_render_json = _run_cli_json_fixture(
        run_custom_command,
        parser.parse_args(["command", "render", "review", "notes.txt", "carefully", "--workspace", str(workspace), "--no-shell", "--format", "json"]),
    )

    config_args = parser.parse_args(
        [
            "config",
            "init",
            "--workspace",
            str(workspace),
            "--api-key",
            "private-key",
            "--base-url",
            "http://config.test/v1",
            "--model",
            "gpt-config",
            "--wire-api",
            "responses",
            "--max-steps",
            "12",
            "--format",
            "json",
        ]
    )
    config_init = _run_cli_json_fixture(run_config_command, config_args)
    with patch.dict(os.environ, {}, clear=True):
        config_show = _run_cli_json_fixture(
            run_config_command,
            parser.parse_args(
                [
                    "config",
                    "show",
                    "--workspace",
                    str(workspace),
                    "--auth-file",
                    str(auth_file),
                    "--server-token",
                    "server-secret",
                    "--format",
                    "json",
                ]
            ),
        )

    mcp_path = fixture_root / "mcp.json"
    mcp_add = _run_cli_json_fixture(
        run_mcp_command,
        parser.parse_args(
            [
                "mcp",
                "add",
                "demo",
                "--config",
                str(mcp_path),
                "--url",
                "https://client:basic-secret@example.com/mcp?token=url-secret&safe=1",
                "--transport",
                "http",
                "--header",
                "Authorization=Bearer secret-token",
                "--header",
                "X-Team=platform",
                "--timeout-ms",
                "45000",
                "--format",
                "json",
            ]
        ),
    )
    mcp_list_table = _run_cli_fixture(
        run_mcp_command,
        parser.parse_args(["mcp", "list", "--config", str(mcp_path)]),
    )
    mcp_doctor = _run_cli_json_fixture(
        run_mcp_command,
        parser.parse_args(["mcp", "doctor", "--config", str(mcp_path), "--format", "json"]),
    )

    payload = {
        "schema_version": 1,
        "parser": parser_cases,
        "model_env": {"default": default_env, "override": override_env},
        "doctor": {
            "text_ok": {
                "exit_code": doctor_text_code,
                "stdout": doctor_text_stdout.getvalue(),
            },
            "json_failed": {
                "exit_code": doctor_json_code,
                "json": json.loads(doctor_json_stdout.getvalue()),
                "stdout": doctor_json_stdout.getvalue(),
            },
            "anthropic_json": {
                "exit_code": doctor_anthropic_code,
                "json": json.loads(doctor_anthropic_stdout.getvalue()),
                "stdout": doctor_anthropic_stdout.getvalue(),
                "openai_probe_called": doctor_anthropic_checked_openai,
            },
        },
        "auth": {
            "login": auth_login,
            "list": auth_list,
            "methods": auth_methods,
        },
        "custom_commands": {
            "list": command_list,
            "show": command_show,
            "render_text": command_render_text,
            "render_json": command_render_json,
        },
        "config": {
            "init": config_init,
            "show": config_show,
        },
        "mcp_cli": {
            "add": mcp_add,
            "list_table": mcp_list_table,
            "doctor": mcp_doctor,
        },
    }
    scrubbed = _scrub_fixture_root(payload, fixture_root)
    rendered = json.dumps(scrubbed, ensure_ascii=False)
    for secret in (
        "private-key",
        "server-secret",
        "groq-secret",
        "secret-token",
        "basic-secret",
        "url-secret",
        "anthropic-private-key",
    ):
        if secret in rendered:
            raise AssertionError(f"CLI fixture leaked secret: {secret}")
    return scrubbed


def capture(output_dir: Path) -> None:
    fixtures = {
        "core_protocol.json": _core_protocol_fixture(),
        "permission_rulesets.json": _permission_rulesets_fixture(),
        "tool_definition_schema.json": _tool_definition_schema_fixture(),
        "tool_runtime.json": _tool_runtime_fixture(),
        "swarm_protocol.json": _swarm_protocol_fixture(),
        "context_state.json": _context_state_fixture(),
        "core_context_policy.json": _core_context_policy_fixture(),
        "agent_loop.json": _agent_loop_fixture(),
        "provider_adapters.json": _provider_adapters_fixture(),
        "session_trace_observability.json": _session_trace_observability_fixture(),
        "mcp_runtime.json": _mcp_runtime_fixture(),
        "app_bridge_tui.json": _app_bridge_tui_fixture(),
        "http_runtime.json": _http_runtime_fixture(),
        "cli_commands.json": _cli_commands_fixture(),
    }
    for name, payload in fixtures.items():
        _write_json(output_dir, name, payload)
    _write_json(
        output_dir,
        "manifest.json",
        {
            "schema_version": 1,
            "fixture_count": len(fixtures),
            "fixtures": sorted(fixtures),
            "purpose": "Goal 0 Python behavior oracle for the Rust rewrite.",
        },
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Capture deterministic Goal 0 golden fixtures.")
    parser.add_argument("--output", type=Path, default=REPO_ROOT / "tests" / "golden" / "rust_rewrite")
    args = parser.parse_args(argv)
    capture(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
