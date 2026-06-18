from __future__ import annotations

from collections.abc import Awaitable, Callable

from mcp.client.auth.oauth2 import OAuthClientProvider, TokenStorage
from mcp.shared.auth import OAuthClientInformationFull, OAuthClientMetadata, OAuthToken

from .types import McpOAuthClientInfo, McpOAuthConfig, McpOAuthTokens, RemoteMcpServerConfig


class McpOAuthMemoryStorage(TokenStorage):
    def __init__(
        self,
        *,
        tokens: OAuthToken | None = None,
        client_info: OAuthClientInformationFull | None = None,
    ) -> None:
        self._tokens = tokens
        self._client_info = client_info

    async def get_tokens(self) -> OAuthToken | None:
        return self._tokens

    async def set_tokens(self, tokens: OAuthToken) -> None:
        self._tokens = tokens

    async def get_client_info(self) -> OAuthClientInformationFull | None:
        return self._client_info

    async def set_client_info(self, client_info: OAuthClientInformationFull) -> None:
        self._client_info = client_info


def build_oauth_client_metadata(server: RemoteMcpServerConfig) -> OAuthClientMetadata:
    oauth = _require_enabled_oauth(server)
    kwargs: dict[str, object] = {
        "redirect_uris": list(oauth.redirect_uris),
        "client_name": oauth.client_name,
    }
    if oauth.scopes:
        kwargs["scope"] = " ".join(oauth.scopes)
    if oauth.client_uri:
        kwargs["client_uri"] = oauth.client_uri
    if oauth.client and oauth.client.token_endpoint_auth_method:
        kwargs["token_endpoint_auth_method"] = oauth.client.token_endpoint_auth_method
    return OAuthClientMetadata(**kwargs)


def build_oauth_storage(server: RemoteMcpServerConfig) -> McpOAuthMemoryStorage:
    oauth = _require_enabled_oauth(server)
    metadata = build_oauth_client_metadata(server)
    return McpOAuthMemoryStorage(
        tokens=_to_sdk_tokens(oauth.tokens),
        client_info=_to_sdk_client_info(oauth, metadata),
    )


def build_oauth_auth(
    server: RemoteMcpServerConfig,
    *,
    storage: TokenStorage | None = None,
    redirect_handler: Callable[[str], Awaitable[None]] | None = None,
    callback_handler: Callable[[], Awaitable[tuple[str, str | None]]] | None = None,
) -> OAuthClientProvider | None:
    oauth = server.oauth
    if oauth is None or not oauth.enabled:
        return None
    metadata = build_oauth_client_metadata(server)
    return OAuthClientProvider(
        server_url=server.url,
        client_metadata=metadata,
        storage=storage or build_oauth_storage(server),
        redirect_handler=redirect_handler,
        callback_handler=callback_handler,
        timeout=oauth.timeout_s,
        client_metadata_url=oauth.client_metadata_url,
    )


def _require_enabled_oauth(server: RemoteMcpServerConfig) -> McpOAuthConfig:
    oauth = server.oauth
    if oauth is None or not oauth.enabled:
        raise ValueError(f"MCP server '{server.name}' does not have OAuth enabled.")
    return oauth


def _to_sdk_tokens(tokens: McpOAuthTokens | None) -> OAuthToken | None:
    if tokens is None:
        return None
    return OAuthToken(
        access_token=tokens.access_token,
        token_type=tokens.token_type,
        expires_in=tokens.expires_in,
        scope=tokens.scope,
        refresh_token=tokens.refresh_token,
    )


def _to_sdk_client_info(
    oauth: McpOAuthConfig,
    metadata: OAuthClientMetadata,
) -> OAuthClientInformationFull | None:
    client = oauth.client
    if client is None:
        return None
    payload = metadata.model_dump(mode="json", exclude_none=True)
    payload.update(_client_info_payload(client, default_redirect_uris=oauth.redirect_uris))
    return OAuthClientInformationFull(**payload)


def _client_info_payload(client: McpOAuthClientInfo, *, default_redirect_uris: tuple[str, ...]) -> dict[str, object]:
    payload: dict[str, object] = {
        "redirect_uris": list(client.redirect_uris or default_redirect_uris),
    }
    if client.client_id:
        payload["client_id"] = client.client_id
    if client.client_secret:
        payload["client_secret"] = client.client_secret
    if client.client_id_issued_at is not None:
        payload["client_id_issued_at"] = client.client_id_issued_at
    if client.client_secret_expires_at is not None:
        payload["client_secret_expires_at"] = client.client_secret_expires_at
    if client.token_endpoint_auth_method:
        payload["token_endpoint_auth_method"] = client.token_endpoint_auth_method
    return payload
