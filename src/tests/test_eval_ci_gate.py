from __future__ import annotations

import contextlib
import io
import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.eval.ci_gate import check_eval_ci_gate, main as ci_gate_main


class EvalCiGateTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("src/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"ci_gate_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def _write_report(self, root: Path, *, success_rate: float = 1.0, trace_check_failed: int = 0, runtime_warning_count: int = 0) -> Path:
        report = {
            "schema_version": "openagent.eval.report.v1",
            "aggregate": {
                "total_cases": 1,
                "passed": 1 if success_rate >= 1.0 else 0,
                "failed": 0 if success_rate >= 1.0 else 1,
                "success_rate": success_rate,
                "trace_check_failed": trace_check_failed,
                "runtime_warning_count": runtime_warning_count,
            },
            "results": [
                {
                    "case_id": "case",
                    "status": "pass" if success_rate >= 1.0 else "fail",
                    "trace_check_ok": trace_check_failed == 0,
                    "runtime_warning_count": runtime_warning_count,
                }
            ],
        }
        path = root / "report.json"
        path.write_text(json.dumps(report, ensure_ascii=False), encoding="utf-8")
        return path

    def test_gate_passes_clean_report(self) -> None:
        temp = self._make_temp_dir()
        report_path = self._write_report(temp)

        result = check_eval_ci_gate(report_path, max_runtime_warnings=0)

        self.assertTrue(result.ok)
        self.assertEqual(result.status, "pass")
        self.assertEqual(result.reasons, [])

    def test_gate_fails_runtime_warning_budget(self) -> None:
        temp = self._make_temp_dir()
        report_path = self._write_report(temp, runtime_warning_count=2)

        result = check_eval_ci_gate(report_path, max_runtime_warnings=0)

        self.assertFalse(result.ok)
        self.assertIn("runtime_warning_count exceeded max_runtime_warnings: 2 > 0", result.reasons)

    def test_gate_fails_regression_budget_and_status(self) -> None:
        temp = self._make_temp_dir()
        report_path = self._write_report(temp)
        regression_path = temp / "regression.json"
        regression_path.write_text(
            json.dumps({"summary": {"status_regressions": 1, "budget_regressions": 2}}, ensure_ascii=False),
            encoding="utf-8",
        )

        result = check_eval_ci_gate(report_path, regression_path=regression_path)

        self.assertFalse(result.ok)
        self.assertIn("status_regressions must be 0: 1", result.reasons)
        self.assertIn("budget_regressions must be 0: 2", result.reasons)

    def test_cli_returns_nonzero_for_failed_gate(self) -> None:
        temp = self._make_temp_dir()
        report_path = self._write_report(temp, success_rate=0.5)
        output = io.StringIO()

        with contextlib.redirect_stdout(output):
            exit_code = ci_gate_main(["--report", str(report_path), "--min-success-rate", "1.0"])

        self.assertEqual(exit_code, 1)
        payload = json.loads(output.getvalue())
        self.assertEqual(payload["status"], "fail")
        self.assertIn("success_rate below min_success_rate", payload["reasons"][0])


if __name__ == "__main__":
    unittest.main()
