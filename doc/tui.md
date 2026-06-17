# OpenAgent TUI

OpenAgent TUI is the terminal interface for the OpenAgent App Bridge. It follows the same architectural idea as Codex TUI: the UI should not call model/provider/tool code directly. It should talk to a session/turn/event runtime and render the resulting stream.

## Why App Bridge First

Codex TUI is built around app-server primitives:

- thread;
- turn;
- item;
- app-server events;
- bottom composer;
- history and resume;
- approvals and overlays.

OpenAgent now maps these to:

| Codex TUI concept | OpenAgent support |
| --- | --- |
| Thread | `Session` |
| Turn | one `AgentLoop.run(...)` |
| Item | `AppEvent` mapped from `StreamEvent` |
| App-server session | `OpenAgentAppRuntime` |
| TUI app state | `TuiState` |
| Chat composer | curses input buffer |
| Agent stream | `item/agentMessage/delta` |
| Tool timeline | `item/toolCall/*` |
| Trace detail | `session.metadata["agent_trace"]` |

This makes Web Console and TUI consume the same runtime shape.

## Run

Recommended command:

```bash
openagent
```

The `openagent` command defaults to:

- `OPENAI_BASE_URL=http://localhost:8080`
- `OPENAI_MODEL=gpt-5.5`
- `OPENAI_WIRE_API=responses`
- `OPENAGENT_APP_MAX_STEPS=30`

Use `openagent doctor` to check whether your local gateway is reachable.

Optional private local config:

```bash
openagent config init \
  --api-key "$OPENAI_API_KEY" \
  --base-url http://localhost:8080 \
  --model gpt-5.5 \
  --wire-api responses

openagent config show
```

```bash
python -m pip install -e .

export OPENAI_API_KEY="..."
export OPENAI_BASE_URL="http://localhost:8080/v1"
export OPENAI_MODEL="gpt-5.5"
export OPENAI_WIRE_API="responses"

openagent-tui --workspace .
```

Module form:

```bash
PYTHONPATH=src python -m openagent.tui --workspace .
```

For scripts and automation, use the same App Bridge runtime without opening the TUI:

```bash
openagent run "summarize this repository"
openagent run --file README.md --format json "review the attached file"
```

For persisted state inspection:

```bash
openagent session list
openagent session export session_abc123 --sanitize
openagent stats
openagent models
```

For local provider credentials:

```bash
openagent auth login --api-key "$OPENAI_API_KEY" --base-url http://localhost:8080 --model gpt-5.5 --wire-api responses
openagent auth list
openagent auth logout
```

Custom command files can be rendered or run from the CLI today:

```bash
mkdir -p .openagent/commands
cat > .openagent/commands/review.md <<'EOF'
---
description: Review recent changes
---

Recent commits:
!`git log --oneline -5`

Review $ARGUMENTS.
EOF

openagent command render review "the current branch"
openagent run --command review "the current branch"
```

Inside the TUI, type:

```text
/help
/sessions
/resume session_abc123
/status
/commands
/review the current branch
```

Built-in commands are handled locally by the TUI. `/sessions` lists recent persisted sessions, `/resume <id-or-prefix>` switches to a previous session and renders the latest transcript messages when available, and `/status` shows the current session/turn state. `/commands` lists both built-in commands and project/global command files. `/name args...` renders a custom command template and submits the rendered prompt to the active session.

## Controls

| Key | Action |
| --- | --- |
| `Enter` | send current task |
| `Ctrl-N` | create new session |
| `Ctrl-R` | open session picker |
| `Ctrl-L` | clear visible timeline |
| `PageUp` / `PageDown` | scroll timeline |
| `Ctrl-C` | interrupt when running, quit when idle; terminal signal fallback exits cleanly |
| `Esc` | quit when idle and input is empty |
| `Ctrl-D` | quit |

When the session picker is open:

| Key | Action |
| --- | --- |
| `Up` / `Down` or `k` / `j` | move selection |
| `PageUp` / `PageDown` | jump selection |
| `Enter` | resume selected session |
| `Esc` | close picker |

## Current Feature Matrix

| Capability | Status | Notes |
| --- | --- | --- |
| Start new session | Supported | Uses `OpenAgentAppRuntime.start_session()` |
| Submit user turn | Supported | Calls `AgentLoop.run(...)` in background thread |
| Stream assistant text | Supported | Renders `item/agentMessage/delta` |
| Stream tool calls/results | Supported | Renders `item/toolCall/started` and `item/toolCall/completed` |
| Runtime warnings | Supported | Renders `runtime/warning` |
| Patch events | Supported | Renders `item/patch/detected` |
| Trace id/run id display | Supported | Reads trace metadata after turn completion |
| Session resume | Supported | Runtime can load sessions; TUI supports `/sessions`, `/resume <id-or-prefix>`, and an interactive `Ctrl-R` session picker |
| Interrupt | Not complete | UI shows intent, but `AgentLoop` has no cooperative cancellation token yet |
| Slash commands | Partial | Built-ins cover help/session/status/new/clear/custom command listing; custom command routing works; interactive picker UI is not complete |
| Mention/file search popup | Not complete | Needs indexed file search and popup UI |
| Approval overlay | Not complete | Needs App Bridge approval request/response protocol |
| MCP elicitation forms | Not complete | Needs typed question/elicitation UI |
| Image attachment rows | Not complete | Needs multimodal message support |
| TUI snapshot tests | Not complete | Current tests cover formatter/state, not terminal render snapshots |

## Codex TUI Mapping

Codex TUI modules worth mirroring over time:

| Codex module | OpenAgent target |
| --- | --- |
| `app_server_session` | `openagent.app_server.runtime` |
| `app/app_server_events.rs` | `openagent.app_server.protocol` |
| `bottom_pane/chat_composer` | `openagent.tui.state` + curses input |
| `bottom_pane/slash_commands` | future `openagent.tui.commands` |
| `bottom_pane/approval_overlay` | future approval protocol |
| `history_ui` / `resume_picker` | future session picker |
| `token_usage` / footer | future runtime budget display |

## Completion Condition for "Full TUI"

OpenAgent can be considered fully TUI-capable when:

1. The TUI can start, resume, fork, and archive sessions.
2. A running turn can be interrupted through a real cancellation token.
3. Tool calls, tool results, patches, runtime warnings, context budget, usage, and trace are all visible.
4. Slash commands cover session, trace, model, tools, skills, memory, and compact operations.
5. Permission approvals and question/elicitation requests are interactive.
6. The renderer has snapshot-style tests or deterministic text render tests.
7. Web Console and TUI share the same App Bridge protocol.
