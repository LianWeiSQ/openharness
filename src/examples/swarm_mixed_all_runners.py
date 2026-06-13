from __future__ import annotations

import asyncio
import json
from pathlib import Path
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import yaml


def _find_repo_root() -> Path:
    here = Path(__file__).resolve()
    for parent in here.parents:
        if (parent / "pyproject.toml").exists() and (parent / "src" / "openagent").is_dir():
            return parent
    return here.parents[3]


REPO_ROOT = _find_repo_root()
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from openagent.core.types import Model
from openagent.integrations.swarm import build_openagent_registry
from swarm import (
    RunnerRegistry,
    SwarmCoordinatorOptions,
    SwarmRuntime,
    build_a2a_registry,
    build_http_registry,
    build_subprocess_registry,
    load_swarm_config,
    run_swarm_coordinator,
)
from swarm.state import swarm_run_result_to_dict


class ScriptedOpenAgentModel:
    async def stream(self, **_kwargs: Any):
        yield {
            "type": "text-delta",
            "id": "oa-final",
            "text": "OpenAgent worker: inspected the local OpenAgent adapter path.",
        }
        yield {
            "type": "finish",
            "finish_reason": "stop",
            "usage": {"input_tokens": 12, "output_tokens": 8, "cost": 0.0},
        }


async def run_example() -> dict[str, Any]:
    server = _MixedMockServer()
    try:
        config = _load_example_config(http_url=server.url("/http-agent"), a2a_url=server.url("/a2a"))
        workspace = REPO_ROOT / "examples" / "workdir_swarm_mixed_all"
        workspace.mkdir(parents=True, exist_ok=True)

        registry = RunnerRegistry()
        for partial in (
            build_openagent_registry(
                config,
                model=ScriptedOpenAgentModel(),
                model_metadata=Model(
                    id="scripted-openagent",
                    provider_id="local",
                    name="Scripted OpenAgent",
                    context_window=32768,
                    max_output=256,
                ),
                workspace_root=workspace,
            ),
            build_subprocess_registry(config),
            build_http_registry(config),
            build_a2a_registry(config),
        ):
            for runner in partial.all():
                registry.register(runner)

        result = await run_swarm_coordinator(
            runtime=SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget),
            task=config.task("mixed_all_review"),
            options=SwarmCoordinatorOptions(run_id="mixed-all-runners-demo", save_team_handoff=False),
        )
        payload = swarm_run_result_to_dict(result=result.run_result, run_id=result.receipt.run_id)
        payload["trace_event_count"] = len(payload.pop("trace_events", []))
        payload["receipt"] = result.receipt.as_dict()
        payload["mock_request_counts"] = server.request_counts()
        payload["runner_kinds"] = {runner.descriptor.id: runner.descriptor.kind for runner in registry.all()}
        return _public_example_payload(payload)
    finally:
        server.close()


def _load_example_config(*, http_url: str, a2a_url: str):
    config_path = Path(__file__).with_suffix(".yaml")
    payload = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    payload["runners"]["http_planner"]["metadata"]["url"] = http_url
    payload["runners"]["a2a_reviewer"]["metadata"]["url"] = a2a_url
    payload["runners"]["subprocess_checker"]["metadata"]["command"] = [
        sys.executable,
        str(Path(__file__).with_name("swarm_subprocess_worker.py")),
    ]
    payload["runners"]["subprocess_checker"]["metadata"]["cwd"] = str(REPO_ROOT)
    return load_swarm_config(payload)


def _public_example_payload(payload: dict[str, Any]) -> dict[str, Any]:
    results = payload.get("results")
    if isinstance(results, dict):
        for result in results.values():
            if not isinstance(result, dict):
                continue
            metadata = result.get("metadata")
            if not isinstance(metadata, dict):
                continue
            trace = metadata.pop("openagent_trace", None)
            metadata.pop("session_id", None)
            if isinstance(trace, dict):
                if trace.get("trace_id"):
                    metadata["oa_trace_id"] = str(trace["trace_id"])
                if trace.get("run_id"):
                    metadata["oa_run_id"] = str(trace["run_id"])
    return payload


class _MixedMockServer:
    def __init__(self) -> None:
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), _MixedHandler)
        self.server.records = []  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def records(self) -> list[dict[str, Any]]:
        return self.server.records  # type: ignore[attr-defined]

    def request_counts(self) -> dict[str, int]:
        counts: dict[str, int] = {"http": 0, "a2a": 0}
        for record in self.records:
            kind = str(record.get("kind") or "")
            if kind in counts:
                counts[kind] += 1
        return counts

    def url(self, path: str) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}{path}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)


class _MixedHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        payload = self._read_json_body()
        if self.path == "/http-agent":
            self._record("http", payload)
            self._send_json(
                {
                    "status": "completed",
                    "summary": "HTTP worker: planned the remote JSON agent path.",
                    "evidence": ["Received a standard swarm HTTP runner payload."],
                    "confidence": 0.83,
                    "usage": {
                        "input_tokens": 6,
                        "output_tokens": 4,
                        "cost": 0.0,
                        "steps": 1,
                        "latency_ms": 1,
                    },
                    "metadata": {"runner_id": "http_planner"},
                },
                content_type="application/json",
            )
            return
        if self.path == "/a2a/message:send":
            self._record("a2a", payload)
            self._send_json(
                {
                    "task": {
                        "id": "a2a-mixed-demo-task",
                        "status": {"state": "TASK_STATE_COMPLETED"},
                        "artifacts": [
                            {
                                "name": "a2a-review",
                                "parts": [{"text": "A2A reviewer: validated the standard agent-to-agent path."}],
                            }
                        ],
                    }
                },
                content_type="application/a2a+json",
            )
            return
        self._send_text(404, "not found")

    def _read_json_body(self) -> dict[str, Any]:
        length = int(self.headers.get("Content-Length") or 0)
        raw_body = self.rfile.read(length)
        if not raw_body:
            return {}
        decoded = json.loads(raw_body.decode("utf-8"))
        return decoded if isinstance(decoded, dict) else {}

    def _record(self, kind: str, payload: dict[str, Any]) -> None:
        self.server.records.append({"kind": kind, "path": self.path, "payload": payload})  # type: ignore[attr-defined]

    def _send_json(self, payload: dict[str, Any], *, content_type: str) -> None:
        body = json.dumps(payload, ensure_ascii=False, sort_keys=True).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_text(self, status: int, text: str) -> None:
        body = text.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *args: Any) -> None:
        return


async def _main() -> None:
    payload = await run_example()
    print(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(_main())
