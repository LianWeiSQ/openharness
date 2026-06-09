# Operations

OpenAgent exposes enough runtime data to debug agent behavior without relying on external telemetry.

## Stream Events

`AgentLoop.run()` yields structured events:

| Event | Meaning |
| --- | --- |
| `text-start`, `text-delta`, `text-end` | Model text stream |
| `tool-call` | Model requested a tool |
| `tool-result` | Tool execution finished |
| `step-start`, `step-finish` | One loop step boundary |
| `question-request` | Agent needs user input |
| `patch` | Workspace files changed |
| `error` | Runtime error |

## Observability

Runtime metadata is stored under `Session.metadata["observability"]`:

- trace identifiers;
- recent events in a bounded buffer;
- model usage;
- context pack diagnostics;
- optional JSONL paths.

Runtime logs are separate from trace events. Logs are for operators; trace events are for replay, metrics, and regression analysis.

## Safety

Logs and traces should not include secrets. OpenAgent redacts common secret-bearing keys such as `api_key`, `authorization`, `cookie`, `password`, `secret`, and `token`.

Remote sandbox connection data is used to initialize the runtime, but is not emitted as tool metadata.

## Evaluation

The repository includes local eval/replay utilities plus benchmark adapters:

- local eval runner for deterministic regression cases;
- trace/replay output for failure analysis;
- Terminal-Bench adapter via `perform_task(instruction, session, logging_dir)`;
- Harbor adapter for benchmark environment execution.

Smoke command:

```bash
PYTHONPATH=src:src/tests python -m unittest \
  src/tests/test_terminal_bench_adapter.py \
  src/tests/test_harbor_adapter.py \
  src/tests/test_loop.py
```

Full regression:

```bash
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```
