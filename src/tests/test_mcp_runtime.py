from __future__ import annotations

import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

from mcp.types import TextContent

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.mcp.config import load_mcp_config
from openagent.core.mcp.runtime import RemoteMcpManager, _build_http_client, _build_oauth_auth, _sanitize_error_text
from openagent.core.mcp.types import McpConfig, RemoteMcpServerConfig, RemoteMcpToolDescriptor
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.types import AgentConfig, Model

from _mock_model import ScriptedLanguageModel


@dataclass
class NoArgs:
    pass


def _make_model_metadata(*, context_window: int = 1024, max_output: int = 128) -> Model:
    return Model(
        id='test-model',
        provider_id='test',
        name='Test Model',
        context_window=context_window,
        max_output=max_output,
    )


class FakeRuntimeManager(RemoteMcpManager):
    def __init__(self, result: object) -> None:
        super().__init__(
            McpConfig(
                servers=(
                    RemoteMcpServerConfig(
                        name='demo',
                        url='https://example.com/mcp',
                    ),
                )
            )
        )
        descriptor = RemoteMcpToolDescriptor(
            server_name='demo',
            original_name='weather',
            dynamic_name='mcp_tool_demo_weather',
            title='Remote Weather',
            description='Remote MCP weather tool',
            input_schema={'type': 'object', 'properties': {}},
        )
        self._servers['demo'].tools_by_dynamic_name = {descriptor.dynamic_name: descriptor}
        self.result = result

    async def _call_tool_with_fallback(self, server, tool_name, arguments):
        del server, tool_name, arguments
        return 'http', self.result


class McpRuntimeTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path('openagent/tests/workdir')
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f't_{uuid4().hex}'
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    async def test_remote_mcp_manager_ignores_structured_content_and_keeps_text_output(self) -> None:
        manager = FakeRuntimeManager(
            SimpleNamespace(
                content=[TextContent(type='text', text='Weather summary\nCloudy with light wind.')],
                structuredContent={'city': 'Shanghai', 'temperature': 24, 'condition': 'cloudy'},
                isError=False,
            )
        )

        result = await manager.call_tool('mcp_tool_demo_weather', {'city': 'Shanghai'})

        self.assertEqual(result.output, 'Weather summary\nCloudy with light wind.')
        self.assertEqual(result.metadata['title'], 'MCP demo/weather')
        self.assertEqual(result.metadata['mcp_original_tool_name'], 'weather')
        self.assertEqual(result.metadata['mcp_transport'], 'http')
        self.assertEqual(result.metadata['mcp_tool_name'], 'mcp_tool_demo_weather')
        self.assertEqual(result.metadata['mcp_non_text_blocks'], [])
        self.assertNotIn('structured_content', result.metadata)
        self.assertNotIn('preview', result.metadata)

    async def test_remote_mcp_manager_records_non_text_blocks_with_placeholders(self) -> None:
        manager = FakeRuntimeManager(
            SimpleNamespace(
                content=[
                    TextContent(type='text', text='Headline'),
                    SimpleNamespace(type='image'),
                    SimpleNamespace(type='resource'),
                    SimpleNamespace(type='blob'),
                ],
                structuredContent={'ignored': True},
                isError=False,
            )
        )

        result = await manager.call_tool('mcp_tool_demo_weather', {})

        self.assertIn('Headline', result.output)
        self.assertIn('[MCP content ignored: image]', result.output)
        self.assertIn('[MCP content ignored: resource]', result.output)
        self.assertIn('[MCP content ignored: binary]', result.output)
        self.assertEqual(result.metadata['mcp_non_text_blocks'], ['image', 'resource', 'binary'])

    async def test_remote_mcp_manager_returns_placeholder_when_only_structured_content_exists(self) -> None:
        manager = FakeRuntimeManager(
            SimpleNamespace(
                content=[],
                structuredContent={'city': 'Shanghai', 'temperature': 24},
                isError=False,
            )
        )

        result = await manager.call_tool('mcp_tool_demo_weather', {})

        self.assertEqual(result.output, '(Remote MCP tool completed with no textual output.)')
        self.assertEqual(result.metadata['mcp_non_text_blocks'], [])
        self.assertNotIn('structured_content', result.metadata)
        self.assertNotIn('preview', result.metadata)

    async def test_loop_does_not_include_structured_content_in_next_model_call(self) -> None:
        manager = FakeRuntimeManager(
            SimpleNamespace(
                content=[TextContent(type='text', text='Weather summary\nCloudy with light wind.')],
                structuredContent={'city': 'Shanghai', 'temperature': 24, 'condition': 'cloudy'},
                isError=False,
            )
        )
        toolkit = ToolkitAdapter()
        toolkit.register_mcp(manager)

        model = ScriptedLanguageModel(
            script=[
                [
                    {'type': 'tool-call', 'call_id': 'mcp-1', 'name': 'mcp_tool_demo_weather', 'input': {}},
                    {'type': 'finish', 'finish_reason': 'tool_call', 'usage': {'input_tokens': 1, 'output_tokens': 1, 'cost': 0.0}},
                ],
                [
                    {'type': 'text-delta', 'id': 't1', 'text': 'Using the MCP weather result now.'},
                    {'type': 'finish', 'finish_reason': 'stop', 'usage': {'input_tokens': 1, 'output_tokens': 1, 'cost': 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name='u',
            permission='FULL',
            tools=['mcp_tool_demo_weather'],
            max_steps=5,
            model=_make_model_metadata(),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt='Test prompt.')
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager(), toolkit=toolkit)

        async for _event in loop.run('上海天气怎么样'):
            pass

        self.assertEqual(model.call_index, 2)
        tool_messages = [message for message in model.seen_messages_by_call[1] if getattr(message, 'role', None) == 'tool']
        self.assertEqual(len(tool_messages), 1)
        self.assertIn('Weather summary', tool_messages[0].content)
        self.assertIn('Cloudy with light wind.', tool_messages[0].content)
        self.assertNotIn('structured', tool_messages[0].content.lower())
        self.assertNotIn('temperature', tool_messages[0].content)

    async def test_remote_mcp_oauth_auth_is_attached_to_http_client(self) -> None:
        server = load_mcp_config(
            {
                "mcpServers": {
                    "secure": {
                        "url": "https://example.com/mcp",
                        "headers": {"X-Team": "platform"},
                        "oauth": {
                            "tokens": {"access_token": "seeded-access-token"},
                            "client": {"client_id": "client-id"},
                        },
                    }
                }
            }
        ).servers[0]

        auth = _build_oauth_auth(server)
        client = _build_http_client(server, timeout_seconds=1.0, auth=auth)
        try:
            self.assertIsNotNone(auth)
            self.assertIs(client.auth, auth)
            self.assertEqual(client.headers["X-Team"], "platform")
        finally:
            await client.aclose()

    async def test_remote_mcp_runtime_sanitizes_secret_errors(self) -> None:
        message = _sanitize_error_text(
            "failed Bearer access-secret at https://user:pass@example.test/mcp?token=url-secret "
            "{'client_secret': 'client-secret'}"
        )

        self.assertNotIn("access-secret", message)
        self.assertNotIn("user:pass", message)
        self.assertNotIn("url-secret", message)
        self.assertNotIn("client-secret", message)
        self.assertIn("[redacted]", message)
