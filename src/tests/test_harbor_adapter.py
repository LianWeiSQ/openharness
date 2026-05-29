from __future__ import annotations

import json
import shutil
import types
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.integrations.harbor import (
    EVENTS_FILENAME,
    FINAL_ANSWER_FILENAME,
    HarborWorkspaceRuntime,
    OpenAgentHarborAgent,
)

from _mock_model import ScriptedLanguageModel


class FakeHarborEnvironment:
    def __init__(self) -> None:
        self.commands: list[dict[str, object]] = []

    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout_sec: int | None = None,
        user: str | int | None = None,
    ) -> object:
        self.commands.append(
            {
                "command": command,
                "cwd": cwd,
                "env": env,
                "timeout_sec": timeout_sec,
                "user": user,
            }
        )
        output = "removed fixture" if "rm " in command else "hello from harbor"
        return types.SimpleNamespace(stdout=output, stderr="", return_code=0)


class HarborAdapterTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("src/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"harbor_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_runtime_executes_bash_in_harbor_environment(self) -> None:
        environment = FakeHarborEnvironment()
        runtime = HarborWorkspaceRuntime(environment, workspace_root="/app")

        result = await runtime.run_command("echo hello", cwd="/app/project", timeout_ms=5000)

        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.cwd, "/app/project")
        self.assertIn("hello from harbor", result.stdout)
        self.assertIn("exit_code=0", result.stdout)
        self.assertEqual(environment.commands[0]["command"], "echo hello")
        self.assertEqual(environment.commands[0]["cwd"], "/app/project")
        self.assertEqual(environment.commands[0]["timeout_sec"], 5)

    async def test_agent_populates_context_and_writes_logs(self) -> None:
        temp = self._make_temp_dir()
        environment = FakeHarborEnvironment()
        context = types.SimpleNamespace(
            n_input_tokens=None,
            n_cache_tokens=None,
            n_output_tokens=None,
            cost_usd=None,
            rollout_details=None,
            metadata=None,
        )
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "bash1",
                        "name": "bash",
                        "input": {"command": "echo hello", "timeout": 5000},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 2, "output_tokens": 3}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 4, "output_tokens": 5}},
                ],
            ]
        )
        agent = OpenAgentHarborAgent(logs_dir=temp, model_name="OpenAI/gpt-test", language_model=model, max_steps=4)

        await agent.run("Create a hello file.", environment, context)

        self.assertEqual(context.n_input_tokens, 6)
        self.assertEqual(context.n_output_tokens, 8)
        self.assertEqual(context.cost_usd, 0.0)
        self.assertEqual(context.metadata["failure_mode"], "none")
        self.assertEqual(context.metadata["model"], "gpt-test")
        self.assertEqual((temp / FINAL_ANSWER_FILENAME).read_text(encoding="utf-8"), "done")
        events = [
            json.loads(line)
            for line in (temp / EVENTS_FILENAME).read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        self.assertTrue(any(event.get("type") == "tool-result" for event in events))
        self.assertEqual(environment.commands[0]["command"], "echo hello")

    async def test_agent_allows_destructive_commands_in_harbor_mode(self) -> None:
        temp = self._make_temp_dir()
        environment = FakeHarborEnvironment()
        context = types.SimpleNamespace(metadata=None, n_input_tokens=None, n_output_tokens=None, cost_usd=None)
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "bash1",
                        "name": "bash",
                        "input": {"command": "rm -f /app/tmp.txt", "timeout": 5000},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "removed"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1}},
                ],
            ]
        )
        agent = OpenAgentHarborAgent(logs_dir=temp, language_model=model, max_steps=4)

        await agent.run("Remove a file.", environment, context)

        self.assertEqual(context.metadata["failure_mode"], "none")
        self.assertEqual(environment.commands[0]["command"], "rm -f /app/tmp.txt")


if __name__ == "__main__":
    unittest.main()
