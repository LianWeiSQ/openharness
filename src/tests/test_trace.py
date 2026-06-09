from __future__ import annotations

import contextlib
import io
import json
import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.tool.definition import ToolContext, ToolDefinition, ToolOutput
from openagent.core.trace import AgentTraceRecorder, RunRecord, check_trace_run, load_trace_events, load_trace_summary
from openagent.core.trace.cli import main as trace_cli_main
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


@dataclass
class EmptyArgs:
    pass


class TraceTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"trace_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def test_trace_recorder_writes_trace_summary_and_redacts_sensitive_fields(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(
            run_id="run_test",
            trace_id="trace_test",
            session_id="session_test",
            agent_name="agent",
            model_id="model",
            provider_id="provider",
            workspace=str(temp),
            started_at_ms=1000,
        )
        metadata: dict[str, object] = {}
        recorder = AgentTraceRecorder(run=run, base_dir=temp, session_metadata=metadata)

        recorder.record_event("run.started", kind="run", attributes={"api_key": "secret", "input_tokens": 2})
        recorder.record_event(
            "model.call.finished",
            kind="model",
            duration_ms=20,
            attributes={"input_tokens": 3, "output_tokens": 4, "cost": 0.01},
        )
        recorder.finish_run(attributes={"status": "completed"})

        trace_events = load_trace_events(recorder.trace_path)
        summary = load_trace_summary(recorder.summary_path)

        self.assertEqual(trace_events[0]["attributes"]["api_key"], "[redacted]")
        self.assertEqual(trace_events[0]["attributes"]["input_tokens"], 2)
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(summary["model_call_count"], 1)
        self.assertEqual(summary["total_input_tokens"], 3)
        self.assertEqual(summary["total_output_tokens"], 4)
        self.assertAlmostEqual(summary["total_cost"], 0.01)
        self.assertTrue(Path(metadata["agent_trace"]["trace_path"]).exists())  # type: ignore[index]

    async def test_agent_loop_writes_standard_trace_for_model_and_mcp_tool_calls(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "c1", "name": "demo_mcp", "input": {}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 5, "output_tokens": 1, "cost": 0.02}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.03}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=["demo_mcp"],
            options={"trace": {"root_dir": "runs"}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        session = Session(directory=temp)
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager())

        async def _execute(_args: EmptyArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(
                title="Demo MCP",
                output="pong",
                metadata={
                    "backend": "mcp",
                    "mcp_server": "demo",
                    "mcp_original_tool_name": "ping",
                },
            )

        loop.toolkit.registry.register(
            ToolDefinition(
                id="demo_mcp",
                description="Demo MCP ping tool.",
                parameters=EmptyArgs,
                execute=_execute,
                group="mcp",
                dangerous=True,
                execution_scope="agnostic",
            )
        )

        async for _event in loop.run("call demo mcp"):
            pass

        trace_metadata = session.metadata["agent_trace"]
        summary = load_trace_summary(trace_metadata["summary_path"])
        trace_events = load_trace_events(trace_metadata["trace_path"])
        finished_tool_events = [event for event in trace_events if event["event"] == "tool.call.finished"]
        event_names = [event["event"] for event in trace_events]

        self.assertEqual(summary["status"], "completed")
        self.assertEqual(summary["model_call_count"], 2)
        self.assertEqual(summary["tool_call_count"], 1)
        self.assertEqual(summary["mcp_call_count"], 1)
        self.assertEqual(summary["total_input_tokens"], 7)
        self.assertEqual(summary["total_output_tokens"], 4)
        self.assertAlmostEqual(summary["total_cost"], 0.05)
        self.assertIn("text.started", event_names)
        self.assertIn("text.delta", event_names)
        self.assertIn("text.finished", event_names)
        text_delta = next(event for event in trace_events if event["event"] == "text.delta")
        self.assertEqual(text_delta["attributes"]["delta_chars"], 4)
        self.assertNotIn("done", json.dumps(text_delta, ensure_ascii=False))
        self.assertEqual(finished_tool_events[0]["attributes"]["tool_source"], "mcp")
        self.assertEqual(finished_tool_events[0]["attributes"]["mcp_server"], "demo")
        self.assertTrue(check_trace_run(Path(trace_metadata["summary_path"]).parent)["ok"])

    def test_trace_cli_shows_summary(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(
            run_id="run_cli",
            trace_id="trace_cli",
            session_id="session_cli",
            agent_name="agent",
        )
        recorder = AgentTraceRecorder(run=run, base_dir=temp)
        recorder.finish_run(attributes={"status": "completed"})

        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = trace_cli_main(["--root", str(temp / ".openagent" / "runs"), "show", "run_cli"])

        self.assertEqual(code, 0)
        output = buffer.getvalue()
        self.assertIn("Run: run_cli", output)
        self.assertIn("Status: completed", output)

    async def test_trace_cli_check_validates_complete_agent_run(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(script=[[{"type": "text-delta", "id": "t1", "text": "ok"}, {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1}}]])
        cfg = AgentConfig(name="u", permission="FULL", max_steps=3, tools=[], options={"trace": {"root_dir": "runs"}})
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        session = Session(directory=temp)
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager())

        async for _event in loop.run("hello"):
            pass

        run_id = session.metadata["agent_trace"]["run_id"]
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = trace_cli_main(["--root", str(temp / "runs"), "check", run_id])

        self.assertEqual(code, 0)
        self.assertIn("Trace OK", buffer.getvalue())

    def test_trace_check_reports_missing_required_events(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(run_id="run_bad", trace_id="trace_bad", session_id="session_bad", agent_name="agent")
        recorder = AgentTraceRecorder(run=run, base_dir=temp)
        recorder.record_event("run.started", kind="run")

        result = check_trace_run(recorder.run_dir)

        self.assertFalse(result["ok"])
        self.assertIn("missing terminal run event", result["errors"])
        self.assertIn("missing model.call.started", result["errors"])


if __name__ == "__main__":
    unittest.main()
