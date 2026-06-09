from __future__ import annotations

import unittest

from openagent.core.mcp.config import load_mcp_config


class McpConfigTests(unittest.TestCase):
    def test_load_mcp_config_parses_remote_servers(self) -> None:
        config = load_mcp_config(
            {
                "mcp": {
                    "demo": {
                        "type": "remote",
                        "url": "https://example.com/mcp",
                        "transport": "auto",
                        "enabled": True,
                        "headers": {"Authorization": "Bearer token"},
                        "timeout_ms": 45000,
                        "tools": {"allow": ["search*", "fetch*"], "deny": ["fetchSecret"]},
                    }
                }
            }
        )

        self.assertEqual(len(config.servers), 1)
        server = config.servers[0]
        self.assertEqual(server.name, "demo")
        self.assertEqual(server.url, "https://example.com/mcp")
        self.assertEqual(server.transport, "auto")
        self.assertEqual(server.headers["Authorization"], "Bearer token")
        self.assertEqual(server.tools.allow, ("search*", "fetch*"))
        self.assertEqual(server.tools.deny, ("fetchSecret",))

    def test_load_mcp_config_parses_mcp_servers_streamable_http(self) -> None:
        config = load_mcp_config(
            {
                "mcpServers": {
                    "demo-streamable-http": {
                        "type": "streamableHttp",
                        "description": "demo server",
                        "url": "https://mcp.example.test/public/demo",
                        "headers": {"X-Auth-Key": "redacted"},
                    }
                }
            }
        )

        self.assertEqual(len(config.servers), 1)
        server = config.servers[0]
        self.assertEqual(server.name, "demo-streamable-http")
        self.assertEqual(server.url, "https://mcp.example.test/public/demo")
        self.assertEqual(server.transport, "http")
        self.assertEqual(server.headers["X-Auth-Key"], "redacted")

    def test_load_mcp_config_parses_sse_type(self) -> None:
        config = load_mcp_config(
            {
                "mcpServers": {
                    "demo": {
                        "type": "sse",
                        "url": "http://localhost:9000/sse",
                    }
                }
            }
        )

        self.assertEqual(config.servers[0].transport, "sse")

    def test_load_mcp_config_rejects_non_remote_type(self) -> None:
        with self.assertRaisesRegex(ValueError, "streamableHttp"):
            load_mcp_config(
                {
                    "mcp": {
                        "demo": {
                            "type": "stdio",
                            "url": "https://example.com/mcp",
                        }
                    }
                }
            )
