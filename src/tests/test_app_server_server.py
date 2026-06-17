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


if __name__ == "__main__":
    unittest.main()
