from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

from swarm import SwarmInspectionConfig, create_inspection_server, load_run_artifact, load_run_detail, load_run_index, write_coordinator_receipt


class SwarmInspectionTests(unittest.TestCase):
    def test_load_run_index_and_detail_from_state_handoff_and_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            state_dir = tmp / "state"
            handoff_dir = tmp / "handoff"
            _write_run_state(state_dir, "run-one")
            _write_handoff(handoff_dir, "run-one")
            receipt_path = write_coordinator_receipt(
                handoff_dir,
                {
                    "schema_version": 1,
                    "run_id": "run-one",
                    "task_id": "task",
                    "run_status": "completed",
                    "runner_count": 1,
                    "runner_status_counts": {"completed": 1},
                    "trace_event_count": 2,
                    "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7, "cost": 0.0},
                    "summary": "receipt summary",
                },
            )

            config = SwarmInspectionConfig(state_dir=state_dir, handoff_dir=handoff_dir)
            index = load_run_index(config)
            detail = load_run_detail(config, "run-one")
            trace = load_run_artifact(config, "run-one", "trace")

        self.assertEqual(receipt_path.name, "coordinator-receipt.json")
        self.assertEqual(index["run_count"], 1)
        run = index["runs"][0]
        self.assertEqual(run["run_id"], "run-one")
        self.assertEqual(run["task_id"], "task")
        self.assertEqual(run["status"], "completed")
        self.assertEqual(run["runner_count"], 1)
        self.assertEqual(run["runner_status_counts"], {"completed": 1})
        self.assertTrue(run["has_state"])
        self.assertTrue(run["has_handoff"])
        self.assertTrue(run["has_receipt"])
        self.assertEqual(run["links"]["receipt"], "/runs/run-one/receipt")
        self.assertIsNotNone(detail)
        assert detail is not None
        self.assertEqual(detail["receipt"]["run_id"], "run-one")
        self.assertEqual(detail["state"]["status"], "completed")
        self.assertEqual(detail["handoff"]["pending_runner_ids"], [])
        self.assertEqual(len(trace), 2)

    def test_malformed_artifacts_become_diagnostics(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            bad_dir = tmp / "state" / "bad-run"
            bad_dir.mkdir(parents=True)
            (bad_dir / "state.latest.json").write_text("{bad json", encoding="utf-8")

            index = load_run_index(SwarmInspectionConfig(state_dir=tmp / "state"))

        self.assertEqual(index["run_count"], 1)
        self.assertEqual(index["runs"][0]["run_id"], "bad-run")
        self.assertTrue(index["diagnostics"])
        self.assertEqual(index["diagnostics"][0]["artifact"], "state")

    def test_http_api_serves_runs_and_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            state_dir = tmp / "state"
            handoff_dir = tmp / "handoff"
            _write_run_state(state_dir, "run-api")
            _write_handoff(handoff_dir, "run-api")
            write_coordinator_receipt(handoff_dir, {"run_id": "run-api", "task_id": "task", "run_status": "completed"})
            server = create_inspection_server(SwarmInspectionConfig(state_dir=state_dir, handoff_dir=handoff_dir), port=0)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            host, port = server.server_address
            base = f"http://{host}:{port}"

            try:
                health = _get_json(f"{base}/health")
                runs = _get_json(f"{base}/runs")
                detail = _get_json(f"{base}/runs/run-api")
                receipt = _get_json(f"{base}/runs/run-api/receipt")
                trace = _get_json(f"{base}/runs/run-api/trace")
                root_html, root_headers = _get_text(f"{base}/")
                ui_html, ui_headers = _get_text(f"{base}/ui")
                missing_status, missing = _get_json_error(f"{base}/runs/missing")
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=1)

        self.assertEqual(health["status"], "ok")
        self.assertEqual(runs["run_count"], 1)
        self.assertEqual(detail["run"]["run_id"], "run-api")
        self.assertEqual(receipt["run_id"], "run-api")
        self.assertEqual(len(trace), 2)
        self.assertIn("Swarm Inspection", root_html)
        self.assertIn('fetchJson("/runs")', root_html)
        self.assertIn("Runner Status", ui_html)
        self.assertIn("text/html", root_headers["content-type"])
        self.assertIn("text/html", ui_headers["content-type"])
        self.assertEqual(missing_status, 404)
        self.assertEqual(missing["status"], "not_found")


def _write_run_state(root: Path, run_id: str) -> None:
    run_dir = root / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "state.latest.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "run_id": run_id,
                "task_id": "task",
                "status": "completed",
                "summary": "state summary",
                "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7, "cost": 0.0, "steps": 1, "latency_ms": 5},
                "results": {"worker": {"status": "completed", "summary": "done"}},
                "trace_events": [{"seq": 1, "name": "swarm.run.started"}, {"seq": 2, "name": "swarm.run.finished"}],
            }
        ),
        encoding="utf-8",
    )


def _write_handoff(root: Path, run_id: str) -> None:
    run_dir = root / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "team-handoff.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "run_id": run_id,
                "task_id": "task",
                "status": "completed",
                "runner_ids": ["worker"],
                "reusable_runner_ids": ["worker"],
                "pending_runner_ids": [],
                "runners": [{"runner_id": "worker", "status": "completed", "reusable": True}],
            }
        ),
        encoding="utf-8",
    )


def _get_json(url: str):
    with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310 - local test server.
        assert "application/json" in response.headers["content-type"]
        return json.loads(response.read().decode("utf-8"))


def _get_text(url: str):
    with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310 - local test server.
        return response.read().decode("utf-8"), {str(key).lower(): str(value) for key, value in response.headers.items()}


def _get_json_error(url: str):
    try:
        _get_json(url)
    except urllib.error.HTTPError as error:
        try:
            return error.code, json.loads(error.read().decode("utf-8"))
        finally:
            error.close()
    raise AssertionError("expected HTTPError")


if __name__ == "__main__":
    unittest.main()
