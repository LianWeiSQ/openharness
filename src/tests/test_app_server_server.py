from __future__ import annotations

import json
import shutil
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from uuid import uuid4

from openagent.app_server.server import create_server


class FakeRuntime:
    def list_models(self):
        return []

    def list_sessions(self):
        return []

    def interrupt_turn(self, turn_id: str):
        return {"id": turn_id, "status": "interrupting", "interrupt_requested": True}

    def respond_approval(self, turn_id: str, request_id: str, action: str):
        return {
            "method": "turn/approval_resolved",
            "params": {
                "turn_id": turn_id,
                "approval": {
                    "request_id": request_id,
                    "action": action,
                },
            },
        }


class AppServerServerTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"server_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def test_headless_server_serves_api_without_static_console(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        with urllib.request.urlopen(f"{base_url}/api/health", timeout=5) as response:  # noqa: S310 - local test server.
            health = json.loads(response.read().decode("utf-8"))

        self.assertEqual(health["service"], "openagent-app-server")
        self.assertEqual(health["ui_enabled"], False)

        try:
            urllib.request.urlopen(f"{base_url}/", timeout=5)  # noqa: S310 - local test server.
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 404)
            error.close()
        else:
            self.fail("headless server should not serve the static console")

    def test_server_requires_bearer_token_when_configured(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            auth_token="server-secret",
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        try:
            urllib.request.urlopen(f"{base_url}/api/health", timeout=5)  # noqa: S310 - local test server.
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 401)
            error.close()
        else:
            self.fail("server should reject unauthenticated API requests")

        request = urllib.request.Request(f"{base_url}/api/health", headers={"Authorization": "Bearer server-secret"})
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            health = json.loads(response.read().decode("utf-8"))

        self.assertEqual(health["auth_required"], True)

    def test_server_interrupt_endpoint_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        request = urllib.request.Request(f"{base_url}/api/turns/turn_123/interrupt", data=b"{}", method="POST")
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            payload = json.loads(response.read().decode("utf-8"))

        self.assertEqual(payload["turn"]["id"], "turn_123")
        self.assertEqual(payload["turn"]["status"], "interrupting")
        self.assertEqual(payload["turn"]["interrupt_requested"], True)

    def test_server_approval_endpoint_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        request = urllib.request.Request(
            f"{base_url}/api/turns/turn_123/approvals/approval_456",
            data=b'{"action":"allow"}',
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            payload = json.loads(response.read().decode("utf-8"))

        self.assertEqual(payload["event"]["method"], "turn/approval_resolved")
        self.assertEqual(payload["event"]["params"]["turn_id"], "turn_123")
        self.assertEqual(payload["event"]["params"]["approval"]["request_id"], "approval_456")
        self.assertEqual(payload["event"]["params"]["approval"]["action"], "allow")


if __name__ == "__main__":
    unittest.main()
