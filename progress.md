# Project Progress

> Append new session notes at the top. Use `tasks.json` as the active task queue and `doc/maintenance.md` as the human-readable issue report.

---

## 2026-06-22 TUI Agent Picker Slice

- Upgraded `/agents` from a passive list command into an OpenCode-style keyboard agent/profile picker.
- Added a TUI agent picker dock with query filtering, Up/Down/Tab selection, Enter-to-select, and Esc-to-close.
- Wired the picker through `TerminalEventHandler::list_agents`; `AppBridgeTerminalHandler` now backs it with `RemoteRuntimeClient::agents`, so opening the picker calls real `GET /api/agents`.
- Selection reuses the existing `/agent <id>` path and writes the current session profile through `PATCH /api/sessions/{session_id}`.
- Updated `/tui/open-agents` remote control to open the same picker path, with direct agent payload support and handler-backed fetch support.
- Added keyflow, remote control, terminal render snapshot, and App Bridge handler smoke coverage.

Verification:

```bash
cargo test -q -p openagent-tui key_event_flow_opens_agent_picker_filters_and_selects
cargo test -q -p openagent-tui remote_control_open_agents_dispatches_picker_fetch
cargo test -q -p openagent-tui terminal_render_snapshot_contains_agent_picker_overlay
cargo test -q -p openagent-tui app_bridge_terminal_agent_picker_fetches_and_sets_agent
cargo test -q -p openagent-tui
cargo check -q -p openagent-tui -p openagent-app-server-client
```

Residual risk:

- This completes the agent/profile picker path only; variant and thinking pickers still have command/control coverage but are not yet full keyboard docks.
- The App Bridge smoke uses a deterministic in-test bridge server, not a full provider-backed runtime.

## 2026-06-22 TUI Model Picker Slice

- Upgraded `/models` from a passive list command into an OpenCode-style keyboard model picker.
- Added a TUI model picker dock with query filtering, Up/Down/Tab selection, Enter-to-select, and Esc-to-close.
- Wired the picker through `TerminalEventHandler::list_models`; `AppBridgeTerminalHandler` now backs it with `RemoteRuntimeClient::models`, so opening the picker calls real `GET /api/models`.
- Selection reuses the existing `/models <id>` path and writes the current session model through `PATCH /api/sessions/{session_id}`.
- Updated `/tui/open-models` remote control to open the same picker path, with direct model payload support and handler-backed fetch support.
- Added keyflow, remote control, terminal render snapshot, and App Bridge handler smoke coverage.

Verification:

```bash
cargo test -q -p openagent-tui key_event_flow_opens_model_picker_filters_and_selects
cargo test -q -p openagent-tui remote_control_open_models_dispatches_picker_fetch
cargo test -q -p openagent-tui terminal_render_snapshot_contains_model_picker_overlay
cargo test -q -p openagent-tui app_bridge_terminal_model_picker_fetches_and_sets_model
cargo test -q -p openagent-tui
cargo check -q -p openagent-tui -p openagent-app-server-client
```

Residual risk:

- This completes the model picker path only; agent, variant, and thinking pickers still have command/control coverage but are not yet full keyboard docks.
- The App Bridge smoke uses a deterministic in-test bridge server, not a full provider-backed runtime.

## 2026-06-22 TUI Session Picker Slice

- Upgraded `/sessions [query]` from a passive timeline listing into an OpenCode-style keyboard session picker.
- Added a TUI session picker dock with query text, remote session candidates, Up/Down/Tab selection, Enter-to-resume, and Esc-to-close.
- Wired the picker through the real `TerminalEventHandler::search_sessions` boundary; `AppBridgeTerminalHandler` now backs it with `RemoteRuntimeClient::search_sessions`, so `/sessions smoke` calls `GET /api/sessions?query=smoke`.
- Updated remote control `/tui/open-sessions` to open the same picker path instead of only appending a hint line.
- Added coverage for key event flow, remote control dispatch, terminal render snapshot, and App Bridge handler search/resume smoke.

Verification:

```bash
cargo test -q -p openagent-tui key_event_flow_opens_session_picker_filters_and_resumes
cargo test -q -p openagent-tui remote_control_open_sessions_dispatches_picker_search
cargo test -q -p openagent-tui terminal_render_snapshot_contains_session_picker_overlay
cargo test -q -p openagent-tui app_bridge_terminal_session_picker_searches_and_resumes
cargo test -q -p openagent-tui
cargo check -q -p openagent-tui -p openagent-app-server-client
```

Residual risk:

- Picker selection resumes by session id; richer OpenCode-style session detail panes and inline rename/delete/archive actions remain future slices.
- The smoke uses the deterministic in-test App Bridge server, not a full provider-backed runtime session.

## 2026-06-22 TUI Session Transcript Slice

- Added a real App Bridge transcript path for session management parity:
  - `GET /api/sessions/{session_id}/messages?limit=N` in `openagent-http-runtime`
  - `RemoteRuntimeClient::session_messages`
  - TUI `/transcript [limit]` command backed by the remote session store
- The endpoint returns structured persisted messages with role, content, metadata, index, total message count, and a bounded limit.
- The TUI renders a compact chronological transcript summary so a resumed session can be inspected without leaving the terminal.
- Added a runtime client round trip against the real `openagent-http-runtime` binary and a TUI App Bridge handler smoke proving `/transcript 2` sends the expected HTTP request and renders remote messages.

Verification:

```bash
cargo test -q -p openagent-http-runtime --test http_runtime remote_runtime_client_reads_session_transcript
cargo test -q -p openagent-tui app_bridge_terminal_transcript_reads_real_session_messages
cargo test -q -p openagent-app-server-client
cargo check -q -p openagent-http-runtime -p openagent-tui
```

Residual risk:

- Transcript is read-only and compact text rendering only; full interactive session picker/detail navigation remains a later session-management slice.
- The TUI transcript command verifies handler/client/HTTP integration, not full raw-mode terminal drawing.

## 2026-06-22 App Bridge Interaction Keyflow Smoke Slice

- Added a real-handler TUI smoke for permission/question interaction flows.
- The smoke drives approval and question dock key events through `handle_key_event`, `AppBridgeTerminalHandler`, and `RemoteRuntimeClient`, with the deterministic in-test App Bridge server receiving actual HTTP response routes:
  - `POST /api/turns/{turn_id}/approvals/{request_id}`
  - `POST /api/turns/{turn_id}/questions/{request_id}/reply`
- Verified approval quick-pick posts `allow`/`once`, clears the active approval, and applies the returned resolved/completed events into the TUI timeline.
- Verified question option selection posts structured `answers`, clears the active question, and applies the returned resolved/completed events into the TUI timeline.
- This strengthens the permission/question parity evidence beyond state-only dock tests and client-only runtime tests.

Verification:

```bash
cargo test -q -p openagent-tui app_bridge_terminal_interaction_keyflow_posts_real_responses
cargo test -q -p openagent-tui
```

Residual risk:

- The smoke still uses a deterministic in-test App Bridge server rather than the full `openagent-http-runtime` binary.
- It verifies keyflow-to-HTTP response integration, not a full raw-mode PTY loop.

## 2026-06-22 App Bridge Terminal Keyflow Smoke Slice

- Added an end-to-end TUI smoke that drives `handle_key_event` through the real `AppBridgeTerminalHandler` and `RemoteRuntimeClient` over HTTP.
- The smoke uses a deterministic in-test App Bridge server implementing the session and event routes needed by the terminal handler:
  - `GET /api/health`
  - `GET /api/sessions`
  - `POST /api/sessions`
  - `POST /api/sessions/{id}/turns`
  - `GET /api/events?last_event_id=...`
- Verified the TUI keyflow can create a remote session with `/new`, submit a prompt through the App Bridge, apply returned turn events into the timeline, merge usage totals, and poll a runtime warning from global App Bridge SSE.
- This closes the earlier evidence gap where TUI coverage was mostly state-level and did not prove the real terminal handler/client path.

Verification:

```bash
cargo test -q -p openagent-tui app_bridge_terminal_keyflow_smoke_uses_real_remote_handler
cargo test -q -p openagent-tui
```

Residual risk:

- The smoke uses a deterministic in-test App Bridge server, not the full `openagent-http-runtime` binary or a real provider.
- It proves keyflow plus handler/client integration, but not a full PTY raw-mode terminal loop with crossterm polling.

## 2026-06-22 Composer At-Trigger File Picker Slice

- Added OpenCode-style `@` composer trigger: pressing `@` in a normal prompt opens the file picker dock instead of inserting a literal character.
- Preserved slash-command behavior: `@` remains literal inside commands such as `/rename @title`, so command arguments are not hijacked by the composer picker.
- The existing picker path is reused, so after `@` users can type to filter, use Up/Down/Tab, press Enter to insert `@path`, or Esc to close without exiting the TUI.
- Added key event coverage for the `@` trigger and command-literal behavior.

Verification:

```bash
cargo test -q -p openagent-tui key_event_flow_at_opens_file_picker_without_touching_commands
cargo test -q -p openagent-tui key_event_flow_opens_file_picker_filters_and_attaches
cargo test -q -p openagent-tui
```

Residual risk:

- `@` only opens the local workspace file picker; remote URL/resource attachment flows remain future composer work.
- Attachment tokens with whitespace in paths remain unsupported by the submit-time parser.

## 2026-06-22 Composer Modal File Picker Slice

- Upgraded `/files [query]` from a timeline-only listing into a keyboard-driven composer file picker dock.
- Added `TerminalEventHandler::search_files` so the TUI state owns modal/key behavior while the App Bridge handler searches the real active workspace.
- File picker now supports incremental query filtering, Up/Down or Tab selection, Enter-to-attach, and Esc-to-close without triggering global exit or prompt history.
- App Bridge `file.open` / `/tui/open-files` now opens the same picker instead of dispatching a plain `/files` timeline command; `file.select` closes the picker and inserts the selected `@path[:range]` reference.
- Added terminal render snapshot coverage proving the file picker appears in the frame, plus key event flow coverage proving filter/select/attach works end to end inside the TUI event loop.

Verification:

```bash
cargo test -q -p openagent-tui key_event_flow_opens_file_picker_filters_and_attaches
cargo test -q -p openagent-tui terminal_render_snapshot_contains_file_picker_overlay
cargo test -q -p openagent-tui remote_control_file_picker_dispatches_and_selects_into_composer
cargo test -q -p openagent-tui
```

Residual risk:

- Superseded by the Composer At-Trigger File Picker Slice: `@` typed inside a normal draft opens the file picker.
- Attachment tokens with whitespace in paths remain unsupported by the submit-time parser.
- Remote URL/resource/image upload attachment flows remain future composer work.

## 2026-06-22 Composer File Picker Slice

- Added OpenCode-style composer file discovery commands: `/files [query]` searches the active workspace and renders ranked `@path` attachment candidates; `/attach <path[:range]>` inserts a normalized file/image reference back into the prompt composer.
- Added App Bridge TUI controls for file attachment workflows:
  - `/tui/open-files` and `file.open` queue a real `/files <query>` command through the terminal handler.
  - `/tui/select-file`, `file.select`, and publish topics `tui.file.select` / `tui.file.attach` insert `@path`, `@path:line`, or `@path:start-end` into the composer.
- Reused the same fuzzy file matcher for both picker listing and submit-time `@file` expansion so selected refs and direct typed refs resolve consistently.
- Updated App Bridge TUI golden action mapping and added coverage for local `/attach`, fuzzy file listing, image/file refs, and remote control dispatch.

Verification:

```bash
cargo test -q -p openagent-tui composer_file_picker_and_attach_controls_insert_references
cargo test -q -p openagent-tui remote_control_file_picker_dispatches_and_selects_into_composer
cargo test -q -p openagent-tui
```

Residual risk:

- Superseded by the Composer Modal File Picker Slice: `/files` now opens a keyboard-driven TUI dock.
- Attachment tokens with whitespace in paths are rejected because the current submit-time parser is whitespace-token based.
- `/files` is workspace-local; remote resource/URL attachments still remain future composer work.

## 2026-06-22 Agent Variant Thinking Control Slice

- Added App Bridge TUI control actions for `agent.open`, `agent.select`, `variant.open`, `variant.select`, `thinking.open`, and `thinking.select`.
- Wired `/tui/open-agents`, `/tui/select-agent`, `/tui/open-variants`, `/tui/select-variant`, `/tui/open-thinking`, and `/tui/select-thinking` into the same command-dispatch path as model selection.
- Added `tui.agent.*`, `tui.variant.*`, and `tui.thinking.*` publish topic routing so external App Bridge publishers can drive these controls.
- Added tests for picker surfaces and real handler command dispatch, and updated the App Bridge TUI golden action map.

Verification:

```bash
cargo test -q -p openagent-tui remote_control_agent_variant_and_thinking_dispatch_handler_commands
cargo test -q -p openagent-tui control_requests_open_model_theme_and_palette_surfaces
cargo test -q -p openagent-tui
cargo test -q -p openagent-app-server -p openagent-app-server-client -p openagent-tui
cargo test -q -p openagent-http-runtime
cargo check -q -p openagent-cli
```

Residual risk:

- The controls now work through App Bridge, but the visible picker is still timeline/list based rather than a full fuzzy modal.
- Variant/thinking validation is command-level only; the TUI does not yet constrain arbitrary values against runtime-provided capabilities.
- Agent/profile switching is per-session metadata today; richer profile inheritance and per-turn overrides still need product polish.

## 2026-06-22 Interaction Live SSE Resume Slice

- Added complete App Bridge metadata to interaction-resolved events:
  - `turn/approval_resolved` now includes `thread_id` and top-level `request_id`.
  - `item/question/resolved` now includes `session_id`, `thread_id`, resolved `turn_id`, and `status` (`answered`/`dismissed`).
- Added an end-to-end live SSE smoke that runs both question and approval resume flows. The fake provider delays the final model response, and `/api/events` must receive the resolved interaction event before `turn/completed`.
- Confirmed approval/question resume still continues the provider loop and records tool outputs into the next provider request.

Verification:

```bash
cargo test -q -p openagent-http-runtime live_sse_tails_interaction_resolved_events_before_provider_final
cargo test -q -p openagent-http-runtime
cargo test -q -p openagent-app-server-client -p openagent-http-runtime -p openagent-tui
cargo check -q -p openagent-cli
cargo check -q -p openagent-http-runtime
```

Residual risk:

- The interaction dock has keyboard and control-response coverage, but there is still no full terminal live-session smoke that drives the dock through real key input against a running HTTP runtime.
- Approval/question responses are now observable before final answer, but long-running tool stdout/stderr still does not stream incrementally.
- Remaining parity work still includes richer composer UX, fuzzy pickers, message-level undo/revert, and broader terminal render snapshots.

## 2026-06-22 TUI Rendered Diff Slice

- Upgraded TUI patch rendering from generic `status` lines to structured timeline kinds: `patch`, `diff-meta`, `diff-hunk`, `diff-add`, and `diff-del`.
- Added theme-aware colors for rendered diff lines so additions, deletions, hunk headers, and patch markers are visually distinct in the terminal.
- Added undo/redo action hints in `/details` and patch result lines, making reversible file changes discoverable from the TUI.
- Added coverage for `patch/detected` rendering and `/details` undo/redo stack markers.

Verification:

```bash
cargo test -q -p openagent-tui patch_events_render_structured_diff_and_undo_redo_markers
cargo test -q -p openagent-tui
cargo test -q -p openagent-app-server -p openagent-app-server-client -p openagent-tui
cargo test -q -p openagent-http-runtime remote_runtime_client_tracks_file_diff_undo_and_redo
cargo check -q -p openagent-cli
```

Residual risk:

- Diff UX is now visibly structured, but still line-oriented; it is not yet a full split-pane or file-tree diff viewer.
- Undo/redo remains tied to file-change snapshots, not a full OpenCode-style message-level revert/unrevert with prompt restoration.
- The `/details` command exposes the latest patch and stack counts; a richer interactive patch picker remains future work.

## 2026-06-22 App Bridge Session Control Slice

- Fixed `/tui/select-session` so the control response dispatches `/resume <session_id>` to the real terminal handler, keeping App Bridge state and the active remote session in sync.
- Added App Bridge session control aliases for rename/archive/unarchive/delete/fork/children/parent/share/unshare/compact/details/undo/redo.
- Added `tui.session.*` publish-topic routing so external UI publishers can invoke session management actions without falling back to raw command strings.
- Added a remote-control test proving these session controls are dispatched as real handler commands, and updated the App Bridge TUI golden action map.

Verification:

```bash
cargo test -q -p openagent-tui remote_control_session_actions_dispatch_handler_commands
cargo test -q -p openagent-tui remote_control_select_model_dispatches_handler_command
cargo test -q -p openagent-tui
cargo test -q -p openagent-app-server -p openagent-app-server-client -p openagent-tui
cargo test -q -p openagent-http-runtime
cargo check -q -p openagent-cli
```

Residual risk:

- Session controls now reach the real handler command path, but the visible picker UI is still text/list based rather than a full OpenCode-style fuzzy session dialog.
- Delete/archive/share controls still rely on slash-command semantics and do not yet have confirmation modals.
- Child/subagent navigation exists via `/children` and `/parent`, but nested navigation UI polish remains.

## 2026-06-22 Provider Tool Event Live SSE Slice

- Provider-loop tool events now flush to the App Bridge event log as soon as each tool starts and completes, instead of waiting for the final `turn/completed` append.
- Live `/api/events` clients can now see `item/toolCall/started`, `item/toolCall/completed`, rendered output metadata, and diff/patch events during a real provider turn while the next provider call is still in flight.
- Added an end-to-end smoke where the fake Responses provider streams a function call, OpenAgent executes `read`, the second provider call deliberately delays the final answer, and live SSE proves tool events are visible before `turn/completed`.

Verification:

```bash
cargo test -q -p openagent-http-runtime global_sse_live_tails_provider_tool_events_before_final_answer
cargo test -q -p openagent-http-runtime
cargo test -q -p openagent-app-server-client -p openagent-http-runtime -p openagent-tui
cargo check -q -p openagent-cli
cargo check -q -p openagent-http-runtime
```

Residual risk:

- Approval/question request events already flush on pause, but their response/resume phases still deserve a dedicated live-SSE smoke.
- Tool progress is event-level live now; long-running tool stdout/stderr incremental streaming is not yet implemented.
- Remaining OpenCode parity work still includes richer composer, session navigation, rendered diff UX, model/theme picker polish, config/keybinds, and broader terminal E2E snapshots.

## 2026-06-22 Runtime Provider Streaming App Bridge Slice

- Added OpenAI Responses SSE normalization to `openagent-provider` so runtime code can materialize text deltas, tool calls, finish reason, and usage from provider stream chunks.
- Changed the HTTP runtime provider path to request native provider streaming by default (`stream: true`, `Accept: text/event-stream`) while preserving JSON fallback for providers/tests that return non-SSE responses.
- Moved provider calls into the runtime provider loop so first turns plus approval/question resumes share the same streaming path.
- App Bridge live SSE now receives provider text deltas while the upstream provider response is still in flight; already-persisted live events are not appended again at turn completion.
- Added an end-to-end smoke where a fake Responses provider sends one delta, delays completion, and `/api/events` receives the delta before `turn/completed`.

Verification:

```bash
cargo check -q -p openagent-provider -p openagent-http-runtime
cargo test -q -p openagent-http-runtime global_sse_live_tails_provider_stream_delta_before_completion
cargo test -q -p openagent-http-runtime
cargo test -q -p openagent-provider
cargo test -q -p openagent-app-server-client -p openagent-http-runtime -p openagent-tui
cargo check -q -p openagent-cli
```

Residual risk:

- Provider streaming is now real for OpenAI-compatible chat/responses SSE, but Anthropic/native non-OpenAI runtime streaming is not wired in this runtime path yet.
- Tool started/completed events inside the provider loop are still mostly flushed on pause/final completion; this slice focused on model token deltas into live App Bridge SSE.
- The full TUI parity goal still has remaining product surfaces: richer pickers, composer extmarks/attachments, rendered diff UX, and broader terminal render/keyflow E2E.

## 2026-06-22 TUI/App Bridge Parity Push

- Replaced the HTTP runtime plain-turn mock path with a real OpenAI-compatible provider call path.
- Added a bounded runtime provider tool loop: provider tool calls are appended as assistant tool-call messages, executed through the built-in toolkit, recorded as `Role::Tool`, and sent back to the provider for the final answer.
- Wired approval/question pause and resume into the provider loop. `/allow` and `/answer` now resume the pending provider turn instead of only updating local state.
- Added live SSE tail support for EventSource-style clients by handling connections concurrently and streaming new app events until terminal turn events or timeout.
- Added TUI Interaction Dock v1 for approval/question: pending requests render in a focused dock and support keyboard selection, numeric quick-pick, Enter, Esc, and custom question answers.
- Completed App Bridge TUI control paths for model/theme/palette open/select/execute so they no longer hard-return unsupported for those namespaces.

Verification:

```bash
cargo test -q -p openagent-http-runtime
cargo test -q -p openagent-app-server-client -p openagent-http-runtime -p openagent-tui
cargo check -q -p openagent-cli
rg -n "OPENAGENT_MOCK|hello from server|echo:|TUI control unsupported|unsupported.*model|unsupported.*theme|unsupported.*palette" \
  crates/openagent-http-runtime/src/lib.rs \
  crates/openagent-http-runtime/tests/http_runtime.rs \
  crates/openagent-tui/src/lib.rs \
  tests/golden/rust_rewrite/app_bridge_tui.json \
  tests/golden/rust_rewrite/http_runtime.json
```

Residual risk:

- Provider HTTP calls in `openagent-http-runtime` are still non-streaming `.send().text()` calls; live SSE now tails runtime events, but token deltas are not emitted while the upstream model response is still in flight.
- Explicit payload-driven `tool_call(s)` turns remain a bridge/test execution path and do not ask the provider for a final response.
- TUI model/theme/palette now have working control paths, but full OpenCode-style fuzzy picker UI is still basic compared with `DialogModel`/`DialogThemeList`.

## 2026-06-12 Swarm Kernel Proposal (decoupled, agent-agnostic)

- Rewrote `doc/multi-agent.md` around a **standalone, agent-agnostic swarm
  kernel**. The kernel (working package `swarm/`) has **zero openagent
  dependency**; openagent is the *reference adapter* (`OpenAgentRunner`), one
  citizen of the swarm. Any CLI / HTTP / A2A agent can join via the same
  `AgentRunner` protocol. Analogy: MCP standardized tool access; this kernel
  standardizes agent-to-agent orchestration.
- Dependency direction is strict: `openagent → swarm`, never the reverse. A CI
  guard asserts no openagent import under `src/swarm/`.
- Protocol named `AgentRunner` (the existing `AgentAdapter` name is taken by the
  model+config reply-stream adapter). Kernel injected into openagent via
  `ToolContext.extra["swarm"]`.
- Restructured `tasks.json` into kernel tasks `SW-001..SW-008` and openagent
  adapter tasks `OA-018..OA-021`. Phased order:
  `OA-002 → SW-001/002 → OA-018/019 (P0) → SW-003 + OA-020 + SW-007 (P1) →
  SW-004/005 (P2) → SW-006 (P3) → OA-003 → SW-008 + OA-021 (P4)`.
- Decisions locked in `doc/multi-agent.md` §9: workers same model as lead (v0),
  compact-JSON result, failures never raise, Supervisor topology first.
- Open question (§9): out-of-process transport ordering (Subprocess first vs
  A2A first); current lean is subprocess first. Packaging recommendation (§8):
  in-repo `src/swarm/` with enforced zero-import boundary, extract to its own
  repo once the protocol stabilizes.

Verification for this proposal:

```bash
python -m json.tool tasks.json >/dev/null
rg -n "import openagent|from openagent" src/swarm || echo "kernel boundary clean (package not created yet)"
```

---

## 2026-06-11 Maintenance State Initialized

- Added local maintenance entry points:
  - `doc/maintenance.md`
  - `tasks.json`
  - `progress.md`
  - `init.sh`
- Captured known trace/eval/cost/runtime-warning maintenance items from recent OpenAgent work.
- Current active local change is `OA-001`: prevent runtime-only options such as `trace` and `runtime_warnings` from leaking to provider-facing model options.
- Current active local documentation change is `OA-017`: capture static step-budget behavior and closeout risk.
- LangSmith / OpenTelemetry integration has been removed and pushed at `4886a8f`; Langfuse export remains.
- Recommended next task: finish, verify, and commit `OA-001`.

Verification for this maintenance setup:

```bash
python -m json.tool tasks.json >/dev/null
bash init.sh
```
