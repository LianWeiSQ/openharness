from __future__ import annotations

import unittest

from openagent.app_server.protocol import stream_event_to_app_event, stream_event_to_app_method


class AppServerProtocolTests(unittest.TestCase):
    def test_maps_core_stream_events_to_ui_methods(self) -> None:
        self.assertEqual(stream_event_to_app_method("text-delta"), "item/agentMessage/delta")
        self.assertEqual(stream_event_to_app_method("tool-call"), "item/toolCall/started")
        self.assertEqual(stream_event_to_app_method("tool-result"), "item/toolCall/completed")
        self.assertEqual(stream_event_to_app_method("runtime-warning"), "runtime/warning")
        self.assertEqual(stream_event_to_app_method("unknown"), "item/event")

    def test_wraps_event_with_thread_turn_and_sequence(self) -> None:
        event = stream_event_to_app_event(
            {"type": "tool-call", "name": "ls", "input": {"path": "."}, "call_id": "call_1"},
            sequence=3,
            thread_id="session_1",
            turn_id="turn_1",
        )

        self.assertEqual(event.sequence, 3)
        self.assertEqual(event.method, "item/toolCall/started")
        self.assertEqual(event.params["thread_id"], "session_1")
        self.assertEqual(event.params["turn_id"], "turn_1")
        self.assertEqual(event.params["event"]["name"], "ls")


if __name__ == "__main__":
    unittest.main()
