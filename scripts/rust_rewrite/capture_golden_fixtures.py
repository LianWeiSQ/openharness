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
from openagent.core.permission.ruleset import PermissionRuleset, ruleset
from openagent.core.tool.definition import ToolDefinition, ToolExecutionSchema
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


def capture(output_dir: Path) -> None:
    fixtures = {
        "core_protocol.json": _core_protocol_fixture(),
        "permission_rulesets.json": _permission_rulesets_fixture(),
        "tool_definition_schema.json": _tool_definition_schema_fixture(),
        "swarm_protocol.json": _swarm_protocol_fixture(),
        "context_state.json": _context_state_fixture(),
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
