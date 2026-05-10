from __future__ import annotations

import copy
import json
import shutil
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

import yaml

from ..agent.universal import UniversalAgent
from ..permission.manager import PermissionManager
from ..session.session import Session
from ..types import AgentConfig, ChatMessage
from ..provider.base import LanguageModel


@dataclass(frozen=True, slots=True)
class EvalCase:
    id: str
    input: str
    workspace: str | None = None
    history: str | list[dict[str, Any]] | None = None
    expected: dict[str, Any] = field(default_factory=dict)
    scoring: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class EvalResult:
    case_id: str
    status: str
    score: float
    duration_ms: int
    steps: int
    tool_calls: int
    input_tokens: int
    output_tokens: int
    cost: float
    error_kind: str | None
    failure_reasons: list[str]
    trace_path: str | None


@dataclass(frozen=True, slots=True)
class EvalRunReport:
    results: list[EvalResult]
    report_path: str
    summary_path: str


async def run_eval_case(
    case: EvalCase,
    *,
    model: LanguageModel,
    base_dir: Path | str,
    output_dir: Path | str,
    agent_config: AgentConfig | None = None,
    system_prompt: str = "Test prompt.",
) -> EvalResult:
    base = Path(base_dir)
    out_dir = Path(output_dir).resolve()
    workdir = _prepare_workspace(case, base_dir=base, output_dir=out_dir)
    before_files = _snapshot_files(workdir)
    session = Session(directory=workdir)
    _load_history(case, session=session, base_dir=base)

    cfg = copy.deepcopy(agent_config) if agent_config is not None else AgentConfig(name="eval", permission="FULL", max_steps=8)
    options = dict(cfg.options or {})
    observability = dict(options.get("observability") or {})
    observability.setdefault("enabled", True)
    observability.setdefault("keep_events", True)
    observability.setdefault("jsonl", True)
    observability.setdefault("jsonl_dir", str(out_dir / "traces"))
    options["observability"] = observability
    cfg.options = options
    agent = UniversalAgent(config=cfg, model=model, system_prompt=system_prompt)
    loop = __import__("openagent.core.loop.processor", fromlist=["AgentLoop"]).AgentLoop(
        agent=agent,
        session=session,
        permission_manager=PermissionManager(),
    )

    started = time.time()
    events: list[dict[str, Any]] = []
    async for event in loop.run(case.input):
        events.append(dict(event))
    duration_ms = int((time.time() - started) * 1000)
    after_files = _snapshot_files(workdir)
    trace_path = _trace_path(session)
    return _score_case(
        case,
        session=session,
        events=events,
        before_files=before_files,
        after_files=after_files,
        duration_ms=duration_ms,
        trace_path=trace_path,
    )


async def run_eval_files(
    paths: list[str | Path],
    *,
    model: LanguageModel,
    base_dir: Path | str,
    output_dir: Path | str,
    agent_config: AgentConfig | None = None,
    system_prompt: str = "Test prompt.",
) -> EvalRunReport:
    out_dir = Path(output_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    cases = [_load_case(Path(path)) for path in paths]
    results = [
        await run_eval_case(
            case,
            model=model,
            base_dir=base_dir,
            output_dir=out_dir,
            agent_config=agent_config,
            system_prompt=system_prompt,
        )
        for case in cases
    ]
    report_path = out_dir / "report.json"
    summary_path = out_dir / "summary.md"
    report_path.write_text(
        json.dumps({"results": [asdict(result) for result in results]}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    summary_path.write_text(_render_summary(results), encoding="utf-8")
    return EvalRunReport(results=results, report_path=str(report_path), summary_path=str(summary_path))


def _load_case(path: Path) -> EvalCase:
    raw = path.read_text(encoding="utf-8")
    if path.suffix.lower() == ".json":
        payload = json.loads(raw)
    else:
        payload = yaml.safe_load(raw)
    if not isinstance(payload, dict):
        raise ValueError(f"Eval case must be an object: {path}")
    return EvalCase(
        id=str(payload["id"]),
        input=str(payload["input"]),
        workspace=str(payload["workspace"]) if payload.get("workspace") is not None else None,
        history=payload.get("history"),
        expected=dict(payload.get("expected") or {}),
        scoring=dict(payload.get("scoring") or {}),
    )


def _prepare_workspace(case: EvalCase, *, base_dir: Path, output_dir: Path) -> Path:
    workdir = output_dir / "workspaces" / case.id
    if workdir.exists():
        shutil.rmtree(workdir)
    if case.workspace:
        src = (base_dir / case.workspace).resolve()
        shutil.copytree(src, workdir)
    else:
        workdir.mkdir(parents=True)
    return workdir


def _load_history(case: EvalCase, *, session: Session, base_dir: Path) -> None:
    history = case.history
    if history is None:
        return
    if isinstance(history, str):
        payload = json.loads((base_dir / history).read_text(encoding="utf-8"))
    else:
        payload = history
    if not isinstance(payload, list):
        raise ValueError("Eval history must be a list")
    for item in payload:
        if not isinstance(item, dict):
            continue
        session.add(
            ChatMessage(
                role=item.get("role", "user"),
                content=str(item.get("content") or ""),
                name=item.get("name"),
                tool_call_id=item.get("tool_call_id"),
                metadata=dict(item.get("metadata") or {}),
            )
        )


def _score_case(
    case: EvalCase,
    *,
    session: Session,
    events: list[dict[str, Any]],
    before_files: dict[str, str],
    after_files: dict[str, str],
    duration_ms: int,
    trace_path: str | None,
) -> EvalResult:
    failures: list[str] = []
    expected = case.expected
    scoring = case.scoring
    final_answer = _final_assistant_text(session)
    obs_events = _observation_events(session)
    changed_files = sorted(path for path in set(before_files) | set(after_files) if before_files.get(path) != after_files.get(path))

    if bool(scoring.get("require_no_error", False)):
        if any(event.get("type") == "error" for event in events) or any(item.get("status") == "error" for item in obs_events):
            failures.append("expected no error events")

    for text in scoring.get("require_final_answer_contains") or []:
        if str(text) not in final_answer:
            failures.append(f"final answer missing required text: {text}")

    for text in scoring.get("forbid_final_answer_contains") or []:
        if str(text) in final_answer:
            failures.append(f"final answer contains forbidden text: {text}")

    for text in expected.get("must_remember") or []:
        if str(text) not in final_answer:
            failures.append(f"final answer did not remember: {text}")

    allowed_changed = expected.get("files_changed")
    if isinstance(allowed_changed, list):
        allowed = {str(item) for item in allowed_changed}
        unexpected = [path for path in changed_files if path not in allowed]
        missing = [path for path in allowed if path not in changed_files]
        if unexpected:
            failures.append("unexpected files changed: " + ", ".join(unexpected))
        if missing:
            failures.append("expected files not changed: " + ", ".join(missing))

    for file_path in scoring.get("file_exists") or []:
        if str(file_path) not in after_files:
            failures.append(f"expected file to exist: {file_path}")

    for file_path, snippets in (scoring.get("file_contains") or {}).items():
        content = after_files.get(str(file_path), "")
        for snippet in snippets or []:
            if str(snippet) not in content:
                failures.append(f"{file_path} missing required text: {snippet}")

    max_steps = scoring.get("max_steps")
    step_count = _count_observation(obs_events, "step.finished")
    if max_steps is not None and step_count > int(max_steps):
        failures.append(f"step count exceeded max_steps: {step_count} > {max_steps}")

    required_tools = {str(item) for item in scoring.get("required_tool_called") or []}
    forbidden_tools = {str(item) for item in scoring.get("forbidden_tool_called") or []}
    tool_names = _tool_names(obs_events)
    for tool in sorted(required_tools):
        if tool not in tool_names:
            failures.append(f"required tool was not called: {tool}")
    for tool in sorted(forbidden_tools):
        if tool in tool_names:
            failures.append(f"forbidden tool was called: {tool}")

    input_tokens, output_tokens, cost = _usage(obs_events)
    max_cost = scoring.get("max_cost")
    if max_cost is not None and cost > float(max_cost):
        failures.append(f"cost exceeded max_cost: {cost} > {max_cost}")

    error_kind = _first_error_kind(obs_events)
    status = "pass" if not failures else "fail"
    return EvalResult(
        case_id=case.id,
        status=status,
        score=1.0 if status == "pass" else 0.0,
        duration_ms=duration_ms,
        steps=step_count,
        tool_calls=len([name for name in tool_names]),
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        cost=cost,
        error_kind=error_kind,
        failure_reasons=failures,
        trace_path=trace_path,
    )


def _snapshot_files(root: Path) -> dict[str, str]:
    files: dict[str, str] = {}
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if ".openagent" in path.parts:
            continue
        rel = path.relative_to(root).as_posix()
        files[rel] = path.read_text(encoding="utf-8", errors="replace")
    return files


def _final_assistant_text(session: Session) -> str:
    for message in reversed(session.messages):
        if message.role == "assistant" and message.content:
            return message.content
    return ""


def _observation_events(session: Session) -> list[dict[str, Any]]:
    root = session.metadata.get("observability")
    if not isinstance(root, dict):
        return []
    events = root.get("events")
    return list(events) if isinstance(events, list) else []


def _trace_path(session: Session) -> str | None:
    root = session.metadata.get("observability")
    if not isinstance(root, dict):
        return None
    value = root.get("jsonl_path")
    return str(value) if value else None


def _count_observation(events: list[dict[str, Any]], name: str) -> int:
    return sum(1 for item in events if item.get("name") == name)


def _tool_names(events: list[dict[str, Any]]) -> set[str]:
    names: set[str] = set()
    for event in events:
        if event.get("name") != "tool.call.finished":
            continue
        attrs = event.get("attributes")
        if isinstance(attrs, dict) and attrs.get("tool_name"):
            names.add(str(attrs["tool_name"]))
    return names


def _usage(events: list[dict[str, Any]]) -> tuple[int, int, float]:
    input_tokens = 0
    output_tokens = 0
    cost = 0.0
    for event in events:
        if event.get("name") != "model.usage":
            continue
        attrs = event.get("attributes")
        if not isinstance(attrs, dict):
            continue
        input_tokens += int(attrs.get("input_tokens") or 0)
        output_tokens += int(attrs.get("output_tokens") or 0)
        cost += float(attrs.get("cost") or 0.0)
    return input_tokens, output_tokens, cost


def _first_error_kind(events: list[dict[str, Any]]) -> str | None:
    for event in events:
        if event.get("status") != "error":
            continue
        attrs = event.get("attributes")
        if isinstance(attrs, dict) and attrs.get("error_kind"):
            return str(attrs["error_kind"])
    return None


def _render_summary(results: list[EvalResult]) -> str:
    total = len(results)
    passed = sum(1 for result in results if result.status == "pass")
    avg_steps = sum(result.steps for result in results) / total if total else 0.0
    avg_input = sum(result.input_tokens for result in results) / total if total else 0.0
    avg_output = sum(result.output_tokens for result in results) / total if total else 0.0
    avg_cost = sum(result.cost for result in results) / total if total else 0.0
    slowest = max(results, key=lambda item: item.duration_ms, default=None)
    priciest = max(results, key=lambda item: item.cost, default=None)
    lines = [
        "# OpenAgent Eval Summary",
        "",
        f"- Total cases: {total}",
        f"- Passed: {passed}",
        f"- Failed: {total - passed}",
        f"- Success rate: {(passed / total * 100) if total else 0:.1f}%",
        f"- Average steps: {avg_steps:.2f}",
        f"- Average input tokens: {avg_input:.2f}",
        f"- Average output tokens: {avg_output:.2f}",
        f"- Average cost: {avg_cost:.6f}",
    ]
    if slowest is not None:
        lines.append(f"- Slowest case: {slowest.case_id} ({slowest.duration_ms} ms)")
    if priciest is not None:
        lines.append(f"- Most expensive case: {priciest.case_id} ({priciest.cost:.6f})")
    failures = [reason for result in results for reason in result.failure_reasons]
    if failures:
        lines.extend(["", "## Failure Reasons"])
        for reason in sorted(set(failures)):
            lines.append(f"- {reason}")
    return "\n".join(lines) + "\n"


__all__ = ["EvalCase", "EvalResult", "EvalRunReport", "run_eval_case", "run_eval_files"]
