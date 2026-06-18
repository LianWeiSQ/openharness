from __future__ import annotations

import unittest

import httpx
from mcp.client.auth.oauth2 import PKCEParameters

from openagent.core.mcp.auth import build_oauth_auth, build_oauth_client_metadata, build_oauth_storage
from openagent.core.mcp.config import load_mcp_config
from openagent.core.mcp.types import RemoteMcpServerConfig


def _oauth_server() -> RemoteMcpServerConfig:
    return load_mcp_config(
        {
            "mcpServers": {
                "secure": {
                    "url": "https://mcp.example.test/mcp",
                    "oauth": {
                        "redirect_uri": "http://127.0.0.1:14555/callback",
                        "scopes": ["tools", "search"],
                        "client_name": "OpenAgent Test",
                        "tokens": {
                            "access_token": "seeded-access-token",
                            "refresh_token": "seeded-refresh-token",
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
    ).servers[0]


class McpOAuthAuthTests(unittest.IsolatedAsyncioTestCase):
    async def test_oauth_storage_round_trips_seeded_tokens_and_client_info(self) -> None:
        storage = build_oauth_storage(_oauth_server())

        tokens = await storage.get_tokens()
        client_info = await storage.get_client_info()

        self.assertIsNotNone(tokens)
        self.assertEqual(tokens.access_token if tokens else None, "seeded-access-token")
        self.assertEqual(tokens.refresh_token if tokens else None, "seeded-refresh-token")
        self.assertIsNotNone(client_info)
        self.assertEqual(client_info.client_id if client_info else None, "client-id")
        self.assertEqual(client_info.client_secret if client_info else None, "client-secret")
        self.assertEqual(client_info.token_endpoint_auth_method if client_info else None, "client_secret_basic")

    async def test_seeded_oauth_token_injects_authorization_header(self) -> None:
        seen: dict[str, str | None] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen["authorization"] = request.headers.get("Authorization")
            return httpx.Response(200, json={"ok": True})

        async with httpx.AsyncClient(
            auth=build_oauth_auth(_oauth_server()),
            transport=httpx.MockTransport(handler),
        ) as client:
            response = await client.get("https://mcp.example.test/mcp")

        self.assertEqual(response.status_code, 200)
        self.assertEqual(seen["authorization"], "Bearer seeded-access-token")

    def test_build_oauth_client_metadata_maps_config(self) -> None:
        metadata = build_oauth_client_metadata(_oauth_server())

        self.assertEqual([str(uri) for uri in metadata.redirect_uris or []], ["http://127.0.0.1:14555/callback"])
        self.assertEqual(metadata.scope, "tools search")
        self.assertEqual(metadata.client_name, "OpenAgent Test")
        self.assertEqual(metadata.token_endpoint_auth_method, "client_secret_basic")

    def test_pkce_generation_shape_is_available_from_sdk(self) -> None:
        params = PKCEParameters.generate()

        self.assertTrue(params.code_verifier)
        self.assertTrue(params.code_challenge)
        self.assertNotEqual(params.code_verifier, params.code_challenge)
