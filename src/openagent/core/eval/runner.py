from __future__ import annotations

import copy
import json
import shutil
import time
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from typing import Any

import yaml

from ..agent.universal import UniversalAgent
from ..permission.manager import PermissionManager
from ..session.session import Session
from ..trace import TRACE_METADATA_KEY, check_trace_run, load_trace_events, load_trace_summary
from ..trace.exporter import load_langfuse_client
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
    trace_summary_path: str | None = None
    trace_check_ok: bool = False
    trace_check_errors: list[str] = field(default_factory=list)
    trace_event_count: int = 0
    model_calls: int = 0
    mcp_calls: int = 0
    skill_calls: int = 0
    local_tool_calls: int = 0
    artifact_count: int = 0
    error_count: int = 0
    total_latency_ms: int = 0
    langfuse_trace_id: str | None = None
    langfuse_scores_sent: bool = False
    langfuse_error: str | None = None


@dataclass(frozen=True, slots=True)
class EvalRunReport:
    results: list[EvalResult]
    report_path: str
    summary_path: str
    regression_path: str | None = None
    regression_summary_path: str | None = None


@dataclass(frozen=True, slots=True)
class _TraceMetrics:
    trace_path: str | None
    summary_path: str | None
    summary: dict[str, Any]
    events: list[dict[str, Any]]
    check: dict[str, Any] | None


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
    trace = dict(options.get("trace") or {})
    trace.setdefault("enabled", True)
    trace.setdefault("root_dir", str(out_dir / "runs"))
    trace.setdefault("keep_events", True)
    trace.setdefault("write_summary", True)
    options["trace"] = trace
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
    trace_metrics = _trace_metrics(session)
    result = _score_case(
        case,
        session=session,
        events=events,
        before_files=before_files,
        after_files=after_files,
        duration_ms=duration_ms,
        trace_metrics=trace_metrics,
    )
    return _export_langfuse_eval_scores(
        result,
        case=case,
        session=session,
        trace_options=_langfuse_trace_options(cfg.options),
    )


async def run_eval_files(
    paths: list[str | Path],
    *,
    model: LanguageModel,
    base_dir: Path | str,
    output_dir: Path | str,
    agent_config: AgentConfig | None = None,
    system_prompt: str = "Test prompt.",
    baseline_report: Path | str | None = None,
    regression_thresholds: dict[str, Any] | None = None,
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
    regression_path: Path | None = None
    regression_summary_path: Path | None = None
    regression: dict[str, Any] | None = None
    if baseline_report is not None:
        regression = _compare_with_baseline(
            baseline_report=Path(baseline_report),
            current_results=results,
            current_report_path=report_path,
            thresholds=regression_thresholds,
        )
        regression_path = out_dir / "regression.json"
        regression_summary_path = out_dir / "regression.md"
        regression_path.write_text(json.dumps(regression, ensure_ascii=False, indent=2), encoding="utf-8")
        regression_summary_path.write_text(_render_regression_summary(regression), encoding="utf-8")
    payload = {
        "schema_version": "openagent.eval.report.v1",
        "aggregate": _aggregate_results(results),
        "results": [asdict(result) for result in results],
    }
    if regression is not None:
        payload["regression"] = {
            "path": str(regression_path) if regression_path is not None else None,
            "summary_path": str(regression_summary_path) if regression_summary_path is not None else None,
            "summary": regression.get("summary", {}),
        }
    report_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    summary_path.write_text(_render_summary(results), encoding="utf-8")
    return EvalRunReport(
        results=results,
        report_path=str(report_path),
        summary_path=str(summary_path),
        regression_path=str(regression_path) if regression_path is not None else None,
        regression_summary_path=str(regression_summary_path) if regression_summary_path is not None else None,
    )


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
    trace_metrics: _TraceMetrics,
) -> EvalResult:
    failures: list[str] = []
    expected = case.expected
    scoring = case.scoring
    final_answer = _final_assistant_text(session)
    obs_events = _observation_events(session)
    trace_summary = trace_metrics.summary
    trace_events = trace_metrics.events
    trace_check = trace_metrics.check or {}
    trace_check_ok = bool(trace_check.get("ok"))
    trace_check_errors = [str(item) for item in trace_check.get("errors") or []]
    trace_event_names = _event_names(trace_events)
    changed_files = sorted(path for path in set(before_files) | set(after_files) if before_files.get(path) != after_files.get(path))

    if bool(scoring.get("require_no_error", False)):
        if any(event.get("type") == "error" for event in events) or any(item.get("status") == "error" for item in obs_events):
            failures.append("expected no error events")

    if _bool_scoring(scoring.get("require_trace_check", True)) and not trace_check_ok:
        detail = "; ".join(trace_check_errors) if trace_check_errors else "missing trace check"
        failures.append(f"trace integrity check failed: {detail}")

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
    step_count = _summary_int(trace_summary, "step_count", default=_count_observation(obs_events, "step.finished"))
    if max_steps is not None and step_count > int(max_steps):
        failures.append(f"step count exceeded max_steps: {step_count} > {max_steps}")

    required_tools = {str(item) for item in scoring.get("required_tool_called") or []}
    forbidden_tools = {str(item) for item in scoring.get("forbidden_tool_called") or []}
    tool_names = _tool_names(obs_events) | _trace_tool_names(trace_events)
    for tool in sorted(required_tools):
        if tool not in tool_names:
            failures.append(f"required tool was not called: {tool}")
    for tool in sorted(forbidden_tools):
        if tool in tool_names:
            failures.append(f"forbidden tool was called: {tool}")

    required_trace_events = {str(item) for item in scoring.get("required_trace_events") or []}
    forbidden_trace_events = {str(item) for item in scoring.get("forbidden_trace_events") or []}
    for event_name in sorted(required_trace_events):
        if event_name not in trace_event_names:
            failures.append(f"required trace event was not recorded: {event_name}")
    for event_name in sorted(forbidden_trace_events):
        if event_name in trace_event_names:
            failures.append(f"forbidden trace event was recorded: {event_name}")

    required_sources = {str(item) for item in scoring.get("required_tool_sources") or []}
    trace_tool_sources = _trace_tool_sources(trace_events)
    for source in sorted(required_sources):
        if source not in trace_tool_sources:
            failures.append(f"required tool source was not called: {source}")

    tool_call_count = _summary_int(trace_summary, "tool_call_count", default=_count_observation(obs_events, "tool.call.finished"))
    model_call_count = _summary_int(trace_summary, "model_call_count", default=_count_observation(obs_events, "model.call.finished"))
    max_tool_calls = scoring.get("max_tool_calls")
    if max_tool_calls is not None and tool_call_count > int(max_tool_calls):
        failures.append(f"tool call count exceeded max_tool_calls: {tool_call_count} > {max_tool_calls}")
    max_model_calls = scoring.get("max_model_calls")
    if max_model_calls is not None and model_call_count > int(max_model_calls):
        failures.append(f"model call count exceeded max_model_calls: {model_call_count} > {max_model_calls}")
    max_duration_ms = scoring.get("max_duration_ms")
    if max_duration_ms is not None and duration_ms > int(max_duration_ms):
        failures.append(f"duration exceeded max_duration_ms: {duration_ms} > {max_duration_ms}")
    total_latency_ms = _summary_int(trace_summary, "total_latency_ms", default=duration_ms)
    max_total_latency_ms = scoring.get("max_total_latency_ms")
    if max_total_latency_ms is not None and total_latency_ms > int(max_total_latency_ms):
        failures.append(f"trace latency exceeded max_total_latency_ms: {total_latency_ms} > {max_total_latency_ms}")

    input_tokens, output_tokens, cost = _trace_usage(trace_summary, obs_events)
    max_cost = scoring.get("max_cost")
    if max_cost is not None and cost > float(max_cost):
        failures.append(f"cost exceeded max_cost: {cost} > {max_cost}")

    error_kind = _first_error_kind(obs_events) or _first_trace_error_kind(trace_summary)
    status = "pass" if not failures else "fail"
    return EvalResult(
        case_id=case.id,
        status=status,
        score=1.0 if status == "pass" else 0.0,
        duration_ms=duration_ms,
        steps=step_count,
        tool_calls=tool_call_count,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        cost=cost,
        error_kind=error_kind,
        failure_reasons=failures,
        trace_path=trace_metrics.trace_path,
        trace_summary_path=trace_metrics.summary_path,
        trace_check_ok=trace_check_ok,
        trace_check_errors=trace_check_errors,
        trace_event_count=_summary_int(trace_summary, "event_count", default=len(trace_events)),
        model_calls=model_call_count,
        mcp_calls=_summary_int(trace_summary, "mcp_call_count", default=_trace_source_count(trace_events, "mcp")),
        skill_calls=_summary_int(trace_summary, "skill_call_count", default=_trace_source_count(trace_events, "skill")),
        local_tool_calls=_summary_int(trace_summary, "local_tool_call_count", default=_trace_source_count(trace_events, "local_tool")),
        artifact_count=_summary_int(trace_summary, "artifact_count", default=0),
        error_count=_summary_int(trace_summary, "error_count", default=sum(1 for event in trace_events if event.get("status") == "error")),
        total_latency_ms=total_latency_ms,
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
    agent_trace = _agent_trace_metadata(session)
    value = agent_trace.get("trace_path")
    if value:
        return str(value)
    root = session.metadata.get("observability")
    if not isinstance(root, dict):
        return None
    value = root.get("jsonl_path")
    return str(value) if value else None


def _trace_summary_path(session: Session) -> str | None:
    agent_trace = _agent_trace_metadata(session)
    value = agent_trace.get("summary_path")
    return str(value) if value else None


def _trace_metrics(session: Session) -> _TraceMetrics:
    trace_path = _trace_path(session)
    summary_path = _trace_summary_path(session)
    events: list[dict[str, Any]] = []
    summary: dict[str, Any] = {}
    check: dict[str, Any] | None = None
    if trace_path:
        try:
            events = load_trace_events(trace_path)
        except Exception:  # noqa: BLE001
            events = []
    if summary_path:
        try:
            summary = load_trace_summary(summary_path)
        except Exception:  # noqa: BLE001
            summary = {}
    run_dir = _agent_trace_metadata(session).get("run_dir")
    if run_dir:
        try:
            check = check_trace_run(str(run_dir))
        except Exception as error:  # noqa: BLE001
            check = {"ok": False, "errors": [str(error)]}
    return _TraceMetrics(trace_path=trace_path, summary_path=summary_path, summary=summary, events=events, check=check)


def _agent_trace_metadata(session: Session) -> dict[str, Any]:
    value = session.metadata.get(TRACE_METADATA_KEY)
    return dict(value) if isinstance(value, dict) else {}


def _langfuse_exporter_metadata(session: Session) -> dict[str, Any]:
    exporters = _agent_trace_metadata(session).get("exporters")
    if not isinstance(exporters, dict):
        return {}
    langfuse = exporters.get("langfuse")
    return dict(langfuse) if isinstance(langfuse, dict) else {}


def _langfuse_trace_options(options: dict[str, Any] | None) -> dict[str, Any]:
    raw_options = options or {}
    trace = raw_options.get("trace")
    if not isinstance(trace, dict):
        return {}
    exporters = trace.get("exporters")
    if not isinstance(exporters, dict):
        return {}
    langfuse = exporters.get("langfuse")
    return dict(langfuse) if isinstance(langfuse, dict) else {}


def _export_langfuse_eval_scores(
    result: EvalResult,
    *,
    case: EvalCase,
    session: Session,
    trace_options: dict[str, Any],
) -> EvalResult:
    metadata = _langfuse_exporter_metadata(session)
    trace_id = str(metadata.get("trace_id") or "").strip()
    if not trace_id:
        return result
    result = replace(result, langfuse_trace_id=trace_id)
    if not _bool_scoring(metadata.get("scores_enabled", True)):
        return result
    try:
        client = load_langfuse_client(trace_options)
        run_id = str(_agent_trace_metadata(session).get("run_id") or "run")
        _create_langfuse_score(
            client,
            trace_id=trace_id,
            score_id=f"openagent:{run_id}:{case.id}:score",
            name="openagent.eval.score",
            value=float(result.score),
            data_type="NUMERIC",
            comment=f"OpenAgent eval score for case {case.id}.",
        )
        status_comment = "; ".join(result.failure_reasons[:5]) if result.failure_reasons else f"OpenAgent eval status for case {case.id}."
        _create_langfuse_score(
            client,
            trace_id=trace_id,
            score_id=f"openagent:{run_id}:{case.id}:status",
            name="openagent.eval.status",
            value=result.status,
            data_type="CATEGORICAL",
            comment=status_comment,
        )
        _create_langfuse_score(
            client,
            trace_id=trace_id,
            score_id=f"openagent:{run_id}:{case.id}:trace_check",
            name="openagent.trace_check",
            value=bool(result.trace_check_ok),
            data_type="BOOLEAN",
            comment="OpenAgent trace integrity check result.",
        )
        flush = getattr(client, "flush", None)
        if callable(flush):
            flush()
        return replace(result, langfuse_scores_sent=True, langfuse_error=None)
    except Exception as error:  # noqa: BLE001
        return replace(result, langfuse_scores_sent=False, langfuse_error=str(error))


def _create_langfuse_score(
    client: Any,
    *,
    trace_id: str,
    score_id: str,
    name: str,
    value: Any,
    data_type: str,
    comment: str,
) -> None:
    create_score = getattr(client, "create_score", None)
    if not callable(create_score):
        raise RuntimeError("Langfuse client does not support create_score().")
    create_score(
        trace_id=trace_id,
        score_id=score_id,
        name=name,
        value=value,
        data_type=data_type,
        comment=comment,
    )


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


def _trace_tool_names(events: list[dict[str, Any]]) -> set[str]:
    names: set[str] = set()
    for event in events:
        if _event_name(event) != "tool.call.finished":
            continue
        attrs = event.get("attributes")
        if isinstance(attrs, dict) and attrs.get("tool_name"):
            names.add(str(attrs["tool_name"]))
    return names


def _event_names(events: list[dict[str, Any]]) -> set[str]:
    return {_event_name(event) for event in events if _event_name(event)}


def _event_name(event: dict[str, Any]) -> str:
    return str(event.get("event") or event.get("name") or "")


def _trace_tool_sources(events: list[dict[str, Any]]) -> set[str]:
    sources: set[str] = set()
    for event in events:
        if _event_name(event) != "tool.call.finished":
            continue
        attrs = event.get("attributes")
        if not isinstance(attrs, dict):
            continue
        source = str(attrs.get("tool_source") or attrs.get("source") or "").strip()
        if not source and str(attrs.get("backend") or "") == "mcp":
            source = "mcp"
        if not source and (attrs.get("skill_name") or attrs.get("tool_group") == "skill"):
            source = "skill"
        if not source and attrs.get("tool_name"):
            source = "local_tool"
        if source:
            sources.add(source)
    return sources


def _trace_source_count(events: list[dict[str, Any]], source: str) -> int:
    count = 0
    for event in events:
        if _event_name(event) != "tool.call.finished":
            continue
        attrs = event.get("attributes")
        if not isinstance(attrs, dict):
            continue
        sources = _trace_tool_sources([event])
        if source in sources or (source == "local_tool" and "local" in sources):
            count += 1
    return count


def _trace_usage(summary: dict[str, Any], obs_events: list[dict[str, Any]]) -> tuple[int, int, float]:
    if summary:
        return (
            _summary_int(summary, "total_input_tokens", default=0),
            _summary_int(summary, "total_output_tokens", default=0),
            float(summary.get("total_cost") or 0.0),
        )
    return _usage(obs_events)


def _summary_int(summary: dict[str, Any], key: str, *, default: int) -> int:
    if key not in summary:
        return default
    try:
        return int(summary.get(key) or 0)
    except (TypeError, ValueError):
        return default


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


def _first_trace_error_kind(summary: dict[str, Any]) -> str | None:
    errors = summary.get("errors")
    if not isinstance(errors, list):
        return None
    for error in errors:
        if isinstance(error, dict) and error.get("error_kind"):
            return str(error["error_kind"])
    return None


def _bool_scoring(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() not in {"0", "false", "no", "off"}
    return bool(value)


def _aggregate_results(results: list[EvalResult]) -> dict[str, Any]:
    total = len(results)
    passed = sum(1 for result in results if result.status == "pass")
    return {
        "total_cases": total,
        "passed": passed,
        "failed": total - passed,
        "success_rate": (passed / total) if total else 0.0,
        "average_steps": _avg(result.steps for result in results),
        "average_model_calls": _avg(result.model_calls for result in results),
        "average_tool_calls": _avg(result.tool_calls for result in results),
        "total_input_tokens": sum(result.input_tokens for result in results),
        "total_output_tokens": sum(result.output_tokens for result in results),
        "total_cost": sum(result.cost for result in results),
        "average_duration_ms": _avg(result.duration_ms for result in results),
        "average_total_latency_ms": _avg(result.total_latency_ms for result in results),
        "trace_check_passed": sum(1 for result in results if result.trace_check_ok),
        "trace_check_failed": sum(1 for result in results if not result.trace_check_ok),
        "mcp_calls": sum(result.mcp_calls for result in results),
        "skill_calls": sum(result.skill_calls for result in results),
        "local_tool_calls": sum(result.local_tool_calls for result in results),
        "artifact_count": sum(result.artifact_count for result in results),
        "error_count": sum(result.error_count for result in results),
    }


def _compare_with_baseline(
    *,
    baseline_report: Path,
    current_results: list[EvalResult],
    current_report_path: Path,
    thresholds: dict[str, Any] | None = None,
) -> dict[str, Any]:
    baseline_payload = json.loads(baseline_report.read_text(encoding="utf-8"))
    baseline_results = baseline_payload.get("results", []) if isinstance(baseline_payload, dict) else []
    if not isinstance(baseline_results, list):
        baseline_results = []
    raw_thresholds = thresholds
    if raw_thresholds is None and isinstance(baseline_payload, dict):
        raw_thresholds = baseline_payload.get("regression_thresholds")
    threshold_config = _normalize_regression_thresholds(raw_thresholds)
    baseline_by_id = {str(item.get("case_id")): item for item in baseline_results if isinstance(item, dict) and item.get("case_id")}
    current_by_id = {result.case_id: asdict(result) for result in current_results}
    cases: list[dict[str, Any]] = []
    for case_id in sorted(set(baseline_by_id) | set(current_by_id)):
        baseline = baseline_by_id.get(case_id)
        current = current_by_id.get(case_id)
        if baseline is None:
            cases.append({"case_id": case_id, "change": "new_case", "current": _case_regression_fields(current)})
            continue
        if current is None:
            cases.append({"case_id": case_id, "change": "removed_case", "baseline": _case_regression_fields(baseline)})
            continue
        score_delta = _float_field(current, "score") - _float_field(baseline, "score")
        cost_delta = _float_field(current, "cost") - _float_field(baseline, "cost")
        duration_delta_ms = _int_field(current, "duration_ms") - _int_field(baseline, "duration_ms")
        input_tokens_delta = _int_field(current, "input_tokens") - _int_field(baseline, "input_tokens")
        output_tokens_delta = _int_field(current, "output_tokens") - _int_field(baseline, "output_tokens")
        total_tokens_delta = (_int_field(current, "input_tokens") + _int_field(current, "output_tokens")) - (
            _int_field(baseline, "input_tokens") + _int_field(baseline, "output_tokens")
        )
        tool_calls_delta = _int_field(current, "tool_calls") - _int_field(baseline, "tool_calls")
        model_calls_delta = _int_field(current, "model_calls") - _int_field(baseline, "model_calls")
        baseline_status = str(baseline.get("status") or "")
        current_status = str(current.get("status") or "")
        budget_regressions = _budget_regressions(
            {
                "cost_delta": cost_delta,
                "duration_delta_ms": duration_delta_ms,
                "input_tokens_delta": input_tokens_delta,
                "output_tokens_delta": output_tokens_delta,
                "total_tokens_delta": total_tokens_delta,
                "tool_calls_delta": tool_calls_delta,
                "model_calls_delta": model_calls_delta,
            },
            threshold_config,
        )
        cases.append(
            {
                "case_id": case_id,
                "change": "compared",
                "baseline": _case_regression_fields(baseline),
                "current": _case_regression_fields(current),
                "status_changed": baseline_status != current_status,
                "status_regression": baseline_status == "pass" and current_status != "pass",
                "status_improvement": baseline_status != "pass" and current_status == "pass",
                "score_delta": score_delta,
                "cost_delta": cost_delta,
                "duration_delta_ms": duration_delta_ms,
                "input_tokens_delta": input_tokens_delta,
                "output_tokens_delta": output_tokens_delta,
                "total_tokens_delta": total_tokens_delta,
                "tool_calls_delta": tool_calls_delta,
                "model_calls_delta": model_calls_delta,
                "budget_regressions": budget_regressions,
            }
        )
    summary = {
        "baseline_report": str(baseline_report),
        "current_report": str(current_report_path),
        "regression_thresholds": threshold_config,
        "baseline_total": len(baseline_by_id),
        "current_total": len(current_by_id),
        "new_cases": sum(1 for item in cases if item.get("change") == "new_case"),
        "removed_cases": sum(1 for item in cases if item.get("change") == "removed_case"),
        "status_regressions": sum(1 for item in cases if item.get("status_regression")),
        "status_improvements": sum(1 for item in cases if item.get("status_improvement")),
        "score_regressions": sum(1 for item in cases if float(item.get("score_delta") or 0.0) < 0),
        "cost_increased_cases": sum(1 for item in cases if float(item.get("cost_delta") or 0.0) > 0),
        "input_tokens_increased_cases": sum(1 for item in cases if int(item.get("input_tokens_delta") or 0) > 0),
        "output_tokens_increased_cases": sum(1 for item in cases if int(item.get("output_tokens_delta") or 0) > 0),
        "total_tokens_increased_cases": sum(1 for item in cases if int(item.get("total_tokens_delta") or 0) > 0),
        "duration_increased_cases": sum(1 for item in cases if int(item.get("duration_delta_ms") or 0) > 0),
        "budget_regressions": sum(1 for item in cases if item.get("budget_regressions")),
    }
    return {"summary": summary, "cases": cases}


def _normalize_regression_thresholds(raw: Any) -> dict[str, float]:
    if not isinstance(raw, dict):
        return {}
    allowed = {
        "max_cost_delta",
        "max_duration_delta_ms",
        "max_input_tokens_delta",
        "max_output_tokens_delta",
        "max_total_tokens_delta",
        "max_tool_calls_delta",
        "max_model_calls_delta",
    }
    normalized: dict[str, float] = {}
    for key in allowed:
        if key not in raw:
            continue
        try:
            normalized[key] = float(raw[key])
        except (TypeError, ValueError):
            continue
    return normalized


def _budget_regressions(deltas: dict[str, float | int], thresholds: dict[str, float]) -> list[str]:
    checks = {
        "max_cost_delta": "cost_delta",
        "max_duration_delta_ms": "duration_delta_ms",
        "max_input_tokens_delta": "input_tokens_delta",
        "max_output_tokens_delta": "output_tokens_delta",
        "max_total_tokens_delta": "total_tokens_delta",
        "max_tool_calls_delta": "tool_calls_delta",
        "max_model_calls_delta": "model_calls_delta",
    }
    failures: list[str] = []
    for threshold_key, delta_key in checks.items():
        if threshold_key not in thresholds:
            continue
        delta = float(deltas.get(delta_key) or 0.0)
        threshold = float(thresholds[threshold_key])
        if delta > threshold:
            failures.append(f"{delta_key} exceeded {threshold_key}: {delta:g} > {threshold:g}")
    return failures


def _case_regression_fields(item: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(item, dict):
        return {}
    fields = [
        "status",
        "score",
        "duration_ms",
        "steps",
        "model_calls",
        "tool_calls",
        "input_tokens",
        "output_tokens",
        "cost",
        "trace_check_ok",
    ]
    return {field: item.get(field) for field in fields if field in item}


def _render_regression_summary(regression: dict[str, Any]) -> str:
    summary = regression.get("summary") if isinstance(regression.get("summary"), dict) else {}
    lines = [
        "# OpenAgent Eval Regression",
        "",
        f"- Baseline cases: {summary.get('baseline_total', 0)}",
        f"- Current cases: {summary.get('current_total', 0)}",
        f"- New cases: {summary.get('new_cases', 0)}",
        f"- Removed cases: {summary.get('removed_cases', 0)}",
        f"- Status regressions: {summary.get('status_regressions', 0)}",
        f"- Status improvements: {summary.get('status_improvements', 0)}",
        f"- Score regressions: {summary.get('score_regressions', 0)}",
        f"- Cost increased cases: {summary.get('cost_increased_cases', 0)}",
        f"- Token increased cases: {summary.get('total_tokens_increased_cases', 0)}",
        f"- Duration increased cases: {summary.get('duration_increased_cases', 0)}",
        f"- Budget regressions: {summary.get('budget_regressions', 0)}",
    ]
    thresholds = summary.get("regression_thresholds")
    if isinstance(thresholds, dict) and thresholds:
        lines.extend(["", "## Regression Thresholds"])
        for key in sorted(thresholds):
            lines.append(f"- {key}: {thresholds[key]:g}")
    interesting = [
        item
        for item in regression.get("cases", [])
        if isinstance(item, dict)
        and (
            item.get("change") != "compared"
            or item.get("status_changed")
            or float(item.get("score_delta") or 0.0) != 0.0
            or float(item.get("cost_delta") or 0.0) != 0.0
            or int(item.get("total_tokens_delta") or 0) != 0
            or int(item.get("duration_delta_ms") or 0) != 0
            or bool(item.get("budget_regressions"))
        )
    ]
    if interesting:
        lines.extend(["", "## Case Changes"])
        for item in interesting:
            case_id = item.get("case_id")
            change = item.get("change")
            if change != "compared":
                lines.append(f"- {case_id}: {change}")
                continue
            baseline_status = (item.get("baseline") or {}).get("status") if isinstance(item.get("baseline"), dict) else None
            current_status = (item.get("current") or {}).get("status") if isinstance(item.get("current"), dict) else None
            lines.append(
                f"- {case_id}: {baseline_status} -> {current_status}, "
                f"score_delta={float(item.get('score_delta') or 0.0):.3f}, "
                f"cost_delta={float(item.get('cost_delta') or 0.0):.6f}, "
                f"tokens_delta={int(item.get('total_tokens_delta') or 0)}, "
                f"duration_delta_ms={int(item.get('duration_delta_ms') or 0)}"
            )
            budget_regressions = item.get("budget_regressions")
            if isinstance(budget_regressions, list):
                for reason in budget_regressions:
                    lines.append(f"  - budget: {reason}")
    return "\n".join(lines) + "\n"


def _float_field(item: dict[str, Any], field: str) -> float:
    try:
        return float(item.get(field) or 0.0)
    except (TypeError, ValueError):
        return 0.0


def _int_field(item: dict[str, Any], field: str) -> int:
    try:
        return int(item.get(field) or 0)
    except (TypeError, ValueError):
        return 0


def _avg(values: Any) -> float:
    materialized = list(values)
    return sum(materialized) / len(materialized) if materialized else 0.0


def _render_summary(results: list[EvalResult]) -> str:
    aggregate = _aggregate_results(results)
    total = int(aggregate["total_cases"])
    passed = int(aggregate["passed"])
    slowest = max(results, key=lambda item: item.duration_ms, default=None)
    priciest = max(results, key=lambda item: item.cost, default=None)
    lines = [
        "# OpenAgent Eval Summary",
        "",
        f"- Total cases: {total}",
        f"- Passed: {passed}",
        f"- Failed: {total - passed}",
        f"- Success rate: {(passed / total * 100) if total else 0:.1f}%",
        f"- Average steps: {float(aggregate['average_steps']):.2f}",
        f"- Average model calls: {float(aggregate['average_model_calls']):.2f}",
        f"- Average tool calls: {float(aggregate['average_tool_calls']):.2f}",
        f"- Total input tokens: {aggregate['total_input_tokens']}",
        f"- Total output tokens: {aggregate['total_output_tokens']}",
        f"- Total cost: {float(aggregate['total_cost']):.6f}",
        f"- Average duration: {float(aggregate['average_duration_ms']):.2f} ms",
        f"- Trace checks passed: {aggregate['trace_check_passed']}",
        f"- Trace checks failed: {aggregate['trace_check_failed']}",
        f"- Tool sources: local={aggregate['local_tool_calls']} skill={aggregate['skill_calls']} mcp={aggregate['mcp_calls']}",
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
