from __future__ import annotations

"""Command line entrypoint for running swarm YAML configs."""

import argparse
import asyncio
import importlib
import json
import sys
from pathlib import Path
from typing import Any, Sequence
from uuid import uuid4

from .a2a_runner import build_a2a_registry
from .config import SwarmConfig, TaskConfig, load_swarm_config
from .coordinator import SwarmCoordinatorOptions, run_swarm_coordinator
from .http_runner import build_http_registry
from .inspection import SwarmInspectionConfig, serve_inspection_api, write_coordinator_receipt
from .registry import RunnerRegistry
from .runtime import SwarmRunResult, SwarmRuntime
from .state import FileSwarmStateStore, swarm_run_result_to_dict
from .subprocess_runner import build_subprocess_registry
from .team import FileTeamHandoffStore

CONFIG_ONLY_RUNNER_KINDS = {"a2a", "http", "subprocess"}
SUCCESS_STATUSES = {"completed", "partial"}


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(list(argv) if argv is not None else None)
    if args.command == "run":
        return _run_command(args)
    if args.command == "inspect":
        return _inspect_command(args)
    parser.print_help(sys.stderr)
    return 2


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="openagent-swarm", description="Run decoupled swarm YAML configs.")
    subparsers = parser.add_subparsers(dest="command")

    run = subparsers.add_parser("run", help="Run a task from a swarm YAML config.")
    run.add_argument("config", help="Path to a swarm YAML config file.")
    run.add_argument("--task", required=True, help="Task id to run from the config.")
    run.add_argument("--run-id", help="Stable run id. Defaults to a generated swarm_<uuid> id.")
    run.add_argument("--state-dir", help="Optional directory for state.latest.json, runner-results.json, and trace.jsonl.")
    run.add_argument("--handoff-dir", help="Optional directory for team-handoff.json receipts.")
    run.add_argument("--enable-openagent", action="store_true", help="Allow YAML kind=openagent runners using the installed OpenAgent integration.")
    run.add_argument("--workspace", default=".", help="Workspace root for enabled OpenAgent runners. Defaults to current directory.")
    run.add_argument("--model", default=None, help="Model id for enabled OpenAgent runners. Defaults to OPENAGENT_SWARM_MODEL or OPENAI_MODEL.")
    run.add_argument("--wire-api", choices=["chat", "responses"], default=None, help="Wire API for enabled OpenAgent runners.")
    run.add_argument("--context-window", type=int, default=None, help="Context window for enabled OpenAgent runner metadata.")
    run.add_argument("--max-output", type=int, default=None, help="Max output for enabled OpenAgent runner metadata.")
    run.add_argument("--pretty", action="store_true", help="Pretty-print JSON output.")

    inspect = subparsers.add_parser("inspect", help="Serve a local JSON API over persisted swarm runs.")
    inspect.add_argument("--state-dir", help="Directory containing state.latest.json run folders.")
    inspect.add_argument("--handoff-dir", help="Directory containing team-handoff.json and coordinator-receipt.json run folders.")
    inspect.add_argument("--host", default="127.0.0.1", help="Host to bind. Defaults to 127.0.0.1.")
    inspect.add_argument("--port", type=int, default=8765, help="Port to bind. Defaults to 8765.")
    return parser


def _run_command(args: argparse.Namespace) -> int:
    try:
        payload = asyncio.run(_run_from_args(args))
    except Exception as error:  # noqa: BLE001 - CLI errors should be compact and machine-readable.
        print(json.dumps({"status": "error", "error": str(error)}, ensure_ascii=False, sort_keys=True), file=sys.stderr)
        return 2
    output = json.dumps(
        payload,
        ensure_ascii=False,
        indent=2 if args.pretty else None,
        sort_keys=True,
    )
    print(output)
    return 0 if payload.get("status") in SUCCESS_STATUSES else 1


def _inspect_command(args: argparse.Namespace) -> int:
    if not args.state_dir and not args.handoff_dir:
        print(json.dumps({"status": "error", "error": "inspect requires --state-dir or --handoff-dir"}, sort_keys=True), file=sys.stderr)
        return 2
    print(f"Serving swarm inspection UI on http://{args.host}:{args.port}", file=sys.stderr)
    serve_inspection_api(
        SwarmInspectionConfig(state_dir=args.state_dir, handoff_dir=args.handoff_dir),
        host=args.host,
        port=args.port,
    )
    return 0


async def _run_from_args(args: argparse.Namespace) -> dict[str, Any]:
    config_path = Path(args.config)
    config = load_swarm_config(config_path)
    task = config.task(str(args.task))
    registry = await build_cli_registry(config=config, task=task, args=args)
    run_id = str(args.run_id or f"swarm_{uuid4().hex}")
    state_store = FileSwarmStateStore(args.state_dir) if args.state_dir else None
    runtime = SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget, state_store=state_store)

    if args.handoff_dir:
        coordinator = await run_swarm_coordinator(
            runtime=runtime,
            task=task,
            options=SwarmCoordinatorOptions(run_id=run_id, save_team_handoff=True),
            team_handoff_store=FileTeamHandoffStore(args.handoff_dir),
        )
        receipt = coordinator.receipt.as_dict()
        receipt_path = write_coordinator_receipt(args.handoff_dir, receipt)
        return _payload_for_result(
            result=coordinator.run_result,
            run_id=run_id,
            state_dir=args.state_dir,
            receipt={**receipt, "receipt_path": str(receipt_path)},
        )

    result = await runtime.run_task(task, run_id=run_id)
    return _payload_for_result(result=result, run_id=run_id, state_dir=args.state_dir, receipt=None)


async def build_cli_registry(*, config: SwarmConfig, task: TaskConfig, args: argparse.Namespace) -> RunnerRegistry:
    supported_kinds = _supported_runner_kinds(enable_openagent=bool(getattr(args, "enable_openagent", False)))
    unsupported = _unsupported_runner_ids_for_task(config=config, task=task, supported_kinds=supported_kinds)
    if unsupported:
        joined = ", ".join(sorted(unsupported))
        hint = " Pass --enable-openagent to allow kind=openagent runners." if _runner_ids_include_kind(config=config, runner_ids=unsupported, kind="openagent") else ""
        raise ValueError(
            f"task {task.id!r} references runner(s) not supported by the config-only CLI: {joined}. "
            f"Supported CLI runner kinds are: {', '.join(sorted(supported_kinds))}.{hint}"
        )

    selected_config = _selected_config_for_task(config=config, task=task, supported_kinds=supported_kinds)
    registry = RunnerRegistry()
    for partial in (
        build_subprocess_registry(selected_config),
        build_http_registry(selected_config),
        build_a2a_registry(selected_config),
    ):
        for runner in partial.all():
            registry.register(runner)
    if getattr(args, "enable_openagent", False):
        openagent_registry = await _build_openagent_registry_from_cli(config=selected_config, args=args)
        for runner in openagent_registry.all():
            registry.register(runner)
    if not list(registry.ids()):
        raise ValueError(f"no CLI-supported runners are configured; use kind {', '.join(sorted(supported_kinds))}")
    return registry


def _supported_runner_kinds(*, enable_openagent: bool) -> set[str]:
    supported = set(CONFIG_ONLY_RUNNER_KINDS)
    if enable_openagent:
        supported.add("openagent")
    return supported


def _unsupported_runner_ids_for_task(*, config: SwarmConfig, task: TaskConfig, supported_kinds: set[str]) -> list[str]:
    kinds_by_id = {runner.id: runner.kind for runner in config.runners}
    selected = list(task.runner_ids)
    return [runner_id for runner_id in selected if kinds_by_id.get(runner_id) not in supported_kinds]


def _selected_config_for_task(*, config: SwarmConfig, task: TaskConfig, supported_kinds: set[str]) -> SwarmConfig:
    if task.runner_ids:
        selected_ids = set(task.runner_ids)
    else:
        selected_ids = {
            runner.id
            for runner in config.runners
            if runner.kind in supported_kinds and (task.role in runner.roles or "*" in runner.roles)
        }
    return SwarmConfig(
        runners=[runner for runner in config.runners if runner.id in selected_ids],
        tasks=[task],
        fanout_budget=config.fanout_budget,
    )


async def _build_openagent_registry_from_cli(*, config: SwarmConfig, args: argparse.Namespace) -> RunnerRegistry:
    module = importlib.import_module("openagent.integrations.swarm")
    builder = getattr(module, "build_openagent_registry_from_env")
    return await builder(
        config,
        workspace_root=getattr(args, "workspace", "."),
        model_id=getattr(args, "model", None),
        context_window=getattr(args, "context_window", None),
        max_output=getattr(args, "max_output", None),
        wire_api=getattr(args, "wire_api", None),
    )


def _runner_ids_include_kind(*, config: SwarmConfig, runner_ids: list[str], kind: str) -> bool:
    kinds_by_id = {runner.id: runner.kind for runner in config.runners}
    return any(kinds_by_id.get(runner_id) == kind for runner_id in runner_ids)


def _payload_for_result(
    *,
    result: SwarmRunResult,
    run_id: str,
    state_dir: str | None,
    receipt: dict[str, Any] | None,
) -> dict[str, Any]:
    payload = swarm_run_result_to_dict(result=result, run_id=run_id)
    trace_events = payload.pop("trace_events", [])
    payload["trace_event_count"] = len(trace_events)
    if state_dir:
        payload["state_dir"] = str(Path(state_dir).resolve())
    if receipt is not None:
        payload["receipt"] = receipt
    return payload


if __name__ == "__main__":
    raise SystemExit(main())
