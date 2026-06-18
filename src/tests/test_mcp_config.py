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

    def test_load_mcp_config_parses_oauth_config(self) -> None:
        config = load_mcp_config(
            {
                "mcpServers": {
                    "secure": {
                        "url": "https://mcp.example.test/mcp",
                        "oauth": {
                            "redirect_uri": "http://127.0.0.1:14555/callback",
                            "scopes": ["tools", "search"],
                            "client_name": "OpenAgent Test",
                            "client_uri": "https://openagent.example.test",
                            "client_metadata_url": "https://client.example.test/openagent.json",
                            "tokens": {
                                "access_token": "access-secret",
                                "refresh_token": "refresh-secret",
                                "expires_in": 3600,
                            },
                            "client": {
                                "client_id": "client-id",
                                "client_secret": "client-secret",
                                "token_endpoint_auth_method": "client_secret_basic",
                            },
                        },
                    }
                }
            }
        )

        oauth = config.servers[0].oauth
        self.assertIsNotNone(oauth)
        assert oauth is not None
        self.assertTrue(oauth.enabled)
        self.assertEqual(oauth.redirect_uris, ("http://127.0.0.1:14555/callback",))
        self.assertEqual(oauth.scopes, ("tools", "search"))
        self.assertEqual(oauth.client_name, "OpenAgent Test")
        self.assertEqual(oauth.client_uri, "https://openagent.example.test")
        self.assertEqual(oauth.client_metadata_url, "https://client.example.test/openagent.json")
        self.assertEqual(oauth.tokens.access_token if oauth.tokens else None, "access-secret")
        self.assertEqual(oauth.tokens.refresh_token if oauth.tokens else None, "refresh-secret")
        self.assertEqual(oauth.client.client_id if oauth.client else None, "client-id")
        self.assertEqual(oauth.client.token_endpoint_auth_method if oauth.client else None, "client_secret_basic")

    def test_load_mcp_config_rejects_invalid_oauth_redirect_uri(self) -> None:
        with self.assertRaisesRegex(ValueError, "redirect_uri"):
            load_mcp_config(
                {
                    "mcpServers": {
                        "secure": {
                            "url": "https://mcp.example.test/mcp",
                            "oauth": {"redirect_uri": "not-a-url"},
                        }
                    }
                }
            )

    def test_load_mcp_config_rejects_invalid_oauth_scope_items(self) -> None:
        with self.assertRaisesRegex(ValueError, "scopes"):
            load_mcp_config(
                {
                    "mcpServers": {
                        "secure": {
                            "url": "https://mcp.example.test/mcp",
                            "oauth": {"scopes": ["tools search"]},
                        }
                    }
                }
            )

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
