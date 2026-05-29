from __future__ import annotations

import json
import re
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.integrations.terminal_bench import (
    EVENTS_FILENAME,
    FINAL_ANSWER_FILENAME,
    OpenAgentTerminalBenchAgent,
    TerminalBenchWorkspaceRuntime,
)

from _mock_model import ScriptedLanguageModel


class FakeTmuxSession:
    def __init__(self) -> None:
        self.commands: list[object] = []
        self.output = ""

    def send_command(self, command: object) -> None:
        self.commands.append(command)
        command_text = str(getattr(command, "command"))
        marker_match = re.search(r"(__OPENAGENT_TBENCH_EXIT_[0-9a-f]+__)", command_text)
        marker = marker_match.group(1) if marker_match else "__OPENAGENT_TBENCH_EXIT_missing__"
        if "rm " in command_text:
            body = "removed fixture"
        else:
            body = "hello from tmux"
        self.output = f"$ {command_text}\n{body}\n{marker}0\n"

    def get_incremental_output(self) -> str:
        return self.output


class TerminalBenchAdapterTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("src/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"terminal_bench_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_runtime_sends_bash_command_to_tmux_session(self) -> None:
        tmux = FakeTmuxSession()
        runtime = TerminalBenchWorkspaceRuntime(tmux, workspace_root="/app")

        result = await runtime.run_command("echo hello", cwd="/app/project", timeout_ms=5000)

        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.cwd, "/app/project")
        self.assertIn("hello from tmux", result.stdout)
        self.assertIn("exit_code=0", result.stdout)
        self.assertEqual(len(tmux.commands), 1)
        sent = getattr(tmux.commands[0], "command")
        self.assertIn("echo hello", sent)
        self.assertIn("cd /app/project", sent)

    def test_agent_returns_token_counts_and_writes_logs(self) -> None:
        temp = self._make_temp_dir()
        tmux = FakeTmuxSession()
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
        agent = OpenAgentTerminalBenchAgent(language_model=model, max_steps=4)

        result = agent.perform_task("Create a hello file.", tmux, logging_dir=temp)

        self.assertEqual(result.total_input_tokens, 6)
        self.assertEqual(result.total_output_tokens, 8)
        self.assertEqual(getattr(result.failure_mode, "value", result.failure_mode), "none")
        self.assertEqual((temp / FINAL_ANSWER_FILENAME).read_text(encoding="utf-8"), "done")
        events = [
            json.loads(line)
            for line in (temp / EVENTS_FILENAME).read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        self.assertTrue(any(event.get("type") == "tool-result" for event in events))
        self.assertEqual(len(tmux.commands), 1)

    def test_agent_allows_destructive_commands_in_terminal_bench_mode(self) -> None:
        tmux = FakeTmuxSession()
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
        agent = OpenAgentTerminalBenchAgent(language_model=model, max_steps=4)

        result = agent.perform_task("Remove a file.", tmux, logging_dir=None)

        self.assertEqual(getattr(result.failure_mode, "value", result.failure_mode), "none")
        self.assertEqual(len(tmux.commands), 1)
        self.assertIn("rm -f /app/tmp.txt", getattr(tmux.commands[0], "command"))


if __name__ == "__main__":
    unittest.main()
