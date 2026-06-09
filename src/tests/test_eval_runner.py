from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.eval import run_eval_case, run_eval_files, summarize_trace
from openagent.core.eval.runner import EvalCase
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


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


if __name__ == "__main__":
    unittest.main()
