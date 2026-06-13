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
from swarm import RunnerRegistry, SwarmCoordinatorOptions, SwarmRuntime, build_a2a_registry, load_swarm_config, run_swarm_coordinator
from swarm.state import swarm_run_result_to_dict


class ScriptedOpenAgentModel:
    async def stream(self, **_kwargs: Any):
        yield {
            "type": "text-delta",
            "id": "oa-final",
            "text": "OpenAgent worker: local runtime can inspect workspace context and return a bounded summary.",
        }
        yield {
            "type": "finish",
            "finish_reason": "stop",
            "usage": {"input_tokens": 12, "output_tokens": 9, "cost": 0.0},
        }


async def run_example() -> dict[str, Any]:
    server = _A2AMockServer()
    try:
        config = _load_example_config(a2a_url=server.url("/a2a"))
        workspace = REPO_ROOT / "examples" / "workdir_swarm_mixed"
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
            build_a2a_registry(config),
        ):
            for runner in partial.all():
                registry.register(runner)

        result = await run_swarm_coordinator(
            runtime=SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget),
            task=config.task("mixed_review"),
            options=SwarmCoordinatorOptions(run_id="mixed-openagent-a2a-demo", save_team_handoff=False),
        )
        payload = swarm_run_result_to_dict(result=result.run_result, run_id=result.receipt.run_id)
        payload["trace_event_count"] = len(payload.pop("trace_events", []))
        payload["receipt"] = result.receipt.as_dict()
        payload["a2a_request_count"] = len(server.records)
        return payload
    finally:
        server.close()


def _load_example_config(*, a2a_url: str):
    config_path = Path(__file__).with_suffix(".yaml")
    payload = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    payload["runners"]["a2a_reviewer"]["metadata"]["url"] = a2a_url
    return load_swarm_config(payload)


class _A2AMockServer:
    def __init__(self) -> None:
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), _A2AHandler)
        self.server.records = []  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def records(self) -> list[dict[str, Any]]:
        return self.server.records  # type: ignore[attr-defined]

    def url(self, path: str) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}{path}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)


class _A2AHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        raw_body = self.rfile.read(length)
        payload = json.loads(raw_body.decode("utf-8")) if raw_body else {}
        self.server.records.append({"path": self.path, "payload": payload})  # type: ignore[attr-defined]
        if self.path != "/a2a/message:send":
            self.send_response(404)
            body = b"not found"
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = json.dumps(
            {
                "task": {
                    "id": "a2a-demo-task",
                    "status": {"state": "TASK_STATE_COMPLETED"},
                    "artifacts": [
                        {
                            "name": "a2a-review",
                            "parts": [{"text": "A2A reviewer: remote-compatible runner returned an independent review."}],
                        }
                    ],
                }
            }
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/a2a+json")
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
