from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import patch
from uuid import uuid4

from openagent.core.eval import run_eval_case, run_eval_files, summarize_trace
from openagent.core.eval.runner import EvalCase
from openagent.core.trace import LangfuseTraceExporter
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


class FakeLangfuseObservation:
    def __init__(self, observation_id: str) -> None:
        self.id = observation_id
        self.updates: list[dict[str, Any]] = []
        self.ended = False

    def update(self, **payload: Any) -> None:
        self.updates.append(dict(payload))

    def end(self) -> None:
        self.ended = True


class FakeLangfuseClient:
    def __init__(self, *, fail_scores: bool = False) -> None:
        self.fail_scores = fail_scores
        self.observations: list[FakeLangfuseObservation] = []
        self.scores: list[dict[str, Any]] = []
        self.flush_count = 0

    def create_trace_id(self, *, seed: str) -> str:
        del seed
        return "a" * 32

    def start_observation(self, **_kwargs: Any) -> FakeLangfuseObservation:
        observation = FakeLangfuseObservation(f"{len(self.observations) + 1:016x}")
        self.observations.append(observation)
        return observation

    def create_score(self, **payload: Any) -> None:
        if self.fail_scores:
            raise RuntimeError("score export failed")
        self.scores.append(dict(payload))

    def flush(self) -> None:
        self.flush_count += 1


class EvalRunnerTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"eval_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_eval_case_passes_and_writes_trace(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "上下文 decision kept"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.01}},
                ]
            ]
        )
        case = EvalCase(
            id="pass_case",
            input="continue",
            expected={"must_remember": ["上下文"], "files_changed": []},
            scoring={"require_no_error": True, "require_final_answer_contains": ["上下文"]},
        )

        result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out")

        self.assertEqual(result.status, "pass")
        self.assertEqual(result.input_tokens, 2)
        self.assertEqual(result.output_tokens, 3)
        self.assertEqual(result.cost, 0.01)
        self.assertIsNotNone(result.trace_path)
        self.assertTrue(Path(result.trace_path or "").exists())
        self.assertIsNotNone(result.trace_summary_path)
        self.assertTrue(Path(result.trace_summary_path or "").exists())
        self.assertTrue(result.trace_check_ok, result.trace_check_errors)
        self.assertGreater(result.trace_event_count, 0)
        self.assertEqual(result.model_calls, 1)
        summary = summarize_trace(result.trace_path or "")
        self.assertGreater(summary["event_count"], 0)
        self.assertEqual(summary["input_tokens"], 2)
        self.assertEqual(summary["output_tokens"], 3)

    async def test_eval_case_fails_when_final_answer_missing_required_text(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ]
            ]
        )
        case = EvalCase(
            id="fail_case",
            input="continue",
            scoring={"require_final_answer_contains": ["missing"]},
        )

        result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out")

        self.assertEqual(result.status, "fail")
        self.assertIn("final answer missing required text: missing", result.failure_reasons)

    async def test_eval_case_fails_on_forbidden_file_change(self) -> None:
        temp = self._make_temp_dir()
        fixture = temp / "fixture"
        fixture.mkdir()
        (fixture / "README.md").write_text("old", encoding="utf-8")
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "w1",
                        "name": "write",
                        "input": {"file_path": "other.txt", "content": "new"},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        case = EvalCase(
            id="file_case",
            input="write",
            workspace="fixture",
            expected={"files_changed": ["README.md"]},
        )

        result = await run_eval_case(
            case,
            model=model,
            base_dir=temp,
            output_dir=temp / "out",
            agent_config=AgentConfig(name="eval", permission="FULL", max_steps=4, tools=["write"]),
        )

        self.assertEqual(result.status, "fail")
        self.assertTrue(any("unexpected files changed" in reason for reason in result.failure_reasons))
        self.assertEqual((fixture / "README.md").read_text(encoding="utf-8"), "old")

    async def test_run_eval_files_writes_report_and_summary(self) -> None:
        temp = self._make_temp_dir()
        case_path = temp / "case.yaml"
        case_path.write_text(
            "\n".join(
                [
                    "id: report_case",
                    "input: hello",
                    "scoring:",
                    "  require_final_answer_contains:",
                    "    - done",
                ]
            ),
            encoding="utf-8",
        )
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ]
            ]
        )

        report = await run_eval_files([case_path], model=model, base_dir=temp, output_dir=temp / "out")

        self.assertTrue(Path(report.report_path).exists())
        self.assertTrue(Path(report.summary_path).exists())
        payload = json.loads(Path(report.report_path).read_text(encoding="utf-8"))
        self.assertEqual(payload["schema_version"], "openagent.eval.report.v1")
        self.assertEqual(payload["aggregate"]["total_cases"], 1)
        self.assertEqual(payload["results"][0]["case_id"], "report_case")
        self.assertIn("trace_check_ok", payload["results"][0])
        self.assertIn("Success rate", Path(report.summary_path).read_text(encoding="utf-8"))

    async def test_eval_case_scores_trace_requirements(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 2, "cost": 0.0}},
                ]
            ]
        )
        case = EvalCase(
            id="trace_scoring_case",
            input="continue",
            scoring={
                "require_final_answer_contains": ["done"],
                "required_trace_events": ["run.started", "model.call.finished"],
                "forbidden_trace_events": ["tool.call.finished"],
                "max_model_calls": 1,
                "max_tool_calls": 0,
            },
        )

        result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out")

        self.assertEqual(result.status, "pass")
        self.assertTrue(result.trace_check_ok, result.trace_check_errors)
        self.assertEqual(result.model_calls, 1)
        self.assertEqual(result.tool_calls, 0)

    async def test_eval_case_scores_runtime_warnings(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 25, "output_tokens": 8, "cost": 0.05}},
                ]
            ]
        )
        case = EvalCase(
            id="runtime_warning_case",
            input="continue",
            scoring={
                "require_final_answer_contains": ["done"],
                "forbidden_runtime_warnings": ["step_total_tokens_exceeded"],
                "max_runtime_warnings": 0,
            },
        )
        cfg = AgentConfig(
            name="eval",
            permission="FULL",
            max_steps=3,
            tools=[],
            options={"runtime_warnings": {"enabled": True, "max_step_total_tokens": 20}},
        )

        result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out", agent_config=cfg)

        self.assertEqual(result.status, "fail")
        self.assertEqual(result.runtime_warning_count, 1)
        self.assertEqual(result.runtime_warning_codes, ["step_total_tokens_exceeded"])
        self.assertIn("forbidden runtime warning was recorded: step_total_tokens_exceeded", result.failure_reasons)
        self.assertIn("runtime warning count exceeded max_runtime_warnings: 1 > 0", result.failure_reasons)

    async def test_eval_case_exports_langfuse_scores(self) -> None:
        temp = self._make_temp_dir()
        client = FakeLangfuseClient()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.0}},
                ]
            ]
        )
        case = EvalCase(
            id="langfuse_case",
            input="continue",
            scoring={"require_final_answer_contains": ["done"]},
        )
        cfg = AgentConfig(
            name="eval",
            permission="FULL",
            max_steps=3,
            tools=[],
            options={"trace": {"exporters": {"langfuse": {"enabled": True, "keys_required": False, "scores_enabled": True}}}},
        )

        with patch.object(LangfuseTraceExporter, "_load_client", return_value=client):
            result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out", agent_config=cfg)

        self.assertEqual(result.status, "pass")
        self.assertEqual(result.langfuse_trace_id, "a" * 32)
        self.assertTrue(result.langfuse_scores_sent)
        self.assertIsNone(result.langfuse_error)
        self.assertEqual({score["name"] for score in client.scores}, {"openagent.eval.score", "openagent.eval.status", "openagent.trace_check"})
        by_name = {score["name"]: score for score in client.scores}
        self.assertEqual(by_name["openagent.eval.score"]["value"], 1.0)
        self.assertEqual(by_name["openagent.eval.score"]["data_type"], "NUMERIC")
        self.assertEqual(by_name["openagent.eval.status"]["value"], "pass")
        self.assertEqual(by_name["openagent.eval.status"]["data_type"], "CATEGORICAL")
        self.assertTrue(by_name["openagent.trace_check"]["value"])
        self.assertEqual(by_name["openagent.trace_check"]["data_type"], "BOOLEAN")
        self.assertTrue(by_name["openagent.eval.score"]["score_id"].startswith("openagent:run_"))
        self.assertTrue(by_name["openagent.eval.score"]["score_id"].endswith(":langfuse_case:score"))
        self.assertGreaterEqual(client.flush_count, 1)

    async def test_eval_case_keeps_report_when_langfuse_score_export_fails(self) -> None:
        temp = self._make_temp_dir()
        client = FakeLangfuseClient(fail_scores=True)
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ]
            ]
        )
        case = EvalCase(
            id="langfuse_failure_case",
            input="continue",
            scoring={"require_final_answer_contains": ["done"]},
        )
        cfg = AgentConfig(
            name="eval",
            permission="FULL",
            max_steps=3,
            tools=[],
            options={"trace": {"exporters": {"langfuse": {"enabled": True, "keys_required": False, "scores_enabled": True}}}},
        )

        with patch.object(LangfuseTraceExporter, "_load_client", return_value=client):
            result = await run_eval_case(case, model=model, base_dir=temp, output_dir=temp / "out", agent_config=cfg)

        self.assertEqual(result.status, "pass")
        self.assertEqual(result.langfuse_trace_id, "a" * 32)
        self.assertFalse(result.langfuse_scores_sent)
        self.assertIn("score export failed", result.langfuse_error or "")

    async def test_run_eval_files_writes_regression_report(self) -> None:
        temp = self._make_temp_dir()
        case_path = temp / "case.yaml"
        case_path.write_text(
            "\n".join(
                [
                    "id: regression_case",
                    "input: hello",
                    "scoring:",
                    "  require_final_answer_contains:",
                    "    - expected",
                ]
            ),
            encoding="utf-8",
        )
        baseline_path = temp / "baseline.json"
        baseline_path.write_text(
            json.dumps(
                {
                    "results": [
                        {
                            "case_id": "regression_case",
                            "status": "pass",
                            "score": 1.0,
                            "duration_ms": 1,
                            "steps": 1,
                            "model_calls": 1,
                            "tool_calls": 0,
                            "input_tokens": 1,
                            "output_tokens": 1,
                            "cost": 0.0,
                            "trace_check_ok": True,
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "actual"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.02}},
                ]
            ]
        )

        report = await run_eval_files(
            [case_path],
            model=model,
            base_dir=temp,
            output_dir=temp / "out",
            baseline_report=baseline_path,
        )

        self.assertIsNotNone(report.regression_path)
        self.assertIsNotNone(report.regression_summary_path)
        regression = json.loads(Path(report.regression_path or "").read_text(encoding="utf-8"))
        self.assertEqual(regression["summary"]["status_regressions"], 1)
        self.assertEqual(regression["cases"][0]["case_id"], "regression_case")
        self.assertIn("Status regressions", Path(report.regression_summary_path or "").read_text(encoding="utf-8"))

    async def test_run_eval_files_flags_budget_regressions(self) -> None:
        temp = self._make_temp_dir()
        case_path = temp / "case.yaml"
        case_path.write_text(
            "\n".join(
                [
                    "id: budget_case",
                    "input: hello",
                    "scoring:",
                    "  require_final_answer_contains:",
                    "    - expected",
                ]
            ),
            encoding="utf-8",
        )
        baseline_path = temp / "baseline.json"
        baseline_path.write_text(
            json.dumps(
                {
                    "results": [
                        {
                            "case_id": "budget_case",
                            "status": "pass",
                            "score": 1.0,
                            "duration_ms": 1,
                            "steps": 1,
                            "model_calls": 1,
                            "tool_calls": 0,
                            "input_tokens": 10,
                            "output_tokens": 5,
                            "cost": 0.01,
                            "trace_check_ok": True,
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "expected"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 25, "output_tokens": 8, "cost": 0.05}},
                ]
            ]
        )

        report = await run_eval_files(
            [case_path],
            model=model,
            base_dir=temp,
            output_dir=temp / "out",
            baseline_report=baseline_path,
            regression_thresholds={
                "max_total_tokens_delta": 10,
                "max_cost_delta": 0.02,
                "max_duration_delta_ms": 10_000,
            },
        )

        regression = json.loads(Path(report.regression_path or "").read_text(encoding="utf-8"))
        case = regression["cases"][0]
        self.assertEqual(case["total_tokens_delta"], 18)
        self.assertEqual(regression["summary"]["total_tokens_increased_cases"], 1)
        self.assertEqual(regression["summary"]["budget_regressions"], 1)
        budget_reasons = " ".join(case["budget_regressions"])
        self.assertIn("total_tokens_delta exceeded max_total_tokens_delta", budget_reasons)
        self.assertIn("cost_delta exceeded max_cost_delta", budget_reasons)
        summary = Path(report.regression_summary_path or "").read_text(encoding="utf-8")
        self.assertIn("Budget regressions: 1", summary)
        self.assertIn("max_total_tokens_delta: 10", summary)


if __name__ == "__main__":
    unittest.main()
