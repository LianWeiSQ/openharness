from __future__ import annotations

import unittest
from pathlib import Path

from openagent.core.execution.runtime import ExecutionBinding, OpenSandboxWorkspaceRuntime, execution_binding_from_session
from openagent.core.session.session import Session


class ExecutionRuntimeTests(unittest.TestCase):
    def test_sandbox_execution_metadata_omits_connection_details(self) -> None:
        runtime = OpenSandboxWorkspaceRuntime(
            ExecutionBinding(
                mode="opensandbox",
                sandbox_id="sbx_public",
                remote_workdir="/workspace/project",
                connection={
                    "api_key": "redacted",
                    "headers": {"Authorization": "redacted"},
                    "domain": "https://sandbox.example.test",
                },
            )
        )

        metadata = runtime.execution_metadata

        self.assertEqual(metadata["execution_mode"], "opensandbox")
        self.assertEqual(metadata["sandbox_id"], "sbx_public")
        self.assertEqual(metadata["remote_workdir"], "/workspace/project")
        self.assertNotIn("connection", metadata)
        self.assertNotIn("redacted", str(metadata))

    def test_execution_binding_normalizes_remote_workdir(self) -> None:
        session = Session(directory=Path("."))
        session.metadata["execution"] = {
            "mode": "opensandbox",
            "sandbox_id": "sbx_public",
            "remote_workdir": "/workspace/project/../project",
            "connection": {"api_key": "redacted"},
        }

        binding = execution_binding_from_session(session)

        self.assertEqual(binding.mode, "opensandbox")
        self.assertEqual(binding.sandbox_id, "sbx_public")
        self.assertEqual(binding.remote_workdir, "/workspace/project")
        self.assertEqual(binding.connection["api_key"], "redacted")


if __name__ == "__main__":
    unittest.main()
