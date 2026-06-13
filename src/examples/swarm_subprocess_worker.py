from __future__ import annotations

import json
import sys
from typing import Any


def main() -> None:
    payload = json.loads(sys.stdin.read() or "{}")
    spec = payload.get("spec") if isinstance(payload.get("spec"), dict) else {}
    runner = payload.get("runner") if isinstance(payload.get("runner"), dict) else {}
    inputs = spec.get("inputs") if isinstance(spec.get("inputs"), dict) else {}
    topic = str(inputs.get("topic") or "unknown topic")
    runner_id = str(runner.get("id") or "subprocess_worker")
    result: dict[str, Any] = {
        "status": "completed",
        "summary": f"Subprocess worker: checked CLI agent path for {topic}.",
        "evidence": ["Received the standard swarm external-runner payload on stdin."],
        "confidence": 0.86,
        "usage": {
            "input_tokens": 5,
            "output_tokens": 4,
            "cost": 0.0,
            "steps": 1,
            "latency_ms": 1,
        },
        "metadata": {
            "runner_id": runner_id,
            "stdout_format": "json",
        },
    }
    print(json.dumps(result, ensure_ascii=False, sort_keys=True))


if __name__ == "__main__":
    main()
