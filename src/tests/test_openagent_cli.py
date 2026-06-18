from __future__ import annotations

import io
import json
import os
import stat
import sys
import tempfile
import threading
import urllib.error
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from mcp.shared.auth import OAuthClientInformationFull, OAuthToken

from openagent.cli.main import (
    DEFAULT_BASE_URL,
    DEFAULT_MODEL,
    DEFAULT_WIRE_API,
    apply_model_env,
    build_run_prompt,
    build_parser,
    candidate_model_urls,
    load_local_env,
    run_auth_command,
    run_attach_command,
    run_client_command,
    run_config_command,
    run_doctor_command,
    run_mcp_command,
    run_models_command,
    run_custom_command,
    run_non_interactive,
    run_serve,
    run_session_command,
    run_stats_command,
)
from openagent.app_server.protocol import AppEvent
from openagent.cli.auth import load_auth_env, load_auth_file, normalize_provider
from openagent.core.session.session import Session
from openagent.core.session.store import FileSessionStore
from openagent.core.types import ChatMessage


class FakeHTTPResponse:
    def __init__(self, payload: dict[str, object]) -> None:
        self.data = json.dumps(payload).encode("utf-8")

    def __enter__(self) -> "FakeHTTPResponse":
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        return None

    def read(self, size: int = -1) -> bytes:
        return self.data if size < 0 else self.data[:size]


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

    def test_doctor_default_text_output(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["doctor"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAI_BASE_URL": "http://gateway.test",
                "OPENAI_MODEL": "gpt-test",
                "OPENAI_WIRE_API": "chat",
            },
            clear=True,
        ), patch(
            "openagent.cli.main.check_models_endpoint",
            return_value=(True, "http://gateway.test/v1/models"),
        ) as check_endpoint:
            exit_code = run_doctor_command(args, stdout=stdout)

        self.assertEqual(exit_code, 0)
        self.assertEqual(args.format, "text")
        check_endpoint.assert_called_once_with(base_url="http://gateway.test")
        self.assertEqual(
            stdout.getvalue(),
            "\n".join(
                [
                    "OpenAgent doctor",
                    "- provider: openai (OpenAI)",
                    "- OPENAI_BASE_URL: http://gateway.test",
                    "- OPENAI_MODEL: gpt-test",
                    "- OPENAI_WIRE_API: chat",
                    "- OPENAI_API_KEY: missing",
                    "- model endpoint: ok (http://gateway.test/v1/models)",
                    "",
                ]
            ),
        )

    def test_doctor_json_output_is_machine_readable(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["doctor", "--format", "json"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAI_API_KEY": "private-key",
                "OPENAI_BASE_URL": "http://gateway.test",
                "OPENAI_MODEL": "gpt-test",
                "OPENAI_WIRE_API": "responses",
            },
            clear=True,
        ), patch(
            "openagent.cli.main.check_models_endpoint",
            return_value=(False, "connection refused"),
        ):
            exit_code = run_doctor_command(args, stdout=stdout)

        payload = json.loads(stdout.getvalue())
        self.assertEqual(exit_code, 2)
        self.assertEqual(
            payload,
            {
                "provider": "openai",
                "provider_label": "OpenAI",
                "base_url": "http://gateway.test",
                "model": "gpt-test",
                "wire_api": "responses",
                "api_key_env": "OPENAI_API_KEY",
                "api_key_set": True,
                "native": False,
                "healthy": False,
                "dependency_checked": False,
                "dependency_ok": True,
                "dependency_message": None,
                "model_endpoint_checked": True,
                "model_endpoint_ok": False,
                "model_endpoint_message": "connection refused",
            },
        )
        self.assertNotIn("private-key", stdout.getvalue())

    def test_doctor_anthropic_json_skips_openai_models_probe(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["doctor", "--format", "json"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAGENT_PROVIDER": "anthropic",
                "ANTHROPIC_API_KEY": "anthropic-private-key",
                "ANTHROPIC_MODEL": "claude-test",
            },
            clear=True,
        ), patch("openagent.cli.main.check_models_endpoint") as check_endpoint:
            with patch("openagent.cli.main._native_provider_dependency_status", return_value=(True, "optional dependency 'anthropic' is installed")):
                apply_model_env(args)
                exit_code = run_doctor_command(args, stdout=stdout)

        payload = json.loads(stdout.getvalue())
        self.assertEqual(exit_code, 0)
        self.assertEqual(payload["provider"], "anthropic")
        self.assertEqual(payload["provider_label"], "Anthropic")
        self.assertEqual(payload["model"], "claude-test")
        self.assertEqual(payload["wire_api"], "messages")
        self.assertEqual(payload["api_key_env"], "ANTHROPIC_API_KEY")
        self.assertEqual(payload["api_key_set"], True)
        self.assertEqual(payload["native"], True)
        self.assertEqual(payload["healthy"], True)
        self.assertEqual(payload["dependency_checked"], True)
        self.assertEqual(payload["dependency_ok"], True)
        self.assertEqual(payload["dependency_message"], "optional dependency 'anthropic' is installed")
        self.assertEqual(payload["model_endpoint_checked"], False)
        self.assertEqual(payload["model_endpoint_ok"], True)
        self.assertIn("skipped", payload["model_endpoint_message"])
        self.assertNotIn("anthropic-private-key", stdout.getvalue())
        check_endpoint.assert_not_called()

    def test_doctor_anthropic_json_fails_when_optional_sdk_is_missing(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["doctor", "--format", "json"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAGENT_PROVIDER": "anthropic",
                "ANTHROPIC_API_KEY": "anthropic-private-key",
                "ANTHROPIC_MODEL": "claude-test",
            },
            clear=True,
        ), patch("openagent.cli.main.check_models_endpoint") as check_endpoint:
            with patch(
                "openagent.cli.main._native_provider_dependency_status",
                return_value=(False, "optional dependency 'anthropic' is not installed"),
            ):
                apply_model_env(args)
                exit_code = run_doctor_command(args, stdout=stdout)

        payload = json.loads(stdout.getvalue())
        self.assertEqual(exit_code, 2)
        self.assertEqual(payload["provider"], "anthropic")
        self.assertEqual(payload["model"], "claude-test")
        self.assertEqual(payload["api_key_set"], True)
        self.assertEqual(payload["native"], True)
        self.assertEqual(payload["healthy"], False)
        self.assertEqual(payload["dependency_checked"], True)
        self.assertEqual(payload["dependency_ok"], False)
        self.assertEqual(payload["dependency_message"], "optional dependency 'anthropic' is not installed")
        self.assertEqual(payload["model_endpoint_checked"], False)
        self.assertEqual(payload["model_endpoint_ok"], False)
        self.assertIn("not installed", payload["model_endpoint_message"])
        self.assertNotIn("anthropic-private-key", stdout.getvalue())
        check_endpoint.assert_not_called()

    def test_apply_model_env_uses_native_provider_env_for_anthropic(self) -> None:
        parser = build_parser()
        args = parser.parse_args(
            [
                "doctor",
                "--api-key",
                "cli-key",
                "--base-url",
                "https://anthropic.test",
                "--model",
                "claude-cli",
                "--wire-api",
                "chat",
            ]
        )

        with patch.dict(os.environ, {"OPENAGENT_PROVIDER": "anthropic"}, clear=True):
            apply_model_env(args)

            self.assertEqual(os.environ["ANTHROPIC_API_KEY"], "cli-key")
            self.assertEqual(os.environ["ANTHROPIC_BASE_URL"], "https://anthropic.test")
            self.assertEqual(os.environ["ANTHROPIC_MODEL"], "claude-cli")
            self.assertEqual(os.environ["ANTHROPIC_WIRE_API"], "chat")
            self.assertNotIn("OPENAI_BASE_URL", os.environ)
            self.assertNotIn("OPENAI_MODEL", os.environ)

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

    def test_session_list_export_stats_and_delete_use_file_store(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            root = Path(raw_tmp) / "sessions"
            session = create_persisted_session(root)

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["session", "list", "--session-root", str(root), "--format", "json"])
            self.assertEqual(run_session_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            listed = json.loads(list_stdout.getvalue())
            self.assertEqual(listed["sessions"][0]["session_id"], session.id)
            self.assertEqual(listed["sessions"][0]["message_count"], 1)

            export_stdout = io.StringIO()
            export_args = parser.parse_args(["session", "export", "--session-root", str(root), session.id, "--sanitize"])
            self.assertEqual(run_session_command(export_args, stdout=export_stdout, stderr=io.StringIO()), 0)
            exported = json.loads(export_stdout.getvalue())
            self.assertEqual(exported["schema_version"], "openagent.session_export.v1")
            self.assertEqual(exported["session"]["messages"][0]["content"], "[redacted]")
            self.assertEqual(exported["session"]["workspace"], "[redacted]")

            stats_stdout = io.StringIO()
            stats_args = parser.parse_args(["stats", "--session-root", str(root), "--format", "json"])
            self.assertEqual(run_stats_command(stats_args, stdout=stats_stdout), 0)
            stats = json.loads(stats_stdout.getvalue())
            self.assertEqual(stats["session_count"], 1)
            self.assertEqual(stats["run_count"], 1)
            self.assertEqual(stats["total_input_tokens"], 12)
            self.assertEqual(stats["total_output_tokens"], 5)

            delete_stdout = io.StringIO()
            delete_args = parser.parse_args(["session", "delete", "--session-root", str(root), session.id])
            self.assertEqual(run_session_command(delete_args, stdout=delete_stdout, stderr=io.StringIO()), 0)
            self.assertFalse((root / session.id).exists())

    def test_models_command_lists_runtime_models(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["models", "--format", "json"])
        stdout = io.StringIO()

        exit_code = run_models_command(args, runtime_factory=FakeModelRuntime, stdout=stdout, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["models"][0]["id"], "gpt-test")

    def test_session_delete_rejects_path_traversal(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            root = Path(raw_tmp) / "sessions"
            outside = Path(raw_tmp) / "outside"
            outside.mkdir()
            stderr = io.StringIO()
            args = parser.parse_args(["session", "delete", "--session-root", str(root), "../outside"])

            exit_code = run_session_command(args, stdout=io.StringIO(), stderr=stderr)

            self.assertEqual(exit_code, 2)
            self.assertTrue(outside.exists())
            self.assertIn("Invalid session id", stderr.getvalue())

    def test_custom_command_list_show_and_render(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            workspace = Path(raw_tmp)
            write_command(
                workspace,
                "review",
                """---
description: Review a target
model: gpt-command
---
Review $1 with $ARGUMENTS.

Recent:
!`printf shell-ok`

Read @note.txt
""",
            )
            (workspace / "note.txt").write_text("file context", encoding="utf-8")

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["command", "list", "--workspace", str(workspace), "--format", "json"])
            self.assertEqual(run_custom_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            listed = json.loads(list_stdout.getvalue())
            self.assertEqual(listed["commands"][0]["name"], "review")
            self.assertEqual(listed["commands"][0]["model"], "gpt-command")

            show_stdout = io.StringIO()
            show_args = parser.parse_args(["command", "show", "--workspace", str(workspace), "review", "--format", "json"])
            self.assertEqual(run_custom_command(show_args, stdout=show_stdout, stderr=io.StringIO()), 0)
            shown = json.loads(show_stdout.getvalue())
            self.assertIn("Review $1", shown["template"])

            render_stdout = io.StringIO()
            render_args = parser.parse_args(["command", "render", "--workspace", str(workspace), "review", "README.md"])
            self.assertEqual(run_custom_command(render_args, stdout=render_stdout, stderr=io.StringIO()), 0)
            rendered = render_stdout.getvalue()
            self.assertIn("Review README.md with README.md.", rendered)
            self.assertIn("shell-ok", rendered)
            self.assertIn("file context", rendered)

    def test_run_command_can_use_custom_command_template(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            workspace = Path(raw_tmp)
            write_command(
                workspace,
                "component",
                """---
description: Create component
model: gpt-command
---
Create component $1 for $ARGUMENTS.
""",
            )
            args = parser.parse_args(["run", "--skip-doctor", "--workspace", str(workspace), "--command", "component", "Button"])

            with patch.dict(os.environ, {}, clear=True):
                exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdout=io.StringIO(), stderr=io.StringIO())
                self.assertEqual(os.environ["OPENAI_MODEL"], "gpt-command")

            self.assertEqual(exit_code, 0)
            self.assertEqual(FakeRuntime.last_prompt, "Create component Button for Button.")

    def test_run_command_missing_custom_command_is_clean_error(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            stderr = io.StringIO()
            args = parser.parse_args(["run", "--skip-doctor", "--workspace", raw_tmp, "--command", "missing", "arg"])

            exit_code = run_non_interactive(args, runtime_factory=FakeRuntime, stdout=io.StringIO(), stderr=stderr)

        self.assertEqual(exit_code, 1)
        self.assertIn("Command not found", stderr.getvalue())

    def test_auth_login_list_env_and_logout(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            login_args = parser.parse_args(
                [
                    "auth",
                    "login",
                    "--auth-file",
                    str(auth_file),
                    "--api-key",
                    "test-secret",
                    "--base-url",
                    "http://localhost:8080",
                    "--model",
                    "gpt-auth",
                    "--wire-api",
                    "responses",
                ]
            )
            login_stdout = io.StringIO()

            self.assertEqual(run_auth_command(login_args, stdout=login_stdout, stderr=io.StringIO()), 0)
            login_payload = json.loads(login_stdout.getvalue())
            self.assertEqual(login_payload["status"], "logged_in")
            self.assertEqual(login_payload["record"]["api_key"], "test****cret")
            self.assertEqual(stat.S_IMODE(auth_file.stat().st_mode), 0o600)

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["auth", "list", "--auth-file", str(auth_file), "--format", "json"])
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run_auth_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            listed = json.loads(list_stdout.getvalue())
            self.assertEqual(listed["providers"][0]["model"], "gpt-auth")
            self.assertNotIn("test-secret", list_stdout.getvalue())

            with patch.dict(os.environ, {}, clear=True):
                loaded = load_auth_env(str(auth_file))
                self.assertEqual(loaded, auth_file)
                self.assertEqual(os.environ["OPENAI_API_KEY"], "test-secret")
                self.assertEqual(os.environ["OPENAI_BASE_URL"], "http://localhost:8080")
                self.assertEqual(os.environ["OPENAI_MODEL"], "gpt-auth")
                self.assertEqual(os.environ["OPENAI_WIRE_API"], "responses")
                self.assertEqual(os.environ["OPENAGENT_ACTIVE_PROVIDER"], "openai")

            logout_stdout = io.StringIO()
            logout_args = parser.parse_args(["auth", "logout", "--auth-file", str(auth_file)])
            self.assertEqual(run_auth_command(logout_args, stdout=logout_stdout, stderr=io.StringIO()), 0)
            self.assertEqual(json.loads(logout_stdout.getvalue())["removed"], True)

    def test_auth_provider_id_normalization_and_invalid_ids(self) -> None:
        self.assertEqual(normalize_provider("Anthropic.US-East_1"), "anthropic.us-east_1")

        for provider in ["", "  ", ".bad", "-bad", "bad provider", "bad/provider"]:
            with self.subTest(provider=provider):
                with self.assertRaisesRegex(ValueError, "Invalid provider id"):
                    normalize_provider(provider)

        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            stderr = io.StringIO()
            args = parser.parse_args(["auth", "login", "--auth-file", str(auth_file), "--provider", "bad/provider", "--api-key", "secret"])

            self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=stderr), 2)
            self.assertIn("Invalid provider id", stderr.getvalue())
            self.assertFalse(auth_file.exists())

            stderr = io.StringIO()
            empty_args = parser.parse_args(["auth", "login", "--auth-file", str(auth_file), "--provider", "", "--api-key", "secret"])
            self.assertEqual(run_auth_command(empty_args, stdout=io.StringIO(), stderr=stderr), 2)
            self.assertIn("Invalid provider id", stderr.getvalue())
            self.assertFalse(auth_file.exists())

    def test_wellknown_login_validates_url_before_fetch(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            for url in [
                "file:///tmp/provider",
                "javascript:alert(1)",
                "data:text/plain,hello",
                "ftp://provider.test",
                "https:///missing-host",
                "https://user:pass@provider.test",
                "https://provider.test?x=1",
                "https://provider.test#token",
                "https://provider.test:99999",
                "https://127.0.0.1",
                "https://10.0.0.1",
                "http://provider.test",
            ]:
                with self.subTest(url=url), patch("openagent.cli.wellknown.open_wellknown_request") as open_request:
                    stderr = io.StringIO()
                    args = parser.parse_args(["providers", "login", url, "--auth-file", str(auth_file), "--provider", "wellknown-test"])

                    self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=stderr), 2)
                    open_request.assert_not_called()
                    self.assertFalse(auth_file.exists())

    def test_wellknown_login_previews_command_without_execution_by_default(self) -> None:
        parser = build_parser()
        opaque = "sk-1234567890abcdef1234567890"
        metadata = {
            "auth": {
                "command": [
                    "provider-login",
                    "--credential",
                    opaque,
                    "-H",
                    "Authorization: Basic abcdef123456",
                    "--private-key=key-value",
                    "--token",
                    "plain-token-value",
                ],
                "env": "WELLKNOWN_API_KEY",
            },
            "config": {
                "provider": {
                    "local-provider": {
                        "options": {"baseURL": "http://127.0.0.1:49153/v1", "wireAPI": "responses"},
                        "models": {"local-model": {}},
                    }
                }
            },
        }

        def fake_open_request(request: object, timeout_s: float = 0) -> FakeHTTPResponse:
            self.assertEqual(getattr(request, "full_url"), "http://127.0.0.1:49152/.well-known/opencode")
            return FakeHTTPResponse(metadata)

        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            args = parser.parse_args(
                [
                    "providers",
                    "login",
                    "http://127.0.0.1:49152",
                    "--allow-insecure-localhost",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "local-provider",
                ]
            )
            stdout = io.StringIO()

            with patch("openagent.cli.wellknown.open_wellknown_request", side_effect=fake_open_request):
                self.assertEqual(run_auth_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["status"], "command_preview")
            self.assertEqual(payload["provider"], "local-provider")
            self.assertEqual(payload["wellknown_url"], "http://127.0.0.1:49152/.well-known/opencode")
            self.assertEqual(payload["base_url"], "http://127.0.0.1:49153/v1")
            self.assertEqual(payload["model"], "local-model")
            self.assertEqual(payload["wire_api"], "responses")
            self.assertEqual(payload["auth_env"], "WELLKNOWN_API_KEY")
            self.assertFalse(payload["executed"])
            self.assertNotIn(opaque, stdout.getvalue())
            self.assertNotIn("abcdef123456", stdout.getvalue())
            self.assertNotIn("key-value", stdout.getvalue())
            self.assertNotIn("plain-token-value", stdout.getvalue())
            self.assertFalse(auth_file.exists())

    def test_wellknown_login_stores_user_supplied_token_and_redacts_output(self) -> None:
        parser = build_parser()
        secret = "wellknown-token-secret"
        metadata = {
            "auth": {"command": [sys.executable, "-c", f"print({secret!r})"], "env": "WELLKNOWN_API_KEY"},
            "config": {
                "provider": {
                    "wellknown-test": {
                        "options": {"baseURL": "https://api.provider.test/v1"},
                        "defaultModel": "provider-model",
                    }
                }
            },
        }

        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            args = parser.parse_args(
                [
                    "auth",
                    "login",
                    "http://localhost:8081/provider",
                    "--allow-insecure-localhost",
                    "--api-key",
                    secret,
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "wellknown-test",
                ]
            )
            stdout = io.StringIO()

            with patch("openagent.cli.wellknown.open_wellknown_request", return_value=FakeHTTPResponse(metadata)):
                self.assertEqual(run_auth_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            raw_output = stdout.getvalue()
            self.assertNotIn(secret, raw_output)
            payload = json.loads(raw_output)
            self.assertEqual(payload["status"], "logged_in")
            self.assertEqual(payload["record"]["type"], "wellknown")
            self.assertEqual(payload["record"]["api_key"], "well**************cret")
            self.assertEqual(payload["record"]["base_url"], "https://api.provider.test/v1")
            self.assertEqual(payload["record"]["model"], "provider-model")
            self.assertEqual(payload["record"]["wellknown_url"], "http://localhost:8081/provider/.well-known/opencode")
            self.assertEqual(payload["record"]["wellknown_provider_url"], "http://localhost:8081/provider")
            self.assertEqual(payload["record"]["auth_env"], "WELLKNOWN_API_KEY")
            self.assertIn("-c", payload["record"]["auth_command_preview"])

            stored = load_auth_file(auth_file)["providers"]["wellknown-test"]
            self.assertEqual(stored["api_key"], secret)
            self.assertEqual(stored["base_url"], "https://api.provider.test/v1")
            self.assertEqual(stored["model"], "provider-model")
            self.assertEqual(stored["type"], "wellknown")
            self.assertEqual(stored["wellknown_provider_url"], "http://localhost:8081/provider")
            self.assertEqual(stored["wellknown_url"], "http://localhost:8081/provider/.well-known/opencode")
            self.assertEqual(stored["auth_env"], "WELLKNOWN_API_KEY")
            self.assertEqual(stored["env"]["api_key"], "WELLKNOWN_API_KEY")

            with patch.dict(os.environ, {"OPENAGENT_PROVIDER": "wellknown-test"}, clear=True):
                load_auth_env(str(auth_file))
                self.assertEqual(os.environ["OPENAI_API_KEY"], secret)
                self.assertEqual(os.environ["OPENAI_BASE_URL"], "https://api.provider.test/v1")
                self.assertEqual(os.environ["OPENAI_MODEL"], "provider-model")
                self.assertEqual(os.environ["WELLKNOWN_API_KEY"], secret)

    def test_wellknown_login_rejects_invalid_metadata(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            for metadata, message in [
                ({"auth": {"command": "echo token", "env": "VALID_ENV"}}, "auth.command"),
                ({"auth": {"command": ["echo", "token"], "env": "not-valid-env"}}, "auth.env"),
            ]:
                with self.subTest(metadata=metadata), patch(
                    "openagent.cli.wellknown.open_wellknown_request",
                    return_value=FakeHTTPResponse(metadata),
                ):
                    stderr = io.StringIO()
                    args = parser.parse_args(
                        [
                            "auth",
                            "login",
                            "https://provider.test",
                            "--auth-file",
                            str(auth_file),
                            "--provider",
                            "wellknown-test",
                        ]
                    )
                    self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=stderr), 2)
                    self.assertIn(message, stderr.getvalue())
                    self.assertFalse(auth_file.exists())

    def test_wellknown_login_stdin_token_does_not_store_without_valid_metadata(self) -> None:
        parser = build_parser()
        secret = "failed-token-secret"
        metadata = {"auth": {"command": ["echo", "token"], "env": "bad-env-name"}}
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            args = parser.parse_args(
                [
                    "providers",
                    "login",
                    "http://[::1]:8081",
                    "--allow-insecure-localhost",
                    "--api-key-stdin",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "wellknown-test",
                ]
            )
            stdout = io.StringIO()
            stderr = io.StringIO()

            with patch("openagent.cli.wellknown.open_wellknown_request", return_value=FakeHTTPResponse(metadata)):
                self.assertEqual(run_auth_command(args, stdout=stdout, stderr=stderr, stdin=io.StringIO(secret)), 2)

            self.assertIn("auth.env", stderr.getvalue())
            self.assertNotIn(secret, stdout.getvalue())
            self.assertNotIn(secret, stderr.getvalue())
            self.assertFalse(auth_file.exists())

    def test_wellknown_login_rejects_metadata_redirects(self) -> None:
        parser = build_parser()
        redirect = urllib.error.HTTPError("https://provider.test/.well-known/opencode", 302, "Found", {}, io.BytesIO(b""))
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            args = parser.parse_args(
                [
                    "providers",
                    "login",
                    "https://provider.test",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "wellknown-test",
                ]
            )
            stderr = io.StringIO()

            with patch("openagent.cli.wellknown.open_wellknown_request", side_effect=redirect):
                self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=stderr), 2)

            self.assertIn("redirects are not allowed", stderr.getvalue())
            self.assertFalse(auth_file.exists())

    def test_auth_multi_provider_list_and_logout_redact_secrets(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            providers = [
                ("openai", "openai-secret", "http://openai.test/v1", "gpt-openai", "responses"),
                ("Anthropic.US", "anthropic-secret", "http://anthropic.test/v1", "claude-test", "chat"),
            ]
            for provider, key, base_url, model, wire_api in providers:
                args = parser.parse_args(
                    [
                        "auth",
                        "login",
                        "--auth-file",
                        str(auth_file),
                        "--provider",
                        provider,
                        "--type",
                        "openai-compatible",
                        "--api-key",
                        key,
                        "--base-url",
                        base_url,
                        "--model",
                        model,
                        "--wire-api",
                        wire_api,
                    ]
                )
                self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=io.StringIO()), 0)

            with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "env-secret"}, clear=True):
                json_stdout = io.StringIO()
                list_args = parser.parse_args(["auth", "list", "--auth-file", str(auth_file), "--format", "json"])
                self.assertEqual(run_auth_command(list_args, stdout=json_stdout, stderr=io.StringIO()), 0)

                raw_json = json_stdout.getvalue()
                self.assertNotIn("openai-secret", raw_json)
                self.assertNotIn("anthropic-secret", raw_json)
                self.assertNotIn("env-secret", raw_json)
                listed = json.loads(raw_json)
                self.assertEqual([row["provider"] for row in listed["providers"]], ["anthropic.us", "openai"])
                self.assertEqual(listed["providers"][0]["type"], "openai-compatible")
                self.assertEqual(listed["providers"][0]["env"]["api_key"], "ANTHROPIC_API_KEY")
                self.assertEqual(listed["providers"][0]["env_status"]["api_key"]["status"], "set")

                table_stdout = io.StringIO()
                table_args = parser.parse_args(["auth", "list", "--auth-file", str(auth_file)])
                self.assertEqual(run_auth_command(table_args, stdout=table_stdout, stderr=io.StringIO()), 0)
                table = table_stdout.getvalue()
                self.assertIn("anthropic.us", table)
                self.assertIn("openai-compatible", table)
                self.assertIn("set", table)
                self.assertNotIn("openai-secret", table)
                self.assertNotIn("anthropic-secret", table)
                self.assertNotIn("env-secret", table)

            logout_stdout = io.StringIO()
            logout_args = parser.parse_args(["auth", "logout", "--auth-file", str(auth_file), "--provider", "ANTHROPIC.US"])
            self.assertEqual(run_auth_command(logout_args, stdout=logout_stdout, stderr=io.StringIO()), 0)
            self.assertTrue(json.loads(logout_stdout.getvalue())["removed"])

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["auth", "list", "--auth-file", str(auth_file), "--format", "json"])
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run_auth_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
                self.assertEqual([row["provider"] for row in json.loads(list_stdout.getvalue())["providers"]], ["openai"])

    def test_auth_list_discovers_env_only_provider_credentials(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            secret = "openrouter-secret-value"
            with patch.dict(
                os.environ,
                {
                    "OPENROUTER_API_KEY": secret,
                    "OPENROUTER_MODEL": "openrouter/test-model",
                },
                clear=True,
            ):
                json_stdout = io.StringIO()
                json_args = parser.parse_args(["providers", "list", "--auth-file", str(auth_file), "--format", "json"])
                self.assertEqual(run_auth_command(json_args, stdout=json_stdout, stderr=io.StringIO()), 0)

                raw_json = json_stdout.getvalue()
                self.assertNotIn(secret, raw_json)
                payload = json.loads(raw_json)
                self.assertEqual([row["provider"] for row in payload["providers"]], ["openrouter"])
                row = payload["providers"][0]
                self.assertEqual(row["source"], "env")
                self.assertEqual(row["type"], "env")
                self.assertTrue(row["has_api_key"])
                self.assertEqual(row["api_key"], "")
                self.assertEqual(row["model"], "openrouter/test-model")
                self.assertEqual(row["env"]["api_key"], "OPENROUTER_API_KEY")
                self.assertEqual(row["env_status"]["api_key"]["status"], "set")
                self.assertEqual(row["methods"], ["api_key"])
                self.assertEqual(row["auth_methods"][0]["env_api_key"], "OPENROUTER_API_KEY")

                table_stdout = io.StringIO()
                table_args = parser.parse_args(["providers", "list", "--auth-file", str(auth_file)])
                self.assertEqual(run_auth_command(table_args, stdout=table_stdout, stderr=io.StringIO()), 0)
                table = table_stdout.getvalue()
                self.assertIn("openrouter", table)
                self.assertIn("set", table)
                self.assertNotIn(secret, table)

    def test_providers_methods_reports_metadata_and_unknown_fallback(self) -> None:
        parser = build_parser()
        with patch.dict(os.environ, {"OPENROUTER_API_KEY": "openrouter-secret", "CUSTOM_GATEWAY_API_KEY": "custom-secret"}, clear=True):
            json_stdout = io.StringIO()
            args = parser.parse_args(["providers", "methods", "openrouter", "--format", "json"])
            self.assertEqual(run_auth_command(args, stdout=json_stdout, stderr=io.StringIO()), 0)

            payload = json.loads(json_stdout.getvalue())
            self.assertEqual(payload["provider"], "openrouter")
            method = payload["methods"][0]
            self.assertEqual(method["id"], "api_key")
            self.assertEqual(method["status"], "set")
            self.assertEqual(method["env"]["api_key"], "OPENROUTER_API_KEY")
            self.assertEqual(method["default_base_url"], "https://openrouter.ai/api/v1")
            self.assertNotIn("openrouter-secret", json_stdout.getvalue())

            unknown_stdout = io.StringIO()
            unknown_args = parser.parse_args(["providers", "methods", "custom-gateway", "--format", "json"])
            self.assertEqual(run_auth_command(unknown_args, stdout=unknown_stdout, stderr=io.StringIO()), 0)
            unknown = json.loads(unknown_stdout.getvalue())
            self.assertEqual(unknown["provider"], "custom-gateway")
            self.assertEqual(unknown["methods"][0]["env"]["api_key"], "CUSTOM_GATEWAY_API_KEY")
            self.assertEqual(unknown["methods"][0]["status"], "set")
            self.assertNotIn("custom-secret", unknown_stdout.getvalue())

            table_stdout = io.StringIO()
            table_args = parser.parse_args(["providers", "methods", "openrouter"])
            self.assertEqual(run_auth_command(table_args, stdout=table_stdout, stderr=io.StringIO()), 0)
            self.assertIn("OPENROUTER_API_KEY", table_stdout.getvalue())

            anthropic_stdout = io.StringIO()
            anthropic_args = parser.parse_args(["providers", "methods", "anthropic", "--format", "json"])
            self.assertEqual(run_auth_command(anthropic_args, stdout=anthropic_stdout, stderr=io.StringIO()), 0)
            anthropic = json.loads(anthropic_stdout.getvalue())
            self.assertEqual(anthropic["provider"], "anthropic")
            self.assertEqual(anthropic["methods"][0]["env"]["api_key"], "ANTHROPIC_API_KEY")
            self.assertEqual(anthropic["methods"][0]["default_model"], "claude-sonnet-4-5")
            self.assertIn("Native Anthropic Messages routing", anthropic["methods"][0]["notes"])

    def test_auth_login_update_preserves_existing_key_and_type(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            create_args = parser.parse_args(
                [
                    "auth",
                    "login",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "openrouter",
                    "--type",
                    "openai-compatible",
                    "--api-key",
                    "openrouter-secret",
                    "--base-url",
                    "http://old.test/v1",
                ]
            )
            self.assertEqual(run_auth_command(create_args, stdout=io.StringIO(), stderr=io.StringIO()), 0)

            update_args = parser.parse_args(
                [
                    "auth",
                    "login",
                    "--auth-file",
                    str(auth_file),
                    "--provider",
                    "openrouter",
                    "--base-url",
                    "http://new.test/v1",
                    "--model",
                    "openrouter/model",
                ]
            )
            self.assertEqual(run_auth_command(update_args, stdout=io.StringIO(), stderr=io.StringIO()), 0)

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["auth", "list", "--auth-file", str(auth_file), "--format", "json"])
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run_auth_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            record = json.loads(list_stdout.getvalue())["providers"][0]
            self.assertEqual(record["provider"], "openrouter")
            self.assertEqual(record["type"], "openai-compatible")
            self.assertEqual(record["base_url"], "http://new.test/v1")
            self.assertEqual(record["model"], "openrouter/model")
            self.assertEqual(record["env"]["api_key"], "OPENROUTER_API_KEY")
            self.assertNotIn("openrouter-secret", list_stdout.getvalue())

            with patch.dict(os.environ, {"OPENAGENT_PROVIDER": "openrouter"}, clear=True):
                load_auth_env(str(auth_file))
                self.assertEqual(os.environ["OPENAI_API_KEY"], "openrouter-secret")
                self.assertEqual(os.environ["OPENROUTER_API_KEY"], "openrouter-secret")

    def test_load_auth_env_selects_active_provider_without_overwriting_env(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            for provider, key, base_url, model, wire_api in [
                ("openai", "openai-secret", "http://openai.test/v1", "gpt-openai", "responses"),
                ("anthropic", "anthropic-secret", "http://anthropic.test/v1", "claude-test", "chat"),
            ]:
                args = parser.parse_args(
                    [
                        "auth",
                        "login",
                        "--auth-file",
                        str(auth_file),
                        "--provider",
                        provider,
                        "--api-key",
                        key,
                        "--base-url",
                        base_url,
                        "--model",
                        model,
                        "--wire-api",
                        wire_api,
                    ]
                )
                self.assertEqual(run_auth_command(args, stdout=io.StringIO(), stderr=io.StringIO()), 0)

            with patch.dict(
                os.environ,
                {
                    "OPENAGENT_PROVIDER": "anthropic",
                    "OPENAI_API_KEY": "user-key",
                    "OPENAI_BASE_URL": "http://user.test/v1",
                },
                clear=True,
            ):
                loaded = load_auth_env(str(auth_file))
                self.assertEqual(loaded, auth_file)
                self.assertEqual(os.environ["OPENAGENT_PROVIDER"], "anthropic")
                self.assertEqual(os.environ["OPENAGENT_ACTIVE_PROVIDER"], "anthropic")
                self.assertEqual(os.environ["OPENAI_API_KEY"], "user-key")
                self.assertEqual(os.environ["OPENAI_BASE_URL"], "http://user.test/v1")
                self.assertEqual(os.environ["OPENAI_MODEL"], "claude-test")
                self.assertEqual(os.environ["OPENAI_WIRE_API"], "chat")
                self.assertEqual(os.environ["ANTHROPIC_API_KEY"], "anthropic-secret")
                self.assertEqual(os.environ["ANTHROPIC_BASE_URL"], "http://anthropic.test/v1")

            with patch.dict(os.environ, {"OPENAGENT_ACTIVE_PROVIDER": "anthropic"}, clear=True):
                load_auth_env(str(auth_file))
                self.assertEqual(os.environ["OPENAGENT_PROVIDER"], "anthropic")
                self.assertEqual(os.environ["OPENAI_API_KEY"], "anthropic-secret")

            with patch.dict(os.environ, {"OPENAGENT_PROVIDER": "missing"}, clear=True):
                load_auth_env(str(auth_file))
                self.assertNotIn("OPENAI_API_KEY", os.environ)

    def test_load_auth_env_maps_env_only_active_provider_without_overwriting_env(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            missing_auth_file = Path(raw_tmp) / "missing-auth.json"
            with patch.dict(os.environ, {}, clear=True):
                loaded = load_auth_env(str(missing_auth_file))
                self.assertIsNone(loaded)
                self.assertNotIn("OPENAI_BASE_URL", os.environ)

            with patch.dict(
                os.environ,
                {
                    "OPENAGENT_PROVIDER": "openrouter",
                    "OPENROUTER_API_KEY": "openrouter-secret",
                    "OPENROUTER_MODEL": "openrouter/model",
                    "OPENAI_API_KEY": "user-key",
                    "OPENAI_BASE_URL": "http://user.test/v1",
                },
                clear=True,
            ):
                loaded = load_auth_env(str(missing_auth_file))
                self.assertIsNone(loaded)
                self.assertEqual(os.environ["OPENAGENT_PROVIDER"], "openrouter")
                self.assertEqual(os.environ["OPENAGENT_ACTIVE_PROVIDER"], "openrouter")
                self.assertEqual(os.environ["OPENAI_API_KEY"], "user-key")
                self.assertEqual(os.environ["OPENAI_BASE_URL"], "http://user.test/v1")
                self.assertEqual(os.environ["OPENAI_MODEL"], "openrouter/model")
                self.assertEqual(os.environ["OPENROUTER_API_KEY"], "openrouter-secret")

    def test_models_command_reflects_active_provider_id_from_env(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["models", "--format", "json"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAGENT_PROVIDER": "openrouter",
                "OPENROUTER_API_KEY": "openrouter-secret",
                "OPENROUTER_MODEL": "openrouter/model",
            },
            clear=True,
        ):
            exit_code = run_models_command(args, stdout=stdout, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["models"][0]["provider_id"], "openrouter")
        self.assertEqual(payload["models"][0]["id"], "openrouter/model")
        self.assertIn("OpenRouter", payload["models"][0]["name"])
        self.assertNotIn("openrouter-secret", stdout.getvalue())

    def test_models_command_reflects_anthropic_native_provider(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["models", "--format", "json"])
        stdout = io.StringIO()

        with patch.dict(
            os.environ,
            {
                "OPENAGENT_PROVIDER": "anthropic",
                "ANTHROPIC_API_KEY": "anthropic-secret",
                "ANTHROPIC_MODEL": "claude-native",
            },
            clear=True,
        ):
            exit_code = run_models_command(args, stdout=stdout, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["models"][0]["provider_id"], "anthropic")
        self.assertEqual(payload["models"][0]["id"], "claude-native")
        self.assertIn("Anthropic", payload["models"][0]["name"])
        self.assertNotIn("anthropic-secret", stdout.getvalue())

    def test_models_command_falls_back_for_invalid_active_provider_env(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["models", "--format", "json"])
        stdout = io.StringIO()
        stderr = io.StringIO()

        with patch.dict(os.environ, {"OPENAGENT_PROVIDER": "bad provider"}, clear=True):
            exit_code = run_models_command(args, stdout=stdout, stderr=stderr)

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["models"][0]["provider_id"], "openai")
        self.assertEqual(stderr.getvalue(), "")

    def test_providers_alias_uses_auth_commands(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            login_args = parser.parse_args(["providers", "login", "--auth-file", str(auth_file), "--provider", "Groq", "--api-key", "groq-secret"])
            self.assertEqual(run_auth_command(login_args, stdout=io.StringIO(), stderr=io.StringIO()), 0)

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["providers", "list", "--auth-file", str(auth_file), "--format", "json"])
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run_auth_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            self.assertEqual(json.loads(list_stdout.getvalue())["providers"][0]["provider"], "groq")
            self.assertNotIn("groq-secret", list_stdout.getvalue())

    def test_auth_login_can_read_key_from_stdin(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            auth_file = Path(raw_tmp) / "auth.json"
            args = parser.parse_args(["auth", "login", "--auth-file", str(auth_file), "--api-key-stdin", "--model", "gpt-stdin"])
            stdout = io.StringIO()

            self.assertEqual(run_auth_command(args, stdin=FakeStdin("stdin-secret\n"), stdout=stdout, stderr=io.StringIO()), 0)

            with patch.dict(os.environ, {}, clear=True):
                load_auth_env(str(auth_file))
                self.assertEqual(os.environ["OPENAI_API_KEY"], "stdin-secret")
                self.assertEqual(os.environ["OPENAI_MODEL"], "gpt-stdin")

    def test_serve_command_passes_app_bridge_options(self) -> None:
        parser = build_parser()
        args = parser.parse_args(
            [
                "serve",
                "--host",
                "0.0.0.0",
                "--port",
                "9999",
                "--workspace",
                "/tmp/workspace",
                "--session-root",
                "/tmp/sessions",
                "--headless",
            ]
        )
        calls: list[dict[str, object]] = []

        def fake_serve(**kwargs: object) -> None:
            calls.append(kwargs)

        with patch.dict(os.environ, {}, clear=True):
            run_serve(args, serve_fn=fake_serve)

        self.assertEqual(
            calls,
            [
                {
                    "host": "0.0.0.0",
                    "port": 9999,
                    "workspace": "/tmp/workspace",
                    "session_store_root": "/tmp/sessions",
                    "serve_static": False,
                    "auth_token": None,
                }
            ],
        )

    def test_serve_command_reads_auth_token_from_env(self) -> None:
        parser = build_parser()
        args = parser.parse_args(["serve", "--headless"])
        calls: list[dict[str, object]] = []

        def fake_serve(**kwargs: object) -> None:
            calls.append(kwargs)

        with patch.dict(os.environ, {"OPENAGENT_SERVER_TOKEN": "server-secret"}, clear=True):
            run_serve(args, serve_fn=fake_serve)

        self.assertEqual(calls[0]["auth_token"], "server-secret")

    def test_client_command_sends_prompt_to_running_app_bridge(self) -> None:
        parser = build_parser()
        server = FakeAppBridgeServer()
        self.addCleanup(server.close)
        with tempfile.TemporaryDirectory() as raw_tmp:
            expected_workspace = str(Path(raw_tmp).resolve())
            args = parser.parse_args(["client", "--server-url", server.url, "--workspace", raw_tmp, "hello", "bridge"])
            stdout = io.StringIO()

            exit_code = run_client_command(args, stdout=stdout, stderr=io.StringIO(), stdin=FakeStdin("", is_tty=True))

        self.assertEqual(exit_code, 0)
        self.assertEqual(stdout.getvalue(), "hello from server\n")
        self.assertEqual(server.records[0]["path"], "/api/sessions")
        self.assertEqual(server.records[0]["payload"]["cwd"], expected_workspace)
        self.assertEqual(server.records[1]["path"], "/api/sessions/session_new/turns")
        self.assertEqual(server.records[1]["payload"]["input"], "hello bridge")

    def test_client_command_sends_bearer_token(self) -> None:
        parser = build_parser()
        server = FakeAppBridgeServer(required_token="server-secret")
        self.addCleanup(server.close)
        args = parser.parse_args(["client", "--server-url", server.url, "--server-token", "server-secret", "hello"])
        stdout = io.StringIO()

        exit_code = run_client_command(args, stdout=stdout, stderr=io.StringIO(), stdin=FakeStdin("", is_tty=True))

        self.assertEqual(exit_code, 0)
        self.assertTrue(server.records)
        self.assertTrue(all(record["authorization"] == "Bearer server-secret" for record in server.records))

    def test_client_command_can_continue_latest_server_session_and_emit_json(self) -> None:
        parser = build_parser()
        server = FakeAppBridgeServer()
        self.addCleanup(server.close)
        args = parser.parse_args(["client", "--server-url", server.url, "--continue", "--format", "json", "continue", "please"])
        stdout = io.StringIO()

        exit_code = run_client_command(args, stdout=stdout, stderr=io.StringIO(), stdin=FakeStdin("", is_tty=True))

        self.assertEqual(exit_code, 0)
        events = [json.loads(line) for line in stdout.getvalue().splitlines()]
        self.assertEqual([event["method"] for event in events], ["item/agentMessage/delta", "turn/completed"])
        self.assertEqual(server.records[0]["path"], "/api/sessions/session_existing/turns")
        self.assertEqual(server.records[0]["payload"]["input"], "continue please")

    def test_attach_command_wires_remote_runtime_to_tui_runner(self) -> None:
        parser = build_parser()
        calls: list[dict[str, object]] = []

        class FakeAttachRuntime:
            def __init__(self, *, server_url: str, workspace: Path, auth_token: str | None) -> None:
                self.server_url = server_url
                self.workspace = workspace
                self.auth_token = auth_token
                calls.append({"server_url": server_url, "workspace": workspace, "auth_token": auth_token})

            def list_sessions(self) -> list[dict[str, object]]:
                calls.append({"health_check": True})
                return [{"id": "session_latest"}]

        def fake_tui_main(argv: list[str], **kwargs: object) -> None:
            calls.append({"argv": argv, **kwargs})

        with tempfile.TemporaryDirectory() as raw_tmp, patch.dict(os.environ, {"ATTACH_TOKEN": "secret"}, clear=True):
            workspace = Path(raw_tmp).resolve()
            args = parser.parse_args(
                [
                    "attach",
                    "http://127.0.0.1:8787/",
                    "--dir",
                    raw_tmp,
                    "--session",
                    "session_123",
                    "--server-token-env",
                    "ATTACH_TOKEN",
                ]
            )

            exit_code = run_attach_command(args, tui_main=fake_tui_main, runtime_factory=FakeAttachRuntime, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        self.assertEqual(calls[0], {"server_url": "http://127.0.0.1:8787", "workspace": workspace, "auth_token": "secret"})
        self.assertEqual(calls[1], {"health_check": True})
        self.assertEqual(calls[2]["argv"], [])
        self.assertIs(calls[2]["runtime"].__class__, FakeAttachRuntime)
        self.assertEqual(calls[2]["initial_session_id"], "session_123")
        self.assertEqual(calls[2]["continue_last"], False)

    def test_attach_command_can_skip_health_check_and_continue_latest(self) -> None:
        parser = build_parser()
        calls: list[dict[str, object]] = []

        class FakeAttachRuntime:
            def __init__(self, *, server_url: str, workspace: Path, auth_token: str | None) -> None:
                calls.append({"server_url": server_url, "workspace": workspace, "auth_token": auth_token})

            def list_sessions(self) -> list[dict[str, object]]:
                raise AssertionError("health check should be skipped")

        def fake_tui_main(argv: list[str], **kwargs: object) -> None:
            calls.append({"argv": argv, **kwargs})

        args = parser.parse_args(["attach", "http://app.test", "--continue", "--skip-health-check", "--server-token", "inline-secret"])

        exit_code = run_attach_command(args, tui_main=fake_tui_main, runtime_factory=FakeAttachRuntime, stderr=io.StringIO())

        self.assertEqual(exit_code, 0)
        self.assertEqual(calls[0]["auth_token"], "inline-secret")
        self.assertEqual(calls[1]["initial_session_id"], None)
        self.assertEqual(calls[1]["continue_last"], True)

    def test_config_init_creates_private_env_file(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            args = parser.parse_args(
                [
                    "config",
                    "init",
                    "--workspace",
                    raw_tmp,
                    "--api-key",
                    "test-secret",
                    "--with-server-token",
                    "--format",
                    "json",
                ]
            )
            stdout = io.StringIO()

            exit_code = run_config_command(args, stdout=stdout, stderr=io.StringIO())

            env_path = (Path(raw_tmp) / ".openagent" / "openagent.env").resolve()
            payload = json.loads(stdout.getvalue())
            self.assertEqual(exit_code, 0)
            self.assertEqual(payload["path"], str(env_path))
            self.assertEqual(payload["api_key_written"], True)
            self.assertEqual(payload["server_token_written"], True)
            self.assertEqual(stat.S_IMODE(env_path.stat().st_mode), 0o600)
            content = env_path.read_text(encoding="utf-8")
            self.assertIn("OPENAI_MODEL=gpt-5.5", content)
            self.assertIn("OPENAI_API_KEY=test-secret", content)
            self.assertIn("OPENAGENT_SERVER_TOKEN=", content)

    def test_config_init_rejects_existing_file_without_force(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            env_dir = Path(raw_tmp) / ".openagent"
            env_dir.mkdir()
            env_path = env_dir / "openagent.env"
            env_path.write_text("OPENAI_MODEL=existing\n", encoding="utf-8")
            args = parser.parse_args(["config", "init", "--workspace", raw_tmp])
            stderr = io.StringIO()

            exit_code = run_config_command(args, stdout=io.StringIO(), stderr=stderr)

            self.assertEqual(exit_code, 1)
            self.assertIn("already exists", stderr.getvalue())
            self.assertEqual(env_path.read_text(encoding="utf-8"), "OPENAI_MODEL=existing\n")

    def test_config_show_reports_resolved_values_without_secrets(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            env_dir = Path(raw_tmp) / ".openagent"
            env_dir.mkdir()
            (env_dir / "openagent.env").write_text(
                "\n".join(
                    [
                        "OPENAI_API_KEY=private-key",
                        "OPENAI_BASE_URL=http://localhost:9999",
                        "OPENAI_MODEL=gpt-config",
                        "OPENAI_WIRE_API=responses",
                        "OPENAGENT_APP_MAX_STEPS=44",
                        "OPENAGENT_SERVER_TOKEN=server-token",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            args = parser.parse_args(["config", "show", "--workspace", raw_tmp, "--format", "json"])
            stdout = io.StringIO()

            with patch.dict(os.environ, {}, clear=True):
                exit_code = run_config_command(args, stdout=stdout, stderr=io.StringIO())

            payload = json.loads(stdout.getvalue())
            self.assertEqual(exit_code, 0)
            self.assertEqual(payload["openai"]["base_url"], "http://localhost:9999")
            self.assertEqual(payload["openai"]["model"], "gpt-config")
            self.assertEqual(payload["openai"]["api_key"], "set")
            self.assertEqual(payload["app_bridge"]["server_token"], "set")
            self.assertNotIn("private-key", stdout.getvalue())
            self.assertNotIn("server-token", stdout.getvalue())

    def test_mcp_add_list_show_and_remove_manage_config_without_leaking_headers(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            add_stdout = io.StringIO()
            add_args = parser.parse_args(
                [
                    "mcp",
                    "add",
                    "demo",
                    "--config",
                    str(config_path),
                    "--url",
                    "https://example.com/mcp",
                    "--transport",
                    "http",
                    "--header",
                    "Authorization=Bearer secret-token",
                    "--header",
                    "X-Team=platform",
                    "--timeout-ms",
                    "45000",
                    "--format",
                    "json",
                ]
            )

            self.assertEqual(run_mcp_command(add_args, stdout=add_stdout, stderr=io.StringIO()), 0)
            add_payload = json.loads(add_stdout.getvalue())
            self.assertEqual(add_payload["server"]["name"], "demo")
            self.assertEqual(add_payload["server"]["header_names"], ["Authorization", "X-Team"])
            self.assertNotIn("secret-token", add_stdout.getvalue())
            self.assertNotIn("platform", add_stdout.getvalue())

            raw = json.loads(config_path.read_text(encoding="utf-8"))
            self.assertEqual(raw["mcpServers"]["demo"]["url"], "https://example.com/mcp")
            self.assertEqual(raw["mcpServers"]["demo"]["headers"]["Authorization"], "Bearer secret-token")

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["mcp", "list", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            listed = json.loads(list_stdout.getvalue())
            self.assertEqual(listed["servers"][0]["name"], "demo")
            self.assertNotIn("secret-token", list_stdout.getvalue())

            show_stdout = io.StringIO()
            show_args = parser.parse_args(["mcp", "show", "demo", "--config", str(config_path)])
            self.assertEqual(run_mcp_command(show_args, stdout=show_stdout, stderr=io.StringIO()), 0)
            self.assertIn("Authorization, X-Team", show_stdout.getvalue())
            self.assertNotIn("secret-token", show_stdout.getvalue())

            remove_stdout = io.StringIO()
            remove_args = parser.parse_args(["mcp", "rm", "demo", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(remove_args, stdout=remove_stdout, stderr=io.StringIO()), 0)
            self.assertEqual(json.loads(remove_stdout.getvalue())["removed"], True)
            self.assertEqual(json.loads(config_path.read_text(encoding="utf-8"))["mcpServers"], {})

    def test_mcp_uses_workspace_default_config_path(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            workspace = Path(raw_tmp)
            args = parser.parse_args(
                [
                    "mcp",
                    "add",
                    "demo",
                    "--workspace",
                    str(workspace),
                    "--url",
                    "https://example.com/mcp",
                    "--format",
                    "json",
                ]
            )
            stdout = io.StringIO()

            self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            expected_path = workspace.resolve() / ".openagent" / "mcp.json"
            self.assertEqual(payload["config_path"], str(expected_path))
            self.assertTrue(expected_path.exists())

    def test_mcp_doctor_validates_config_without_refresh_by_default(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "type": "remote",
                                "url": "https://example.com/mcp",
                                "transport": "auto",
                                "headers": {"Authorization": "Bearer secret-token"},
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            args = parser.parse_args(["mcp", "doctor", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()

            self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["configured"], True)
            self.assertEqual(payload["server_count"], 1)
            self.assertEqual(payload["servers"][0]["status"], "idle")
            self.assertNotIn("secret-token", stdout.getvalue())

    def test_mcp_doctor_refresh_failure_is_status_not_traceback(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps({"mcpServers": {"demo": {"url": "https://example.com/mcp"}}}),
                encoding="utf-8",
            )
            args = parser.parse_args(["mcp", "doctor", "--refresh", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()

            with patch("openagent.cli.mcp.RemoteMcpManager.refresh_all_sync", side_effect=RuntimeError("network down")):
                exit_code = run_mcp_command(args, stdout=stdout, stderr=io.StringIO())

            payload = json.loads(stdout.getvalue())
            self.assertEqual(exit_code, 2)
            self.assertEqual(payload["servers"][0]["status"], "error")
            self.assertEqual(payload["servers"][0]["last_error"], "network down")
            self.assertNotIn("Traceback", stdout.getvalue())

    def test_mcp_auth_set_token_list_status_and_logout_redact_secrets(self) -> None:
        parser = build_parser()
        sentinel = "sentinel-mcp-token"
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "type": "remote",
                                "url": "https://example.com/mcp",
                                "headers": {"X-Team": "platform"},
                            },
                            "disabled": {"url": "https://disabled.example/mcp", "enabled": False},
                            "local": {"command": "node", "args": ["server.js"]},
                            "oauth": {"url": "https://oauth.example/mcp", "oauth": {"issuer": "https://issuer.example"}},
                            "oauth-off": {"url": "https://off.example/mcp", "oauth": False},
                        }
                    }
                ),
                encoding="utf-8",
            )

            set_stdout = io.StringIO()
            set_args = parser.parse_args(
                [
                    "mcp",
                    "auth",
                    "set-token",
                    "demo",
                    "--config",
                    str(config_path),
                    "--bearer-token",
                    sentinel,
                    "--format",
                    "json",
                ]
            )
            self.assertEqual(run_mcp_command(set_args, stdout=set_stdout, stderr=io.StringIO()), 0)
            set_payload = json.loads(set_stdout.getvalue())
            self.assertEqual(set_payload["server"]["status"], "authenticated")
            self.assertEqual(set_payload["server"]["auth_header_names"], ["Authorization"])
            self.assertNotIn(sentinel, set_stdout.getvalue())

            mode = stat.S_IMODE(config_path.stat().st_mode)
            self.assertEqual(mode, 0o600)
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            self.assertEqual(raw["mcpServers"]["demo"]["headers"]["Authorization"], f"Bearer {sentinel}")

            list_stdout = io.StringIO()
            list_args = parser.parse_args(["mcp", "auth", "list", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(list_args, stdout=list_stdout, stderr=io.StringIO()), 0)
            listed = json.loads(list_stdout.getvalue())
            statuses = {server["name"]: server["status"] for server in listed["servers"]}
            self.assertEqual(statuses["demo"], "authenticated")
            self.assertEqual(statuses["disabled"], "disabled")
            self.assertEqual(statuses["local"], "not_remote")
            self.assertEqual(statuses["oauth"], "needs_auth")
            self.assertEqual(statuses["oauth-off"], "oauth_disabled")
            self.assertNotIn(sentinel, list_stdout.getvalue())
            self.assertNotIn("platform", list_stdout.getvalue())

            status_stdout = io.StringIO()
            status_args = parser.parse_args(["mcp", "auth", "status", "demo", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(status_args, stdout=status_stdout, stderr=io.StringIO()), 0)
            status_payload = json.loads(status_stdout.getvalue())
            self.assertEqual(status_payload["server"]["status"], "authenticated")
            self.assertNotIn(sentinel, status_stdout.getvalue())

            logout_stdout = io.StringIO()
            logout_args = parser.parse_args(["mcp", "logout", "demo", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(logout_args, stdout=logout_stdout, stderr=io.StringIO()), 0)
            logout_payload = json.loads(logout_stdout.getvalue())
            self.assertEqual(logout_payload["removed_headers"], ["Authorization"])
            self.assertEqual(logout_payload["server"]["status"], "not_authenticated")
            self.assertNotIn(sentinel, logout_stdout.getvalue())
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            self.assertNotIn("Authorization", raw["mcpServers"]["demo"]["headers"])
            self.assertEqual(raw["mcpServers"]["demo"]["headers"]["X-Team"], "platform")

    def test_mcp_auth_without_subcommand_lists_status_and_sanitizes_url(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "url": "https://client:basic-secret@example.com/mcp?token=url-secret&safe=1",
                                "oauth": {"clientId": "client-id", "clientSecret": "client-secret"},
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            args = parser.parse_args(["mcp", "auth", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()

            self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            server = payload["servers"][0]
            self.assertEqual(server["status"], "needs_auth")
            self.assertTrue(server["has_client_registration"])
            self.assertTrue(server["client_id_set"])
            self.assertTrue(server["client_secret_set"])
            self.assertIn("https://[redacted]@example.com", server["url"])
            self.assertIn("token=[redacted]", server["url"])
            self.assertNotIn("basic-secret", stdout.getvalue())
            self.assertNotIn("url-secret", stdout.getvalue())
            self.assertNotIn("client-secret", stdout.getvalue())

    def test_mcp_read_surfaces_redact_url_userinfo(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "url": "https://client:basic-secret@example.com/mcp?token=url-secret&safe=1",
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )

            commands = [
                ["mcp", "list", "--config", str(config_path), "--format", "json"],
                ["mcp", "show", "demo", "--config", str(config_path), "--format", "json"],
                ["mcp", "auth", "--config", str(config_path), "--format", "json"],
                ["mcp", "debug", "demo", "--config", str(config_path), "--format", "json"],
            ]
            for command in commands:
                with self.subTest(command=command):
                    stdout = io.StringIO()
                    self.assertEqual(run_mcp_command(parser.parse_args(command), stdout=stdout, stderr=io.StringIO()), 0)
                    output = stdout.getvalue()
                    self.assertIn("https://[redacted]@example.com", output)
                    self.assertIn("token=[redacted]", output)
                    self.assertNotIn("basic-secret", output)
                    self.assertNotIn("url-secret", output)

    def test_mcp_auth_set_token_stdin_custom_header_redacts_secret(self) -> None:
        parser = build_parser()
        sentinel = "stdin-sentinel-token"
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps({"mcpServers": {"demo": {"url": "https://example.com/mcp"}}}),
                encoding="utf-8",
            )
            args = parser.parse_args(
                [
                    "mcp",
                    "auth",
                    "set-token",
                    "demo",
                    "--config",
                    str(config_path),
                    "--bearer-token-stdin",
                    "--header-name",
                    "X-API-Key",
                    "--format",
                    "json",
                ]
            )
            stdout = io.StringIO()

            with patch("sys.stdin", io.StringIO(sentinel + "\n")):
                self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["server"]["status"], "authenticated")
            self.assertEqual(payload["server"]["auth_header_names"], ["X-API-Key"])
            self.assertNotIn(sentinel, stdout.getvalue())
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            self.assertEqual(raw["mcpServers"]["demo"]["headers"]["X-API-Key"], sentinel)

    def test_mcp_logout_removes_auth_headers_and_token_fields(self) -> None:
        parser = build_parser()
        secret_values = [
            "header-secret",
            "proxy-secret",
            "key-secret",
            "api-secret",
            "oauth-secret",
            "refresh-secret",
            "client-secret",
            "top-secret",
            "nested-secret",
        ]
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "url": "https://example.com/mcp",
                                "headers": {
                                    "Authorization": "Bearer header-secret",
                                    "Proxy-Authorization": "Bearer proxy-secret",
                                    "X-API-Key": "key-secret",
                                    "Api-Key": "api-secret",
                                    "X-Team": "platform",
                                },
                                "oauth": {
                                    "issuer": "https://issuer.example",
                                    "scopes": ["tools"],
                                    "access_token": "oauth-secret",
                                    "refresh_token": "refresh-secret",
                                    "client_secret": "client-secret",
                                },
                                "token": "top-secret",
                                "client": {
                                    "client_id": "nested-client-id",
                                    "client_secret": "nested-secret",
                                    "registration_url": "https://issuer.example/register",
                                },
                                "client_id": "client-id",
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            args = parser.parse_args(["mcp", "logout", "demo", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()

            self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["server"]["status"], "needs_auth")
            self.assertEqual(payload["removed_headers"], ["Api-Key", "Authorization", "Proxy-Authorization", "X-API-Key"])
            self.assertEqual(
                payload["removed_fields"],
                ["client.client_secret", "oauth.access_token", "oauth.client_secret", "oauth.refresh_token", "token"],
            )
            for secret in secret_values:
                self.assertNotIn(secret, stdout.getvalue())
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            server = raw["mcpServers"]["demo"]
            self.assertEqual(server["headers"], {"X-Team": "platform"})
            self.assertEqual(server["oauth"], {"issuer": "https://issuer.example", "scopes": ["tools"]})
            self.assertNotIn("token", server)
            self.assertEqual(
                server["client"],
                {"client_id": "nested-client-id", "registration_url": "https://issuer.example/register"},
            )
            self.assertEqual(server["client_id"], "client-id")

    def test_mcp_auth_status_and_logout_handle_structured_oauth_tokens(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "secure": {
                                "url": "https://example.com/mcp",
                                "oauth": {
                                    "enabled": True,
                                    "scopes": ["tools"],
                                    "tokens": {
                                        "access_token": "nested-access-secret",
                                        "refresh_token": "nested-refresh-secret",
                                    },
                                    "client": {
                                        "client_id": "registered-client",
                                        "client_secret": "nested-client-secret",
                                    },
                                },
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )

            status_stdout = io.StringIO()
            status_args = parser.parse_args(
                ["mcp", "auth", "status", "secure", "--config", str(config_path), "--format", "json"]
            )
            self.assertEqual(run_mcp_command(status_args, stdout=status_stdout, stderr=io.StringIO()), 0)
            status_payload = json.loads(status_stdout.getvalue())
            self.assertEqual(status_payload["server"]["status"], "authenticated")
            self.assertEqual(status_payload["server"]["method"], "oauth")
            self.assertIn("oauth.tokens", status_payload["server"]["token_fields"])
            self.assertTrue(status_payload["server"]["client_id_set"])
            self.assertTrue(status_payload["server"]["client_secret_set"])
            self.assertNotIn("nested-access-secret", status_stdout.getvalue())
            self.assertNotIn("nested-client-secret", status_stdout.getvalue())

            logout_stdout = io.StringIO()
            logout_args = parser.parse_args(["mcp", "logout", "secure", "--config", str(config_path), "--format", "json"])
            self.assertEqual(run_mcp_command(logout_args, stdout=logout_stdout, stderr=io.StringIO()), 0)
            logout_payload = json.loads(logout_stdout.getvalue())
            self.assertEqual(logout_payload["server"]["status"], "needs_auth")
            self.assertEqual(logout_payload["removed_fields"], ["oauth.client.client_secret", "oauth.tokens"])
            self.assertNotIn("nested-access-secret", logout_stdout.getvalue())
            self.assertNotIn("nested-client-secret", logout_stdout.getvalue())
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            self.assertEqual(
                raw["mcpServers"]["secure"]["oauth"],
                {"client": {"client_id": "registered-client"}, "enabled": True, "scopes": ["tools"]},
            )

    def test_mcp_auth_login_writes_oauth_tokens_and_client_registration(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps({"mcpServers": {"secure": {"url": "https://example.com/mcp"}}}),
                encoding="utf-8",
            )
            calls: list[dict[str, object]] = []

            async def fake_flow(server, **kwargs):
                calls.append({"server": server, **kwargs})
                return (
                    OAuthToken(access_token="oauth-access-secret", refresh_token="oauth-refresh-secret"),
                    OAuthClientInformationFull(
                        redirect_uris=["http://127.0.0.1:14555/oauth/callback"],
                        client_id="registered-client",
                        client_secret="registered-secret",
                    ),
                )

            args = parser.parse_args(
                [
                    "mcp",
                    "auth",
                    "login",
                    "secure",
                    "--config",
                    str(config_path),
                    "--scope",
                    "tools",
                    "--no-browser",
                    "--format",
                    "json",
                ]
            )
            stdout = io.StringIO()

            with patch("openagent.cli.mcp.run_mcp_oauth_login_flow", side_effect=fake_flow):
                self.assertEqual(run_mcp_command(args, stdout=stdout, stderr=io.StringIO()), 0)

            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["server"]["status"], "authenticated")
            self.assertEqual(payload["server"]["method"], "oauth")
            self.assertEqual(payload["server"]["token_fields"], ["oauth.tokens"])
            self.assertNotIn("oauth-access-secret", stdout.getvalue())
            self.assertNotIn("registered-secret", stdout.getvalue())
            raw = json.loads(config_path.read_text(encoding="utf-8"))
            oauth = raw["mcpServers"]["secure"]["oauth"]
            self.assertEqual(oauth["enabled"], True)
            self.assertEqual(oauth["scopes"], ["tools"])
            self.assertEqual(oauth["tokens"]["access_token"], "oauth-access-secret")
            self.assertEqual(oauth["tokens"]["refresh_token"], "oauth-refresh-secret")
            self.assertEqual(oauth["client"]["client_id"], "registered-client")
            self.assertEqual(oauth["client"]["client_secret"], "registered-secret")
            self.assertEqual(calls[0]["redirect_uri"], "http://127.0.0.1:14555/oauth/callback")
            self.assertTrue(calls[0]["no_browser"])

    def test_mcp_debug_refresh_failure_is_structured_and_redacted(self) -> None:
        parser = build_parser()
        sentinel = "debug-sentinel-token"
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "demo": {
                                "url": "https://example.com/mcp",
                                "headers": {"Authorization": f"Bearer {sentinel}"},
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            args = parser.parse_args(["mcp", "debug", "demo", "--refresh", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()

            with patch("openagent.cli.mcp.RemoteMcpManager.refresh_all_sync", side_effect=RuntimeError("network down")):
                exit_code = run_mcp_command(args, stdout=stdout, stderr=io.StringIO())

            payload = json.loads(stdout.getvalue())
            self.assertEqual(exit_code, 2)
            self.assertEqual(payload["server"]["status"], "authenticated")
            self.assertEqual(payload["headers"], {"Authorization": "[redacted]"})
            self.assertEqual(payload["refresh_error"], "network down")
            self.assertEqual(payload["runtime_status"]["status"], "error")
            self.assertEqual(payload["runtime_status"]["last_error"], "network down")
            self.assertNotIn(sentinel, stdout.getvalue())
            self.assertNotIn("Traceback", stdout.getvalue())

            stdout = io.StringIO()
            with patch(
                "openagent.cli.mcp.RemoteMcpManager.refresh_all_sync",
                side_effect=RuntimeError(f"failed with token={sentinel} and Bearer {sentinel}"),
            ):
                exit_code = run_mcp_command(args, stdout=stdout, stderr=io.StringIO())
            self.assertEqual(exit_code, 2)
            self.assertNotIn(sentinel, stdout.getvalue())
            self.assertIn("token=[redacted]", stdout.getvalue())
            self.assertIn("Bearer [redacted]", stdout.getvalue())

            stdout = io.StringIO()
            with patch(
                "openagent.cli.mcp.RemoteMcpManager.refresh_all_sync",
                side_effect=RuntimeError('{"access_token":"json-secret","state":"state-secret"}'),
            ):
                exit_code = run_mcp_command(args, stdout=stdout, stderr=io.StringIO())
            self.assertEqual(exit_code, 2)
            self.assertNotIn("json-secret", stdout.getvalue())
            self.assertNotIn("state-secret", stdout.getvalue())
            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["refresh_error"], '{"access_token":"[redacted]","state":"[redacted]"}')
            self.assertEqual(
                payload["runtime_status"]["last_error"],
                '{"access_token":"[redacted]","state":"[redacted]"}',
            )

    def test_mcp_debug_missing_and_non_remote_fail_cleanly(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            config_path.write_text(
                json.dumps({"mcpServers": {"local": {"command": "node", "args": ["server.js"]}}}),
                encoding="utf-8",
            )

            missing_args = parser.parse_args(["mcp", "debug", "missing", "--config", str(config_path), "--format", "json"])
            stderr = io.StringIO()
            self.assertEqual(run_mcp_command(missing_args, stdout=io.StringIO(), stderr=stderr), 1)
            self.assertIn("MCP server not found: missing", stderr.getvalue())
            self.assertNotIn("Traceback", stderr.getvalue())

            local_args = parser.parse_args(["mcp", "debug", "local", "--config", str(config_path), "--format", "json"])
            stdout = io.StringIO()
            stderr = io.StringIO()
            self.assertEqual(run_mcp_command(local_args, stdout=stdout, stderr=stderr), 2)
            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload["server"]["status"], "not_remote")
            self.assertEqual(payload["server"]["remote"], False)
            self.assertEqual(payload["config_status"]["ok"], False)
            self.assertIn("not remote", payload["config_status"]["error"])
            self.assertEqual(stderr.getvalue(), "")
            self.assertNotIn("Traceback", stdout.getvalue())

    def test_mcp_rejects_invalid_header_and_missing_server_cleanly(self) -> None:
        parser = build_parser()
        with tempfile.TemporaryDirectory() as raw_tmp:
            config_path = Path(raw_tmp) / "mcp.json"
            add_args = parser.parse_args(["mcp", "add", "demo", "--config", str(config_path), "--url", "https://example.com/mcp", "--header", "bad"])
            stderr = io.StringIO()

            self.assertEqual(run_mcp_command(add_args, stdout=io.StringIO(), stderr=stderr), 2)
            self.assertIn("KEY=VALUE", stderr.getvalue())

            show_args = parser.parse_args(["mcp", "show", "missing", "--config", str(config_path)])
            stderr = io.StringIO()
            self.assertEqual(run_mcp_command(show_args, stdout=io.StringIO(), stderr=stderr), 1)
            self.assertIn("MCP server not found", stderr.getvalue())


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


class FakeModelRuntime:
    def __init__(self, *, workspace: str | None, session_store_root: str | None) -> None:
        self.workspace = workspace
        self.session_store_root = session_store_root

    def list_models(self) -> list[dict[str, object]]:
        return [
            {
                "id": "gpt-test",
                "provider_id": "openai",
                "name": "OpenAI Compatible/gpt-test",
                "context_window": 128000,
                "max_output": 4096,
            }
        ]


class FakeAppBridgeServer:
    def __init__(self, *, required_token: str | None = None) -> None:
        self.records: list[dict[str, object]] = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                if not self._authorized():
                    return
                if self.path == "/api/sessions":
                    self._send_json({"sessions": [{"id": "session_existing"}]})
                    return
                if self.path == "/api/sessions/session_existing":
                    self._send_json({"session": {"id": "session_existing"}})
                    return
                if self.path == "/api/turns/turn_1/events":
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.end_headers()
                    self.wfile.write(
                        (
                            'id: 1\n'
                            'event: item/agentMessage/delta\n'
                            'data: {"sequence": 1, "method": "item/agentMessage/delta", "params": {"event": {"text": "hello from server"}}}\n\n'
                            'id: 2\n'
                            'event: turn/completed\n'
                            'data: {"sequence": 2, "method": "turn/completed", "params": {"status": "completed", "final_answer": "hello from server"}}\n\n'
                        ).encode("utf-8")
                    )
                    return
                self._send_json({"error": "not found"}, status=404)

            def do_POST(self) -> None:  # noqa: N802
                if not self._authorized():
                    return
                payload = self._read_json()
                owner.records.append({"method": "POST", "path": self.path, "payload": payload, "authorization": self.headers.get("Authorization") or ""})
                if self.path == "/api/sessions":
                    self._send_json({"session": {"id": "session_new"}}, status=201)
                    return
                if self.path in {"/api/sessions/session_new/turns", "/api/sessions/session_existing/turns"}:
                    self._send_json({"turn": {"id": "turn_1"}}, status=201)
                    return
                self._send_json({"error": "not found"}, status=404)

            def log_message(self, format: str, *args: object) -> None:  # noqa: A002
                return

            def _authorized(self) -> bool:
                if required_token is None:
                    return True
                if self.headers.get("Authorization") == f"Bearer {required_token}":
                    if self.command == "GET":
                        owner.records.append({"method": "GET", "path": self.path, "payload": {}, "authorization": self.headers.get("Authorization") or ""})
                    return True
                self._send_json({"error": "unauthorized"}, status=401)
                return False

            def _read_json(self) -> dict[str, object]:
                raw_len = int(self.headers.get("Content-Length") or "0")
                if raw_len <= 0:
                    return {}
                value = json.loads(self.rfile.read(raw_len).decode("utf-8"))
                return value if isinstance(value, dict) else {}

            def _send_json(self, payload: dict[str, object], *, status: int = 200) -> None:
                data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        host, port = self.server.server_address
        self.url = f"http://{host}:{port}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def create_persisted_session(root: Path) -> Session:
    store = FileSessionStore(root)
    session = Session(directory=root.parent / "workspace")
    run_id = "run_test"
    store.start_run(
        session,
        run_id=run_id,
        trace_id="trace_test",
        agent_name="test-agent",
        model_id="gpt-test",
        provider_id="openai",
        permission="FULL",
        max_steps=3,
    )
    message = ChatMessage(role="user", content="private prompt")
    session.messages.append(message)
    store.append_message(session, message, run_id=run_id, index=0)
    store.record_event(
        session_id=session.id,
        run_id=run_id,
        event="model.usage",
        kind="model",
        attributes={"input_tokens": 12, "output_tokens": 5, "cost": 0.003},
    )
    store.finish_run(session, run_id=run_id, status="completed", steps=1, finish_reason="stop")
    return session


def write_command(workspace: Path, name: str, content: str) -> Path:
    directory = workspace / ".openagent" / "commands"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{name}.md"
    path.write_text(content, encoding="utf-8")
    return path


if __name__ == "__main__":
    unittest.main()
