#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from dataclasses import asdict, dataclass, field, fields, is_dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Literal


REPO_ROOT = Path(__file__).resolve().parents[2]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from openagent.core.context_state import build_compaction_record, render_work_state
from openagent.core.message_materializer import RUNTIME_OPTION_KEYS, materialize_openai_compatible_payload
from openagent.core.observability import (
    ObservationConfig,
    ObservationEvent,
    TraceRecord,
    input_preview,
    output_stats,
    sanitize_observation_value,
)
from openagent.core.permission.ruleset import PermissionRuleset, ruleset
from openagent.core.runtime_logging import RuntimeLogRecord, RuntimeLoggingConfig
from openagent.core.runtime_warnings import (
    RuntimeWarningConfig,
    RuntimeWarningRecord,
    format_runtime_warning_event,
)
from openagent.core.session.todo import TodoItem
from openagent.core.tool.definition import ToolDefinition, ToolExecutionSchema
from openagent.core.tool.builtin import file as file_tools
from openagent.core.tool.builtin import memory as memory_tools
from openagent.core.tool.builtin import question as question_tools
from openagent.core.tool.builtin import search as search_tools
from openagent.core.tool.builtin import shell as shell_tools
from openagent.core.tool.builtin import todo as todo_tools
from openagent.core.tool.builtin.file import _format_read_output_from_text
from openagent.core.tool.builtin.shell import _blocked_command
from openagent.core.tool.registry import ToolRegistry
from openagent.core.tool.truncation import Truncate
from openagent.core.tool.utils import ensure_within_root
from openagent.core.trace import render_trace_summary
from openagent.core.trace.recorder import sanitize_trace_value
from openagent.core.trace.schema import RunRecord, TraceConfig, TraceEvent
from openagent.core.types import (
    ChatMessage,
    Model,
    ModelCapabilities,
    ModelPricing,
    ToolCall,
    ToolResult,
    ToolSchema,
    Usage,
)
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


def capture(output_dir: Path) -> None:
    fixtures = {
        "core_protocol.json": _core_protocol_fixture(),
        "permission_rulesets.json": _permission_rulesets_fixture(),
        "tool_definition_schema.json": _tool_definition_schema_fixture(),
        "tool_runtime.json": _tool_runtime_fixture(),
        "swarm_protocol.json": _swarm_protocol_fixture(),
        "context_state.json": _context_state_fixture(),
        "session_trace_observability.json": _session_trace_observability_fixture(),
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
