from __future__ import annotations

import contextlib
import io
import json
import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from unittest.mock import patch
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.tool.definition import ToolContext, ToolDefinition, ToolOutput
from openagent.core.trace import AgentTraceRecorder, LangfuseTraceExporter, RunRecord, TraceConfig, check_trace_run, load_trace_config, load_trace_events, load_trace_summary
from openagent.core.trace.cli import main as trace_cli_main
from openagent.core.trace.exporter import build_trace_exporters
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


@dataclass
class EmptyArgs:
    pass


class FakeTraceExporter:
    name = "fake"

    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []
        self.closed = False

    def record_event(self, event: dict[str, Any]) -> None:
        self.events.append(dict(event))

    def close(self) -> None:
        self.closed = True


class FailingTraceExporter:
    name = "failing"

    def record_event(self, event: dict[str, Any]) -> None:
        del event
        raise RuntimeError("export failed")

    def close(self) -> None:
        raise RuntimeError("close failed")


class FakeLangfuseObservation:
    def __init__(self, observation_id: str, *, name: str, as_type: str, trace_context: dict[str, str] | None) -> None:
        self.id = observation_id
        self.name = name
        self.as_type = as_type
        self.trace_context = dict(trace_context or {})
        self.updates: list[dict[str, Any]] = []
        self.ended = False

    def update(self, **payload: Any) -> None:
        self.updates.append(dict(payload))

    def end(self) -> None:
        self.ended = True


class FakeLangfuseClient:
    def __init__(self) -> None:
        self.started: list[FakeLangfuseObservation] = []
        self.flushed = False

    def create_trace_id(self, *, seed: str) -> str:
        del seed
        return "1" * 32

    def start_observation(self, *, name: str, as_type: str = "span", trace_context: dict[str, str] | None = None) -> FakeLangfuseObservation:
        observation = FakeLangfuseObservation(f"{len(self.started) + 1:016x}", name=name, as_type=as_type, trace_context=trace_context)
        self.started.append(observation)
        return observation

    def flush(self) -> None:
        self.flushed = True


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

    def test_trace_config_parses_langsmith_exporter_options(self) -> None:
        config = load_trace_config(
            {
                "trace": {
                    "root_dir": "runs",
                    "exporters": {
                        "langsmith": {
                            "enabled": True,
                            "project": "openagent-dev",
                            "include_content": False,
                        }
                    },
                }
            }
        )

        self.assertEqual(config.root_dir, "runs")
        self.assertTrue(config.exporters["langsmith"]["enabled"])
        self.assertEqual(config.exporters["langsmith"]["project"], "openagent-dev")

    def test_trace_config_parses_langfuse_exporter_options(self) -> None:
        config = load_trace_config(
            {
                "trace": {
                    "root_dir": "runs",
                    "exporters": {
                        "langfuse": {
                            "enabled": True,
                            "scores_enabled": True,
                            "include_content": False,
                        }
                    },
                }
            }
        )

        self.assertEqual(config.root_dir, "runs")
        self.assertTrue(config.exporters["langfuse"]["enabled"])
        self.assertTrue(config.exporters["langfuse"]["scores_enabled"])
        self.assertFalse(config.exporters["langfuse"]["include_content"])

    def test_trace_recorder_exports_events_and_closes_on_terminal_event(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(run_id="run_export", trace_id="trace_export", session_id="session_export", agent_name="agent")
        metadata: dict[str, object] = {}
        exporter = FakeTraceExporter()
        recorder = AgentTraceRecorder(run=run, base_dir=temp, session_metadata=metadata, exporters=[exporter])

        recorder.record_event("run.started", kind="run")
        recorder.record_event("model.call.started", kind="model", span_id="span_model")
        recorder.record_event(
            "model.call.finished",
            kind="model",
            span_id="span_model",
            attributes={"model": "mock", "input_tokens": 1, "output_tokens": 2},
        )
        recorder.finish_run(attributes={"status": "completed"})

        self.assertTrue(exporter.closed)
        self.assertEqual([event["event"] for event in exporter.events], ["run.started", "model.call.started", "model.call.finished", "run.finished"])
        exporters = metadata["agent_trace"]["exporters"]  # type: ignore[index]
        self.assertEqual(exporters["enabled"], ["fake"])
        self.assertEqual(exporters["diagnostics"], [])

    def test_trace_exporter_errors_are_recorded_as_diagnostics(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(run_id="run_fail_export", trace_id="trace_fail_export", session_id="session_fail_export", agent_name="agent")
        metadata: dict[str, object] = {}
        recorder = AgentTraceRecorder(run=run, base_dir=temp, session_metadata=metadata, exporters=[FailingTraceExporter()])

        recorder.record_event("run.started", kind="run")
        recorder.finish_run(attributes={"status": "completed"})

        summary = load_trace_summary(recorder.summary_path)
        exporters = metadata["agent_trace"]["exporters"]  # type: ignore[index]
        self.assertEqual(summary["status"], "completed")
        self.assertGreaterEqual(len(exporters["diagnostics"]), 2)
        self.assertEqual(exporters["diagnostics"][0]["exporter"], "failing")

    def test_langfuse_exporter_maps_trace_events_without_content_by_default(self) -> None:
        run = RunRecord(run_id="run_lf", trace_id="trace_lf", session_id="session_lf", agent_name="agent", model_id="mock-model")
        client = FakeLangfuseClient()
        exporter = LangfuseTraceExporter(
            run=run,
            client=client,
            langfuse_trace_id="1" * 32,
            include_content=False,
            scores_enabled=True,
        )

        exporter.record_event({"seq": 1, "event": "run.started", "kind": "run", "attributes": {"input_preview": "secret prompt"}})
        exporter.record_event({"seq": 2, "event": "step.started", "kind": "step", "attributes": {"step_index": 1}})
        exporter.record_event(
            {
                "seq": 3,
                "event": "model.call.started",
                "kind": "model",
                "span_id": "model_1",
                "attributes": {"model": "mock-model", "provider": "mock", "prompt": "secret prompt"},
            }
        )
        exporter.record_event(
            {
                "seq": 4,
                "event": "model.call.finished",
                "kind": "model",
                "span_id": "model_1",
                "status": "ok",
                "duration_ms": 42,
                "attributes": {"model": "mock-model", "completion": "secret output", "input_tokens": 3, "output_tokens": 4, "cost": 0.01},
            }
        )
        exporter.record_event(
            {
                "seq": 5,
                "event": "tool.call.started",
                "kind": "tool",
                "span_id": "tool_1",
                "attributes": {"tool_name": "bash", "tool_source": "local_tool", "arguments_preview": "cat secret.txt"},
            }
        )
        exporter.record_event(
            {
                "seq": 6,
                "event": "tool.call.finished",
                "kind": "tool",
                "span_id": "tool_1",
                "status": "ok",
                "attributes": {"tool_name": "bash", "result_summary": "secret result"},
            }
        )
        exporter.record_event({"seq": 7, "event": "step.finished", "kind": "step", "attributes": {"step_index": 1}})
        exporter.record_event({"seq": 8, "event": "run.finished", "kind": "run", "status": "ok", "attributes": {"status": "completed"}})
        exporter.close()

        self.assertEqual([item.as_type for item in client.started], ["agent", "span", "generation", "tool"])
        self.assertTrue(all(item.trace_context.get("trace_id") == "1" * 32 for item in client.started))
        generation = next(item for item in client.started if item.as_type == "generation")
        flattened_updates = json.dumps(generation.updates, ensure_ascii=False)
        self.assertNotIn("secret prompt", flattened_updates)
        self.assertNotIn("secret output", flattened_updates)
        self.assertTrue(all("input" not in update and "output" not in update for update in generation.updates))
        self.assertIn(
            {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7},
            [update.get("usage_details") for update in generation.updates if update.get("usage_details")],
        )
        self.assertTrue(all(item.ended for item in client.started))
        self.assertTrue(client.flushed)

    def test_langfuse_exporter_can_include_content_when_enabled(self) -> None:
        run = RunRecord(run_id="run_lf_content", trace_id="trace_lf_content", session_id="session_lf_content", agent_name="agent")
        client = FakeLangfuseClient()
        exporter = LangfuseTraceExporter(
            run=run,
            client=client,
            langfuse_trace_id="2" * 32,
            include_content=True,
        )

        exporter.record_event(
            {
                "seq": 1,
                "event": "model.call.started",
                "kind": "model",
                "span_id": "model_1",
                "attributes": {"prompt": "visible prompt"},
            }
        )
        exporter.record_event(
            {
                "seq": 2,
                "event": "model.call.finished",
                "kind": "model",
                "span_id": "model_1",
                "attributes": {"completion": "visible output"},
            }
        )

        generation = client.started[0]
        merged = {key: value for update in generation.updates for key, value in update.items()}
        self.assertEqual(merged["input"], "visible prompt")
        self.assertEqual(merged["output"], "visible output")

    def test_langfuse_exporter_metadata_is_recorded_on_session(self) -> None:
        temp = self._make_temp_dir()
        run = RunRecord(run_id="run_lf_meta", trace_id="trace_lf_meta", session_id="session_lf_meta", agent_name="agent")
        metadata: dict[str, object] = {}
        exporter = LangfuseTraceExporter(
            run=run,
            client=FakeLangfuseClient(),
            langfuse_trace_id="3" * 32,
            scores_enabled=True,
        )
        recorder = AgentTraceRecorder(run=run, base_dir=temp, session_metadata=metadata, exporters=[exporter])

        recorder.record_event("run.started", kind="run")
        recorder.finish_run(attributes={"status": "completed"})

        exporters = metadata["agent_trace"]["exporters"]  # type: ignore[index]
        self.assertEqual(exporters["enabled"], ["langfuse"])
        self.assertEqual(exporters["langfuse"]["trace_id"], "3" * 32)
        self.assertTrue(exporters["langfuse"]["scores_enabled"])

    def test_build_trace_exporters_records_langfuse_diagnostics(self) -> None:
        run = RunRecord(run_id="run_lf_diag", trace_id="trace_lf_diag", session_id="session_lf_diag", agent_name="agent")
        diagnostics: list[dict[str, Any]] = []
        with patch.object(LangfuseTraceExporter, "from_config", side_effect=RuntimeError("missing langfuse")):
            exporters = build_trace_exporters(
                run=run,
                config=TraceConfig(exporters={"langfuse": {"enabled": True}}),
                diagnostics=diagnostics,
            )

        self.assertEqual(exporters, [])
        self.assertEqual(diagnostics[0]["exporter"], "langfuse")
        self.assertEqual(diagnostics[0]["error_kind"], "RuntimeError")

        with patch.object(LangfuseTraceExporter, "from_config", side_effect=RuntimeError("missing langfuse")):
            with self.assertRaises(RuntimeError):
                build_trace_exporters(
                    run=run,
                    config=TraceConfig(exporters={"langfuse": {"enabled": True, "strict": True}}),
                    diagnostics=[],
                )

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
