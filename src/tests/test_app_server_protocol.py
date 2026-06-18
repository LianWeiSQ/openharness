from __future__ import annotations

import unittest

from openagent.app_server.protocol import AppEvent, stream_event_to_app_event, stream_event_to_app_method


class AppServerProtocolTests(unittest.TestCase):
    def test_maps_core_stream_events_to_ui_methods(self) -> None:
        self.assertEqual(stream_event_to_app_method("text-delta"), "item/agentMessage/delta")
        self.assertEqual(stream_event_to_app_method("tool-call"), "item/toolCall/started")
        self.assertEqual(stream_event_to_app_method("tool-result"), "item/toolCall/completed")
        self.assertEqual(stream_event_to_app_method("runtime-warning"), "runtime/warning")
        self.assertEqual(stream_event_to_app_method("patch"), "item/patch/detected")
        self.assertEqual(stream_event_to_app_method("patch-reverted"), "item/patch/reverted")
        self.assertEqual(stream_event_to_app_method("patch-revert-failed"), "item/patch/revert_failed")
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

    def test_app_event_serializes_optional_global_sequence(self) -> None:
        event = AppEvent(
            sequence=2,
            method="turn/completed",
            params={"thread_id": "session_1", "turn_id": "turn_1"},
            global_sequence=7,
        )

        self.assertEqual(event.to_dict()["sequence"], 2)
        self.assertEqual(event.to_dict()["global_sequence"], 7)


if __name__ == "__main__":
    unittest.main()
