from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from openagent.cli.main import (
    DEFAULT_BASE_URL,
    DEFAULT_MODEL,
    DEFAULT_WIRE_API,
    apply_model_env,
    build_run_prompt,
    build_parser,
    candidate_model_urls,
    load_local_env,
    run_non_interactive,
)
from openagent.app_server.protocol import AppEvent


class OpenAgentCliTests(unittest.TestCase):
    def test_default_command_sets_gpt55_local_gateway_defaults(self) -> None:
        parser = build_parser()
        args = parser.parse_args([])

        with patch.dict(os.environ, {}, clear=True):
            apply_model_env(args)
            self.assertEqual(os.environ["OPENAI_BASE_URL"], DEFAULT_BASE_URL)
            self.assertEqual(os.environ["OPENAI_MODEL"], DEFAULT_MODEL)
            self.assertEqual(os.environ["OPENAI_WIRE_API"], DEFAULT_WIRE_API)
            self.assertEqual(os.environ["OPENAGENT_APP_MAX_STEPS"], "30")

    def test_cli_options_override_environment(self) -> None:
        parser = build_parser()
        args = parser.parse_args(
            [
                "tui",
                "--base-url",
                "http://127.0.0.1:9999",
                "--model",
                "gpt-test",
                "--wire-api",
                "chat",
                "--max-steps",
                "8",
            ]
        )

        with patch.dict(os.environ, {"OPENAI_MODEL": "env-model"}, clear=True):
            apply_model_env(args)
            self.assertEqual(os.environ["OPENAI_BASE_URL"], "http://127.0.0.1:9999")
            self.assertEqual(os.environ["OPENAI_MODEL"], "gpt-test")
            self.assertEqual(os.environ["OPENAI_WIRE_API"], "chat")
            self.assertEqual(os.environ["OPENAGENT_APP_MAX_STEPS"], "8")

    def test_candidate_model_urls_match_provider_base_url_behavior(self) -> None:
        self.assertEqual(
            candidate_model_urls("http://localhost:8080"),
            ["http://localhost:8080/v1/models", "http://localhost:8080/models"],
        )
        self.assertEqual(
            candidate_model_urls("http://localhost:8080/v1"),
            ["http://localhost:8080/v1/models"],
        )

    def test_load_local_env_sets_missing_values_only(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            config = Path(raw_tmp) / "openagent.env"
            config.write_text(
                "\n".join(
                    [
                        "OPENAI_API_KEY='from-file'",
                        "export OPENAI_MODEL=file-model",
                        "OPENAI_BASE_URL=http://localhost:9999",
                    ]
                ),
                encoding="utf-8",
            )
            with patch.dict(os.environ, {"OPENAI_MODEL": "env-model"}, clear=True):
                loaded = load_local_env(str(config))
                self.assertEqual(loaded, config)
                self.assertEqual(os.environ["OPENAI_API_KEY"], "from-file")
                self.assertEqual(os.environ["OPENAI_MODEL"], "env-model")
                self.assertEqual(os.environ["OPENAI_BASE_URL"], "http://localhost:9999")

    def test_run_command_prints_streamed_answer_text(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["run", "--skip-doctor", "hello", "agent"])
        stdout = io.StringIO()
        stderr = io.StringIO()

        exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdout=stdout, stderr=stderr)

        self.assertEqual(exit_code, 0)
        self.assertEqual(stdout.getvalue(), "hello from openagent\n")
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(FakeRuntime.last_prompt, "hello agent")

    def test_run_command_can_emit_json_events(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["run", "--skip-doctor", "--format", "json", "hello"])
        stdout = io.StringIO()

        exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdout=stdout, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        events = [json.loads(line) for line in stdout.getvalue().splitlines()]
        self.assertEqual([event["method"] for event in events], ["item/agentMessage/delta", "turn/completed"])

    def test_run_command_can_continue_latest_session(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["run", "--skip-doctor", "--continue", "hello"])

        exit_code = run_non_interactive(args, runtime_factory=FakeRuntimeWithSession, stdout=io.StringIO(), stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        self.assertEqual(FakeRuntimeWithSession.resumed_session_id, "session_existing")

    def test_run_command_reads_prompt_from_stdin(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["run", "--skip-doctor"])
        stdin = FakeStdin("stdin prompt")

        exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdin=stdin, stdout=io.StringIO(), stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        self.assertEqual(FakeRuntime.last_prompt, "stdin prompt")

    def test_run_command_requires_prompt_or_stdin(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["run", "--skip-doctor"])
        stderr = io.StringIO()

        exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdin=FakeStdin("", is_tty=True), stdout=io.StringIO(), stderr=stderr)

        self.assertEqual(exit_code, 2)
        self.assertIn("requires a prompt", stderr.getvalue())

    def test_build_run_prompt_attaches_files(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            workspace = Path(raw_tmp)
            target = workspace / "note.txt"
            target.write_text("important context", encoding="utf-8")

            prompt = build_run_prompt("review this", files=["note.txt"], workspace=workspace)

        self.assertIn("review this", prompt)
        self.assertIn("Attached file:", prompt)
        self.assertIn("important context", prompt)


class FakeTurn:
    def __init__(self) -> None:
        self.status = "completed"
        self.final_answer = "hello from openagent"
        self.error = None
        self.events = [
            AppEvent(
                sequence=1,
                method="item/agentMessage/delta",
                params={"event": {"type": "text-delta", "text": "hello from openagent"}},
            ),
            AppEvent(
                sequence=2,
                method="turn/completed",
                params={"final_answer": "hello from openagent"},
            ),
        ]

    def wait_for_sequence(self, sequence: int, *, timeout_s: float) -> AppEvent | None:
        del timeout_s
        if sequence <= len(self.events):
            return self.events[sequence - 1]
        return None


class FakeRuntime:
    last_prompt: str | None = None

    def __init__(self, *, workspace: Path, session_store_root: str | None) -> None:
        self.workspace = workspace
        self.session_store_root = session_store_root

    def start_session(self, *, cwd: Path) -> dict[str, object]:
        return {"id": "session_new", "directory": str(cwd)}

    def resume_session(self, session_id: str) -> dict[str, object]:
        return {"id": session_id}

    def list_sessions(self) -> list[dict[str, object]]:
        return []

    def start_turn(self, *, session_id: str, user_text: str) -> FakeTurn:
        self.__class__.last_prompt = user_text
        self.session_id = session_id
        return FakeTurn()


class FakeRuntimeWithSession(FakeRuntime):
    resumed_session_id: str | None = None

    def list_sessions(self) -> list[dict[str, object]]:
        return [{"id": "session_existing"}]

    def resume_session(self, session_id: str) -> dict[str, object]:
        self.__class__.resumed_session_id = session_id
        return {"id": session_id}


class FakeStdin:
    def __init__(self, value: str, *, is_tty: bool = False) -> None:
        self.value = value
        self._is_tty = is_tty

    def isatty(self) -> bool:
        return self._is_tty

    def read(self) -> str:
        return self.value


if __name__ == "__main__":
    unittest.main()
