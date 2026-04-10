from __future__ import annotations

import os
import shutil
import types
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch
from uuid import uuid4

from openagent.core.execution import (
    CommandResult,
    ExecutionBinding,
    OpenSandboxWorkspaceRuntime,
    build_workspace_runtime,
    execution_binding_from_session,
)
from openagent.core.session.session import Session
from openagent.core.tool.toolkit import ToolkitAdapter


class ExecutionRuntimeTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, root, True)
        return root

    def test_execution_binding_defaults_to_local(self) -> None:
        session = Session(directory=self._make_temp_root())
        binding = execution_binding_from_session(session)
        self.assertEqual(binding.mode, "local")

    def test_execution_binding_requires_sandbox_fields(self) -> None:
        session = Session(directory=self._make_temp_root())
        session.metadata["execution"] = {"mode": "opensandbox", "remote_workdir": "/workspace/project"}
        with self.assertRaisesRegex(ValueError, "sandbox_id"):
            execution_binding_from_session(session)

        session.metadata["execution"] = {"mode": "opensandbox", "sandbox_id": "sbx_123"}
        with self.assertRaisesRegex(ValueError, "remote_workdir"):
            execution_binding_from_session(session)

    def test_execution_binding_rejects_non_posix_remote_workdir(self) -> None:
        session = Session(directory=self._make_temp_root())
        session.metadata["execution"] = {
            "mode": "opensandbox",
            "sandbox_id": "sbx_123",
            "remote_workdir": r"C:\\repo",
        }
        with self.assertRaisesRegex(ValueError, "POSIX"):
            execution_binding_from_session(session)

    async def test_opensandbox_runtime_uses_sdk_and_resolves_paths(self) -> None:
        session = Session(directory=self._make_temp_root())
        session.metadata["execution"] = {
            "mode": "opensandbox",
            "sandbox_id": "sbx_123",
            "remote_workdir": "/workspace/project",
            "connection": {"domain": "api.example.test", "protocol": "https", "use_server_proxy": True},
        }

        writes: list[tuple[str, str]] = []
        searches: list[tuple[str, str]] = []
        command_calls: list[str] = []

        class FakeWriteEntry:
            def __init__(self, *, path: str, data: str) -> None:
                self.path = path
                self.data = data

        class FakeSearchEntry:
            def __init__(self, *, path: str, pattern: str) -> None:
                self.path = path
                self.pattern = pattern

        class FakeConnectionConfig:
            def __init__(self, **kwargs) -> None:
                self.kwargs = kwargs

        class FakeFiles:
            async def read_file(self, path: str):
                return {"/workspace/project/a.txt": "hello world"}.get(path, "")

            async def write_files(self, entries):
                for entry in entries:
                    writes.append((entry.path, entry.data))
                return None

            async def search(self, entries):
                for entry in entries:
                    searches.append((entry.path, entry.pattern))
                return [types.SimpleNamespace(path="/workspace/project/a.txt")]

        class FakeCommands:
            async def run(self, command: str):
                command_calls.append(command)
                return types.SimpleNamespace(
                    logs=types.SimpleNamespace(stdout=[types.SimpleNamespace(text="ok\n")], stderr=[]),
                    exit_code=0,
                )

        class FakeSandboxClient:
            def __init__(self) -> None:
                self.files = FakeFiles()
                self.commands = FakeCommands()

        class FakeSandbox:
            calls: list[tuple[str, object]] = []

            @staticmethod
            async def connect(sandbox_id: str, *, connection_config):
                FakeSandbox.calls.append((sandbox_id, connection_config))
                return FakeSandboxClient()

        with patch("openagent.core.execution.runtime._load_opensandbox_sdk", return_value=(FakeSandbox, FakeConnectionConfig, FakeWriteEntry, FakeSearchEntry)):
            with patch.dict(os.environ, {"OPEN_SANDBOX_API_KEY": "test-key"}, clear=False):
                runtime = build_workspace_runtime(session)
                self.assertIsInstance(runtime, OpenSandboxWorkspaceRuntime)
                self.assertEqual(runtime.resolve_path("src/app.py"), "/workspace/project/src/app.py")
                with self.assertRaisesRegex(ValueError, "escapes"):
                    runtime.resolve_path("../secret.txt")
                with self.assertRaisesRegex(ValueError, "POSIX"):
                    runtime.resolve_path(r"C:\\repo")

                result = await runtime.run_command("echo hi", None, 10_000)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "ok\n")
                self.assertEqual(command_calls[0], "cd /workspace/project && echo hi")

                text = await runtime.read_text("/workspace/project/a.txt")
                self.assertEqual(text, "hello world")

                await runtime.write_text("/workspace/project/src/new.txt", "demo")
                self.assertEqual(writes[-1], ("/workspace/project/src/new.txt", "demo"))
                self.assertTrue(any("mkdir -p /workspace/project/src" in call for call in command_calls))

                matches = await runtime.glob("/workspace/project", "**/*.txt")
                self.assertEqual(matches, ["/workspace/project/a.txt"])
                self.assertEqual(searches[-1], ("/workspace/project", "**/*.txt"))
                self.assertEqual(FakeSandbox.calls[0][0], "sbx_123")
                self.assertEqual(FakeSandbox.calls[0][1].kwargs["api_key"], "test-key")
                self.assertEqual(FakeSandbox.calls[0][1].kwargs["domain"], "api.example.test")

    async def test_opensandbox_runtime_allows_local_dev_server_without_api_key(self) -> None:
        session = Session(directory=self._make_temp_root())
        session.metadata["execution"] = {
            "mode": "opensandbox",
            "sandbox_id": "sbx_local",
            "remote_workdir": "/workspace/project",
            "connection": {
                "domain": "http://127.0.0.1:8090",
                "use_server_proxy": True,
                "request_timeout_seconds": 12,
            },
        }

        class FakeWriteEntry:
            def __init__(self, *, path: str, data: str) -> None:
                self.path = path
                self.data = data

        class FakeSearchEntry:
            def __init__(self, *, path: str, pattern: str) -> None:
                self.path = path
                self.pattern = pattern

        class FakeConnectionConfig:
            def __init__(self, **kwargs) -> None:
                self.kwargs = kwargs

        class FakeSandboxClient:
            class commands:
                @staticmethod
                async def run(command: str):
                    return types.SimpleNamespace(logs=types.SimpleNamespace(stdout=[], stderr=[]), exit_code=0)

        class FakeSandbox:
            calls: list[tuple[str, object]] = []

            @staticmethod
            async def connect(sandbox_id: str, *, connection_config):
                FakeSandbox.calls.append((sandbox_id, connection_config))
                return FakeSandboxClient()

        with patch("openagent.core.execution.runtime._load_opensandbox_sdk", return_value=(FakeSandbox, FakeConnectionConfig, FakeWriteEntry, FakeSearchEntry)):
            with patch.dict(os.environ, {}, clear=True):
                runtime = build_workspace_runtime(session)
                await runtime.run_command("true", None, 10_000)
                self.assertEqual(FakeSandbox.calls[0][0], "sbx_local")
                self.assertEqual(FakeSandbox.calls[0][1].kwargs["domain"], "http://127.0.0.1:8090")
                self.assertTrue(FakeSandbox.calls[0][1].kwargs["use_server_proxy"])
                self.assertNotIn("api_key", FakeSandbox.calls[0][1].kwargs)
                self.assertEqual(int(FakeSandbox.calls[0][1].kwargs["request_timeout"].total_seconds()), 12)
    async def test_toolkit_filters_and_executes_sandbox_workspace_tools(self) -> None:
        root = self._make_temp_root()
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()

        sandbox_runtime = OpenSandboxWorkspaceRuntime(
            ExecutionBinding(mode="opensandbox", sandbox_id="sbx_123", remote_workdir="/workspace/project")
        )
        sandbox_runtime.exists = AsyncMock(side_effect=lambda path: path == "/workspace/project/a.txt")
        sandbox_runtime.is_dir = AsyncMock(return_value=False)
        sandbox_runtime.read_text = AsyncMock(return_value="hello\nworld")
        sandbox_runtime.write_text = AsyncMock(return_value=None)
        sandbox_runtime.glob = AsyncMock(return_value=["/workspace/project/a.txt"])
        sandbox_runtime.grep = AsyncMock(return_value=[{"path": "/workspace/project/a.txt", "line": 1, "text": "hello", "mtime": 0.0}])
        sandbox_runtime.ls = AsyncMock(return_value=[types.SimpleNamespace(path="/workspace/project/a.txt", is_dir=False, mtime=0.0)])
        sandbox_runtime.run_command = AsyncMock(return_value=CommandResult(returncode=0, stdout="sandbox ok", stderr="", cwd="/workspace/project"))

        session = Session(directory=root)
        ctx = {
            "session_root": str(root),
            "session": session,
            "execution_mode": "opensandbox",
            "workspace_root": "/workspace/project",
            "workspace_runtime": sandbox_runtime,
            "execution_metadata": sandbox_runtime.execution_metadata,
        }

        tools = {tool.name for tool in toolkit.get_all_tools(execution_mode="opensandbox")}
        self.assertIn("bash", tools)
        self.assertIn("read", tools)
        self.assertIn("web_fetch", tools)
        self.assertNotIn("code_search", tools)

        blocked = await toolkit.execute(name="code_search", input={"query": "x"}, context=ctx)
        self.assertIsNotNone(blocked.error)
        self.assertIn('not available in opensandbox mode', blocked.error or "")

        read_res = await toolkit.execute(name="read", input={"file_path": "a.txt", "offset": 0, "limit": 5}, context=ctx)
        self.assertIsNone(read_res.error)
        self.assertIn("00001| hello", read_res.output)
        self.assertTrue(session.has_read_file("opensandbox://sbx_123/workspace/project/a.txt"))

        write_res = await toolkit.execute(name="write", input={"file_path": "a.txt", "content": "updated"}, context=ctx)
        self.assertIsNone(write_res.error)
        self.assertEqual(write_res.metadata["file_path"], "/workspace/project/a.txt")
        self.assertEqual(write_res.metadata["sandbox_id"], "sbx_123")

        bash_res = await toolkit.execute(name="bash", input={"command": "pwd"}, context=ctx)
        self.assertIsNone(bash_res.error)
        self.assertEqual(bash_res.output, "sandbox ok")
        self.assertEqual(bash_res.metadata["workdir"], "/workspace/project")
        sandbox_runtime.run_command.assert_awaited()
