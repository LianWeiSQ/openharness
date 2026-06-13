from __future__ import annotations

"""Persistent state store for swarm runs."""

import json
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any, Protocol

from .protocol import AgentResult, ArtifactRef, Usage


class SwarmStateStore(Protocol):
    def save_run(self, result: Any, *, run_id: str) -> dict[str, Any]:
        """Persist a swarm run and return the saved payload."""

    def load_run(self, run_id: str) -> dict[str, Any]:
        """Load a previously persisted swarm run payload."""


class FileSwarmStateStore:
    def __init__(self, root: str | Path) -> None:
        self.root = Path(root).resolve()

    def run_dir(self, run_id: str) -> Path:
        return self.root / _safe_name(run_id)

    def save_run(self, result: Any, *, run_id: str) -> dict[str, Any]:
        payload = swarm_run_result_to_dict(result=result, run_id=run_id)
        run_dir = self.run_dir(run_id)
        run_dir.mkdir(parents=True, exist_ok=True)
        _write_json(run_dir / "state.latest.json", payload)
        _write_json(run_dir / "runner-results.json", payload["results"])
        _write_jsonl(run_dir / "trace.jsonl", payload["trace_events"])
        return payload

    def load_run(self, run_id: str) -> dict[str, Any]:
        with (self.run_dir(run_id) / "state.latest.json").open("r", encoding="utf-8") as handle:
            return json.load(handle)


def swarm_run_result_to_dict(*, result: Any, run_id: str) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "run_id": run_id,
        "task_id": str(getattr(result, "task_id")),
        "status": str(getattr(result, "status")),
        "summary": str(getattr(result, "summary")),
        "usage": _usage_to_dict(getattr(result, "usage", None)),
        "warnings": [str(item) for item in getattr(result, "warnings", [])],
        "results": {
            str(runner_id): _agent_result_to_dict(agent_result)
            for runner_id, agent_result in dict(getattr(result, "results", {}) or {}).items()
        },
        "trace_events": [_trace_event_to_dict(event) for event in getattr(result, "trace_events", [])],
    }


def _agent_result_to_dict(result: AgentResult) -> dict[str, Any]:
    return {
        "status": result.status,
        "summary": result.summary,
        "evidence": list(result.evidence),
        "open_questions": list(result.open_questions),
        "confidence": result.confidence,
        "artifacts": [_artifact_to_dict(artifact) for artifact in result.artifacts],
        "usage": _usage_to_dict(result.usage),
        "metadata": _json_safe(result.metadata),
    }


def _usage_to_dict(usage: Usage | None) -> dict[str, Any]:
    if usage is None:
        usage = Usage()
    return {
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens,
        "cost": usage.cost,
        "steps": usage.steps,
        "latency_ms": usage.latency_ms,
    }


def _artifact_to_dict(artifact: ArtifactRef) -> dict[str, Any]:
    return {
        "kind": artifact.kind,
        "uri": artifact.uri,
        "title": artifact.title,
        "metadata": _json_safe(artifact.metadata),
    }


def _trace_event_to_dict(event: Any) -> dict[str, Any]:
    as_dict = getattr(event, "as_dict", None)
    if callable(as_dict):
        return _json_safe(as_dict())
    if isinstance(event, dict):
        return _json_safe(event)
    if is_dataclass(event):
        return _json_safe(asdict(event))
    return {"event": str(event)}


def _json_safe(value: Any) -> Any:
    try:
        json.dumps(value)
        return value
    except TypeError:
        if isinstance(value, dict):
            return {str(key): _json_safe(item) for key, item in value.items()}
        if isinstance(value, (list, tuple, set)):
            return [_json_safe(item) for item in value]
        if is_dataclass(value):
            return _json_safe(asdict(value))
        return str(value)


def _write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _write_jsonl(path: Path, events: list[dict[str, Any]]) -> None:
    path.write_text("".join(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n" for event in events), encoding="utf-8")


def _safe_name(value: str) -> str:
    cleaned = "".join(char if char.isalnum() or char in {"-", "_"} else "_" for char in value).strip("_")
    return cleaned or "run"


__all__ = ["FileSwarmStateStore", "SwarmStateStore", "swarm_run_result_to_dict"]
