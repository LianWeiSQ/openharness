from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
import shlex
import sys
from typing import Any, Sequence

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
    FileSwarmStateStore,
    FileTeamHandoffStore,
    RunnerRegistry,
    SwarmCoordinatorOptions,
    SwarmRuntime,
    build_subprocess_registry,
    load_swarm_config,
    run_swarm_coordinator,
    write_coordinator_receipt,
)
from swarm import cli as swarm_cli
from swarm.state import swarm_run_result_to_dict

DEFAULT_OFFLINE_RUN_ID = "swarm-course-demo-offline"
DEFAULT_REAL_RUN_ID = "swarm-course-demo-real"


class ScriptedCourseOpenAgentModel:
    async def stream(self, **_kwargs: Any):
        yield {
            "type": "text-delta",
            "id": "course-final",
            "text": (
                "Course teacher: swarm mode routes one lesson task to multiple runner types, "
                "keeps src/swarm decoupled, and records a coordinator receipt for review."
            ),
        }
        yield {
            "type": "finish",
            "finish_reason": "stop",
            "usage": {"input_tokens": 18, "output_tokens": 15, "cost": 0.0},
        }


async def run_offline_example(
    *,
    run_id: str = DEFAULT_OFFLINE_RUN_ID,
    persist: bool = False,
    state_dir: str | Path | None = None,
    handoff_dir: str | Path | None = None,
    inspect_host: str = "127.0.0.1",
    inspect_port: int = 8765,
) -> dict[str, Any]:
    config = _load_config_for_local_run()
    workspace = REPO_ROOT / "examples" / "workdir_swarm_course_demo"
    workspace.mkdir(parents=True, exist_ok=True)
    state_root = Path(state_dir).expanduser() if state_dir is not None else workspace / "state"
    handoff_root = Path(handoff_dir).expanduser() if handoff_dir is not None else workspace / "handoff"
    state_store = FileSwarmStateStore(state_root) if persist else None
    handoff_store = FileTeamHandoffStore(handoff_root) if persist else None

    registry = RunnerRegistry()
    for partial in (
        build_openagent_registry(
            config,
            model=ScriptedCourseOpenAgentModel(),
            model_metadata=Model(
                id="scripted-course-openagent",
                provider_id="local",
                name="Scripted Course OpenAgent",
                context_window=32768,
                max_output=256,
            ),
            workspace_root=workspace,
        ),
        build_subprocess_registry(config),
    ):
        for runner in partial.all():
            registry.register(runner)

    result = await run_swarm_coordinator(
        runtime=SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget, state_store=state_store),
        task=config.task("lesson_walkthrough"),
        options=SwarmCoordinatorOptions(run_id=run_id, save_team_handoff=persist),
        team_handoff_store=handoff_store,
    )
    receipt_path: Path | None = None
    if persist:
        receipt_path = write_coordinator_receipt(handoff_root, result.receipt.as_dict())
    payload = swarm_run_result_to_dict(result=result.run_result, run_id=result.receipt.run_id)
    payload["trace_event_count"] = len(payload.pop("trace_events", []))
    payload["receipt"] = result.receipt.as_dict()
    if receipt_path is not None:
        payload["receipt"]["receipt_path"] = str(receipt_path.resolve())
    payload["runner_kinds"] = {runner.descriptor.id: runner.descriptor.kind for runner in registry.all()}
    payload["demo"] = {
        "mode": "offline",
        "purpose": "course walkthrough",
        "real_model_command": display_real_model_command(config_path=Path(__file__).with_suffix(".yaml"), workspace=REPO_ROOT),
        "persistence": {
            "enabled": persist,
            "state_dir": str(state_root.resolve()) if persist else None,
            "handoff_dir": str(handoff_root.resolve()) if persist else None,
            "state_path": str((state_root / run_id / "state.latest.json").resolve()) if persist else None,
            "handoff_path": str((handoff_root / run_id / "team-handoff.json").resolve()) if persist else None,
            "receipt_path": str(receipt_path.resolve()) if receipt_path else None,
        },
        "inspect_command": display_inspect_command(
            state_dir=state_root,
            handoff_dir=handoff_root,
            host=inspect_host,
            port=inspect_port,
        )
        if persist
        else None,
    }
    return _public_example_payload(payload)


def run_real_model_cli(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run the course demo against a real OpenAI-compatible gateway.")
    parser.add_argument("--workspace", default=str(REPO_ROOT), help="Workspace root for OpenAgent runner.")
    parser.add_argument("--run-id", default=DEFAULT_REAL_RUN_ID, help="Stable run id for the real model run.")
    parser.add_argument("--state-dir", default=None, help="Optional swarm state directory for real model mode.")
    parser.add_argument("--handoff-dir", default=None, help="Optional swarm handoff/receipt directory for real model mode.")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output.")
    args = parser.parse_args(list(argv) if argv is not None else None)
    return swarm_cli.main(
        real_model_cli_args(
            config_path=Path(__file__).with_suffix(".yaml"),
            workspace=Path(args.workspace),
            run_id=args.run_id,
            state_dir=Path(args.state_dir) if args.state_dir else None,
            handoff_dir=Path(args.handoff_dir) if args.handoff_dir else None,
            pretty=args.pretty,
        )
    )


def real_model_cli_args(
    *,
    config_path: Path,
    workspace: Path,
    run_id: str = DEFAULT_REAL_RUN_ID,
    state_dir: Path | None = None,
    handoff_dir: Path | None = None,
    pretty: bool = True,
) -> list[str]:
    args = [
        "run",
        str(config_path),
        "--task",
        "lesson_walkthrough",
        "--enable-openagent",
        "--workspace",
        str(workspace),
        "--run-id",
        run_id,
    ]
    if state_dir is not None:
        args.extend(["--state-dir", str(state_dir)])
    if handoff_dir is not None:
        args.extend(["--handoff-dir", str(handoff_dir)])
    if pretty:
        args.append("--pretty")
    return args


def display_real_model_command(
    *,
    config_path: Path,
    workspace: Path,
    run_id: str = DEFAULT_REAL_RUN_ID,
) -> str:
    return _shell_join(["openagent-swarm", *real_model_cli_args(config_path=config_path, workspace=workspace, run_id=run_id, pretty=True)])


def display_inspect_command(
    *,
    state_dir: str | Path,
    handoff_dir: str | Path,
    host: str = "127.0.0.1",
    port: int = 8765,
) -> str:
    return _shell_join(
        [
            "openagent-swarm",
            "inspect",
            "--state-dir",
            str(Path(state_dir).expanduser()),
            "--handoff-dir",
            str(Path(handoff_dir).expanduser()),
            "--host",
            host,
            "--port",
            str(port),
        ]
    )


def _shell_join(parts: Sequence[str]) -> str:
    return " ".join(shlex.quote(str(part)) for part in parts)


def _load_config_for_local_run():
    config_path = Path(__file__).with_suffix(".yaml")
    payload = yaml.safe_load(config_path.read_text(encoding="utf-8"))
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


async def _main() -> None:
    parser = argparse.ArgumentParser(description="OpenAgent swarm course demo.")
    parser.add_argument("--real", action="store_true", help="Use a real OpenAI-compatible gateway through openagent-swarm --enable-openagent.")
    parser.add_argument("--workspace", default=str(REPO_ROOT), help="Workspace root for real model mode.")
    parser.add_argument("--run-id", default=None, help="Run id for real model mode or persisted offline mode.")
    parser.add_argument("--persist", action="store_true", help="Persist offline state, handoff, and coordinator receipt for inspection.")
    parser.add_argument("--state-dir", default=None, help="State directory for --persist or --real.")
    parser.add_argument("--handoff-dir", default=None, help="Handoff/receipt directory for --persist or --real.")
    parser.add_argument("--inspect-host", default="127.0.0.1", help="Host shown in the generated inspect command.")
    parser.add_argument("--inspect-port", type=int, default=8765, help="Port shown in the generated inspect command.")
    args = parser.parse_args()

    if args.real:
        real_args = ["--workspace", args.workspace, "--run-id", args.run_id or DEFAULT_REAL_RUN_ID, "--pretty"]
        if args.state_dir:
            real_args.extend(["--state-dir", args.state_dir])
        if args.handoff_dir:
            real_args.extend(["--handoff-dir", args.handoff_dir])
        raise SystemExit(run_real_model_cli(real_args))

    payload = await run_offline_example(
        run_id=args.run_id or DEFAULT_OFFLINE_RUN_ID,
        persist=args.persist,
        state_dir=args.state_dir,
        handoff_dir=args.handoff_dir,
        inspect_host=args.inspect_host,
        inspect_port=args.inspect_port,
    )
    print(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(_main())
