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
        summary = summarize_trace(result.trace_path or "")
        self.assertGreater(summary["event_count"], 0)

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
        self.assertEqual(payload["results"][0]["case_id"], "report_case")
        self.assertIn("Success rate", Path(report.summary_path).read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
