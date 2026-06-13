from __future__ import annotations

"""Local inspection API for persisted swarm runs."""

import json
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlparse

COORDINATOR_RECEIPT_FILE = "coordinator-receipt.json"


@dataclass(frozen=True, slots=True)
class SwarmInspectionConfig:
    state_dir: str | Path | None = None
    handoff_dir: str | Path | None = None

    @property
    def resolved_state_dir(self) -> Path | None:
        return Path(self.state_dir).resolve() if self.state_dir else None

    @property
    def resolved_handoff_dir(self) -> Path | None:
        return Path(self.handoff_dir).resolve() if self.handoff_dir else None


def write_coordinator_receipt(root: str | Path, receipt: dict[str, Any]) -> Path:
    run_id = str(receipt.get("run_id") or "").strip()
    if not run_id:
        raise ValueError("coordinator receipt requires run_id")
    path = Path(root).resolve() / _safe_name(run_id) / COORDINATOR_RECEIPT_FILE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(_json_safe(receipt), ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def load_run_index(config: SwarmInspectionConfig) -> dict[str, Any]:
    state_root = config.resolved_state_dir
    handoff_root = config.resolved_handoff_dir
    run_ids = _discover_run_ids(state_root, handoff_root)
    runs = [_run_summary(run_id, state_root=state_root, handoff_root=handoff_root) for run_id in run_ids]
    diagnostics = [diagnostic for run in runs for diagnostic in run.pop("diagnostics", [])]
    return {
        "schema_version": 1,
        "run_count": len(runs),
        "runs": runs,
        "diagnostics": diagnostics,
    }


def load_run_detail(config: SwarmInspectionConfig, run_id: str) -> dict[str, Any] | None:
    state_root = config.resolved_state_dir
    handoff_root = config.resolved_handoff_dir
    summary = _run_summary(run_id, state_root=state_root, handoff_root=handoff_root)
    if not summary["has_state"] and not summary["has_handoff"] and not summary["has_receipt"]:
        return None
    state, state_error = _read_json(_artifact_path(state_root, run_id, "state.latest.json"))
    handoff, handoff_error = _read_json(_artifact_path(handoff_root, run_id, "team-handoff.json"))
    receipt, receipt_error = _read_json(_artifact_path(handoff_root, run_id, COORDINATOR_RECEIPT_FILE))
    diagnostics = list(summary.pop("diagnostics", []))
    diagnostics.extend(_diagnostics_for_errors(run_id, state_error=state_error, handoff_error=handoff_error, receipt_error=receipt_error))
    return {
        "schema_version": 1,
        "run": summary,
        "state": state,
        "handoff": handoff,
        "receipt": receipt,
        "diagnostics": diagnostics,
    }


def load_run_artifact(config: SwarmInspectionConfig, run_id: str, artifact: str) -> dict[str, Any] | list[Any] | None:
    state_root = config.resolved_state_dir
    handoff_root = config.resolved_handoff_dir
    if artifact == "state":
        payload, _error = _read_json(_artifact_path(state_root, run_id, "state.latest.json"))
        return payload
    if artifact == "handoff":
        payload, _error = _read_json(_artifact_path(handoff_root, run_id, "team-handoff.json"))
        return payload
    if artifact == "receipt":
        payload, _error = _read_json(_artifact_path(handoff_root, run_id, COORDINATOR_RECEIPT_FILE))
        return payload
    if artifact == "trace":
        state, _error = _read_json(_artifact_path(state_root, run_id, "state.latest.json"))
        if isinstance(state, dict):
            return state.get("trace_events") or []
        return None
    raise ValueError(f"unknown swarm inspection artifact: {artifact}")


def create_inspection_server(
    config: SwarmInspectionConfig,
    *,
    host: str = "127.0.0.1",
    port: int = 8765,
) -> ThreadingHTTPServer:
    class Handler(_InspectionHandler):
        inspection_config = config

    return ThreadingHTTPServer((host, port), Handler)


def serve_inspection_api(config: SwarmInspectionConfig, *, host: str = "127.0.0.1", port: int = 8765) -> None:
    server = create_inspection_server(config, host=host, port=port)
    try:
        server.serve_forever()
    finally:
        server.server_close()


class _InspectionHandler(BaseHTTPRequestHandler):
    inspection_config: SwarmInspectionConfig

    def do_GET(self) -> None:  # noqa: N802
        try:
            status, payload = self._route()
        except Exception as error:  # noqa: BLE001
            status, payload = 500, {"status": "error", "error": str(error)}
        self._write_json(status, payload)

    def _route(self) -> tuple[int, dict[str, Any] | list[Any]]:
        parts = _path_parts(self.path)
        if not parts or parts == ["health"]:
            return 200, {"status": "ok", "service": "swarm-inspection"}
        if parts == ["runs"]:
            return 200, load_run_index(self.inspection_config)
        if len(parts) == 2 and parts[0] == "runs":
            detail = load_run_detail(self.inspection_config, parts[1])
            if detail is None:
                return 404, _not_found(parts[1])
            return 200, detail
        if len(parts) == 3 and parts[0] == "runs" and parts[2] in {"state", "handoff", "receipt", "trace"}:
            payload = load_run_artifact(self.inspection_config, parts[1], parts[2])
            if payload is None:
                return 404, _not_found(parts[1], artifact=parts[2])
            return 200, payload
        return 404, {"status": "not_found", "error": "unknown endpoint"}

    def _write_json(self, status: int, payload: dict[str, Any] | list[Any]) -> None:
        body = json.dumps(_json_safe(payload), ensure_ascii=False, sort_keys=True).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *args: Any) -> None:
        return


def _run_summary(run_id: str, *, state_root: Path | None, handoff_root: Path | None) -> dict[str, Any]:
    state_path = _artifact_path(state_root, run_id, "state.latest.json")
    handoff_path = _artifact_path(handoff_root, run_id, "team-handoff.json")
    receipt_path = _artifact_path(handoff_root, run_id, COORDINATOR_RECEIPT_FILE)
    state, state_error = _read_json(state_path)
    handoff, handoff_error = _read_json(handoff_path)
    receipt, receipt_error = _read_json(receipt_path)
    source = receipt if isinstance(receipt, dict) else state if isinstance(state, dict) else handoff if isinstance(handoff, dict) else {}
    diagnostics = _diagnostics_for_errors(run_id, state_error=state_error, handoff_error=handoff_error, receipt_error=receipt_error)
    return {
        "run_id": run_id,
        "task_id": _first_text(source, "task_id"),
        "status": _first_text(source, "run_status", "status"),
        "summary_preview": _preview(_first_text(source, "summary")),
        "usage": source.get("usage") if isinstance(source.get("usage"), dict) else {},
        "runner_count": _runner_count(source),
        "runner_status_counts": _runner_status_counts(source),
        "trace_event_count": _trace_event_count(source),
        "has_state": state_path is not None and state_path.exists(),
        "has_handoff": handoff_path is not None and handoff_path.exists(),
        "has_receipt": receipt_path is not None and receipt_path.exists(),
        "handoff_has_pending": bool(handoff.get("pending_runner_ids")) if isinstance(handoff, dict) else False,
        "pending_runner_ids": [str(item) for item in handoff.get("pending_runner_ids") or []] if isinstance(handoff, dict) else [],
        "reusable_runner_ids": [str(item) for item in handoff.get("reusable_runner_ids") or []] if isinstance(handoff, dict) else [],
        "links": _links_for_run(run_id, state=state_path is not None and state_path.exists(), handoff=handoff_path is not None and handoff_path.exists(), receipt=receipt_path is not None and receipt_path.exists()),
        "diagnostics": diagnostics,
    }


def _discover_run_ids(state_root: Path | None, handoff_root: Path | None) -> list[str]:
    run_ids: set[str] = set()
    for root in (state_root, handoff_root):
        if root is None or not root.exists():
            continue
        for child in root.iterdir():
            if child.is_dir():
                run_ids.add(child.name)
    return sorted(run_ids)


def _artifact_path(root: Path | None, run_id: str, name: str) -> Path | None:
    if root is None:
        return None
    return root / _safe_name(run_id) / name


def _read_json(path: Path | None) -> tuple[dict[str, Any] | list[Any] | None, str | None]:
    if path is None or not path.exists():
        return None, None
    try:
        with path.open("r", encoding="utf-8") as handle:
            payload = json.load(handle)
    except Exception as error:  # noqa: BLE001
        return None, str(error)
    if isinstance(payload, (dict, list)):
        return payload, None
    return None, "artifact payload must be a JSON object or array"


def _diagnostics_for_errors(run_id: str, **errors: str | None) -> list[dict[str, str]]:
    diagnostics: list[dict[str, str]] = []
    for artifact, error in errors.items():
        if error:
            diagnostics.append({"run_id": run_id, "artifact": artifact.replace("_error", ""), "error": error})
    return diagnostics


def _links_for_run(run_id: str, *, state: bool, handoff: bool, receipt: bool) -> dict[str, str]:
    links = {"detail": f"/runs/{run_id}"}
    if state:
        links["state"] = f"/runs/{run_id}/state"
        links["trace"] = f"/runs/{run_id}/trace"
    if handoff:
        links["handoff"] = f"/runs/{run_id}/handoff"
    if receipt:
        links["receipt"] = f"/runs/{run_id}/receipt"
    return links


def _runner_count(source: dict[str, Any]) -> int:
    if isinstance(source.get("runner_count"), int):
        return int(source["runner_count"])
    if isinstance(source.get("results"), dict):
        return len(source["results"])
    if isinstance(source.get("runner_ids"), list):
        return len(source["runner_ids"])
    return 0


def _runner_status_counts(source: dict[str, Any]) -> dict[str, int]:
    if isinstance(source.get("runner_status_counts"), dict):
        return {str(key): int(value) for key, value in source["runner_status_counts"].items()}
    results = source.get("results")
    if isinstance(results, dict):
        counts: dict[str, int] = {}
        for item in results.values():
            status = str(item.get("status") if isinstance(item, dict) else "unknown")
            counts[status] = counts.get(status, 0) + 1
        return dict(sorted(counts.items()))
    runners = source.get("runners")
    if isinstance(runners, list):
        counts = {}
        for item in runners:
            status = str(item.get("status") if isinstance(item, dict) else "unknown")
            counts[status] = counts.get(status, 0) + 1
        return dict(sorted(counts.items()))
    return {}


def _trace_event_count(source: dict[str, Any]) -> int:
    if isinstance(source.get("trace_event_count"), int):
        return int(source["trace_event_count"])
    if isinstance(source.get("trace_events"), list):
        return len(source["trace_events"])
    return 0


def _first_text(source: dict[str, Any], *keys: str) -> str:
    for key in keys:
        value = source.get(key)
        if value is not None:
            return str(value)
    return ""


def _preview(value: str, limit: int = 240) -> str:
    return value if len(value) <= limit else value[: limit - 3].rstrip() + "..."


def _path_parts(path: str) -> list[str]:
    parsed = urlparse(path)
    return [unquote(part) for part in parsed.path.split("/") if part]


def _not_found(run_id: str, *, artifact: str | None = None) -> dict[str, str]:
    if artifact:
        return {"status": "not_found", "run_id": run_id, "artifact": artifact, "error": "artifact not found"}
    return {"status": "not_found", "run_id": run_id, "error": "run not found"}


def _safe_name(value: str) -> str:
    cleaned = "".join(char if char.isalnum() or char in {"-", "_"} else "_" for char in value).strip("_")
    return cleaned or "run"


def _json_safe(value: Any) -> Any:
    try:
        json.dumps(value)
        return value
    except TypeError:
        if isinstance(value, dict):
            return {str(key): _json_safe(item) for key, item in value.items()}
        if isinstance(value, (list, tuple, set)):
            return [_json_safe(item) for item in value]
        return str(value)


__all__ = [
    "COORDINATOR_RECEIPT_FILE",
    "SwarmInspectionConfig",
    "create_inspection_server",
    "load_run_artifact",
    "load_run_detail",
    "load_run_index",
    "serve_inspection_api",
    "write_coordinator_receipt",
]
