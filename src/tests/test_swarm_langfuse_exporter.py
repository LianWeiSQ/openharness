from __future__ import annotations

import json
import unittest
from typing import Any
from unittest.mock import patch

from swarm import AgentResult, AgentSpec, RunContext, SwarmRuntime, Usage, build_function_registry, load_swarm_config
from swarm.langfuse_exporter import SwarmLangfuseExporter, export_swarm_trace_to_langfuse


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
        return "a" * 32

    def start_observation(self, *, name: str, as_type: str = "span", trace_context: dict[str, str] | None = None) -> FakeLangfuseObservation:
        observation = FakeLangfuseObservation(f"{len(self.started) + 1:016x}", name=name, as_type=as_type, trace_context=trace_context)
        self.started.append(observation)
        return observation

    def flush(self) -> None:
        self.flushed = True


class SwarmLangfuseExporterTests(unittest.IsolatedAsyncioTestCase):
    async def test_exporter_maps_swarm_trace_tree(self) -> None:
        result = await _swarm_result()
        client = FakeLangfuseClient()
        exporter = SwarmLangfuseExporter(
            client=client,
            langfuse_trace_id="a" * 32,
            include_content=False,
        )

        exported = exporter.export(result.trace_events)

        self.assertTrue(exported.enabled)
        self.assertEqual(exported.trace_id, "a" * 32)
        self.assertGreaterEqual(exported.observations_sent, 5)
        by_name = {item.name: item for item in client.started}
        run = by_name["swarm.run langfuse-run"]
        task = by_name["swarm.task lf-task"]
        runner = by_name["swarm.runner alpha"]
        self.assertEqual(run.as_type, "agent")
        self.assertEqual(task.as_type, "span")
        self.assertEqual(runner.as_type, "span")
        self.assertEqual(task.trace_context["parent_span_id"], run.id)
        self.assertEqual(runner.trace_context["parent_span_id"], task.id)
        self.assertTrue(all(item.trace_context["trace_id"] == "a" * 32 for item in client.started))
        self.assertTrue(all(item.ended for item in client.started))
        self.assertTrue(client.flushed)

        runner_finished = next(item for item in client.started if item.name == "runner.finished")
        self.assertEqual(runner_finished.trace_context["parent_span_id"], runner.id)
        merged = _merged_updates(runner)
        self.assertEqual(merged["metadata"]["swarm_runner_id"], "alpha")
        self.assertEqual(merged["metadata"]["swarm_task_id"], "lf-task")
        self.assertEqual(merged["usage_details"], {"input_tokens": 7, "output_tokens": 3, "total_tokens": 10})
        self.assertEqual(merged["cost_details"], {"total": 0.02})

    async def test_exporter_redacts_content_attributes_by_default(self) -> None:
        result = await _swarm_result(summary="secret final answer")
        client = FakeLangfuseClient()
        exporter = SwarmLangfuseExporter(
            client=client,
            langfuse_trace_id="b" * 32,
            include_content=False,
        )

        exporter.export(result.trace_events)

        flattened = json.dumps([item.updates for item in client.started], ensure_ascii=False)
        self.assertNotIn("secret final answer", flattened)
        self.assertNotIn("Started alpha", flattened)
        self.assertNotIn("Trace context contains sensitive task details.", flattened)

    async def test_export_helper_is_non_fatal_unless_strict(self) -> None:
        result = await _swarm_result()
        with patch("swarm.langfuse_exporter.load_langfuse_client", side_effect=RuntimeError("missing langfuse")):
            exported = export_swarm_trace_to_langfuse(result.trace_events, options={"enabled": True})

        self.assertTrue(exported.enabled)
        self.assertIsNone(exported.trace_id)
        self.assertEqual(exported.diagnostics[0]["error_kind"], "RuntimeError")

        with patch("swarm.langfuse_exporter.load_langfuse_client", side_effect=RuntimeError("missing langfuse")):
            with self.assertRaises(RuntimeError):
                export_swarm_trace_to_langfuse(result.trace_events, options={"enabled": True, "strict": True})

    async def test_export_helper_uses_configured_client(self) -> None:
        result = await _swarm_result()
        client = FakeLangfuseClient()

        with patch("swarm.langfuse_exporter.load_langfuse_client", return_value=client):
            exported = export_swarm_trace_to_langfuse(
                result.trace_events,
                options={"enabled": True, "keys_required": False, "tags": ["custom"]},
            )

        self.assertEqual(exported.trace_id, "a" * 32)
        self.assertGreater(exported.observations_sent, 0)
        run = next(item for item in client.started if item.name == "swarm.run langfuse-run")
        metadata = _merged_updates(run)["metadata"]
        self.assertEqual(metadata["langfuse_tags"], ["custom"])


async def _swarm_result(*, summary: str = "safe summary"):
    config = load_swarm_config(
        {
            "runners": {
                "alpha": {"kind": "function", "roles": ["research"]},
            },
            "tasks": {
                "lf-task": {
                    "role": "research",
                    "objective": "Export trace to Langfuse.",
                    "context": "Trace context contains sensitive task details.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["alpha"],
                }
            },
        }
    )

    def ok(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
        return AgentResult(
            status="completed",
            summary=summary,
            confidence=0.8,
            usage=Usage(input_tokens=7, output_tokens=3, cost=0.02),
        )

    registry = build_function_registry(config, {"alpha": ok})
    return await SwarmRuntime(registry=registry).run_task(config.task("lf-task"), run_id="langfuse-run")


def _merged_updates(observation: FakeLangfuseObservation) -> dict[str, Any]:
    merged: dict[str, Any] = {}
    for update in observation.updates:
        merged.update(update)
    return merged


if __name__ == "__main__":
    unittest.main()
