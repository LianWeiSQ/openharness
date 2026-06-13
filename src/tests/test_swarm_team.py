from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import (
    AgentResult,
    FileTeamHandoffStore,
    SwarmRunResult,
    Usage,
    build_team_handoff,
    load_swarm_config,
    task_for_team_handoff_resume,
    team_handoff_from_dict,
)


class SwarmTeamHandoffTests(unittest.TestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_team_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def test_build_team_handoff_marks_reusable_pending_and_missing_runners(self) -> None:
        config = _config(["alpha", "beta", "gamma"])
        result = SwarmRunResult(
            task_id="task",
            status="partial",
            summary="partial team",
            results={
                "alpha": AgentResult(
                    status="completed",
                    summary="alpha done",
                    evidence=["alpha.py:1"],
                    usage=Usage(input_tokens=3),
                    metadata={"session_id": "session-alpha"},
                ),
                "beta": AgentResult(status="failed", summary="beta failed"),
            },
            warnings=["team incomplete"],
        )

        handoff = build_team_handoff(task=config.task("task"), result=result, run_id="team-run")

        self.assertTrue(handoff.has_pending)
        self.assertEqual(handoff.reusable_runner_ids, ["alpha"])
        self.assertEqual(handoff.pending_runner_ids, ["beta", "gamma"])
        self.assertEqual(handoff.failed_runner_ids, ["beta"])
        self.assertEqual(handoff.missing_runner_ids, ["gamma"])
        self.assertEqual(handoff.runners[0].metadata["session_id"], "session-alpha")
        self.assertEqual(handoff.task_contract["objective"], "Run a resumable team.")

    def test_build_team_handoff_marks_all_completed_as_terminal(self) -> None:
        config = _config(["alpha", "beta"])
        result = SwarmRunResult(
            task_id="task",
            status="completed",
            summary="done",
            results={
                "alpha": AgentResult(status="completed", summary="alpha done"),
                "beta": AgentResult(status="completed", summary="beta done"),
            },
        )

        handoff = build_team_handoff(task=config.task("task"), result=result, run_id="team-run")

        self.assertFalse(handoff.has_pending)
        self.assertEqual(handoff.reusable_runner_ids, ["alpha", "beta"])
        self.assertEqual(handoff.pending_runner_ids, [])
        self.assertEqual(handoff.status, "completed")

    def test_file_team_handoff_store_roundtrips_manifest(self) -> None:
        temp = self._workspace()
        store = FileTeamHandoffStore(temp / "handoffs")
        config = _config(["alpha"])
        result = SwarmRunResult(
            task_id="task",
            status="completed",
            summary="done",
            results={"alpha": AgentResult(status="completed", summary="alpha done")},
        )
        handoff = build_team_handoff(task=config.task("task"), result=result, run_id="team/run:1")

        payload = store.save_handoff(handoff)
        loaded = store.load_handoff("team/run:1")

        self.assertTrue(store.handoff_path("team/run:1").exists())
        self.assertEqual(payload["run_id"], "team/run:1")
        self.assertEqual(loaded.run_id, "team/run:1")
        self.assertEqual(loaded.reusable_runner_ids, ["alpha"])
        self.assertEqual(loaded.as_dict(), team_handoff_from_dict(payload).as_dict())

    def test_task_for_team_handoff_resume_enables_resume_and_can_target_pending_runners(self) -> None:
        config = _config(["alpha", "beta"])
        task = config.task("task")
        result = SwarmRunResult(
            task_id="task",
            status="partial",
            summary="partial",
            results={
                "alpha": AgentResult(status="completed", summary="alpha done"),
                "beta": AgentResult(status="partial", summary="beta needs follow-up"),
            },
        )
        handoff = build_team_handoff(task=task, result=result, run_id="team-run")

        full_resume = task_for_team_handoff_resume(task=task, handoff=handoff)
        pending_resume = task_for_team_handoff_resume(task=task, handoff=handoff, pending_only=True)

        self.assertEqual(full_resume.runner_ids, ["alpha", "beta"])
        self.assertEqual(pending_resume.runner_ids, ["beta"])
        self.assertTrue(full_resume.metadata["resume"]["enabled"])
        self.assertEqual(full_resume.metadata["resume"]["reuse_statuses"], ["completed"])
        self.assertEqual(full_resume.metadata["team_handoff"]["run_id"], "team-run")
        self.assertEqual(full_resume.metadata["team_handoff"]["pending_runner_ids"], ["beta"])


def _config(runners: list[str]):
    return load_swarm_config(
        {
            "runners": {runner: {"kind": "function", "roles": ["worker"]} for runner in runners},
            "tasks": {
                "task": {
                    "role": "worker",
                    "objective": "Run a resumable team.",
                    "context": "Team handoff context.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": runners,
                }
            },
        }
    )


if __name__ == "__main__":
    unittest.main()
