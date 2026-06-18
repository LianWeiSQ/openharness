# OpenCode CLI/TUI Parity Matrix

This matrix is the audit ledger for bringing OpenAgent's local CLI and TUI
access layers to observable OpenCode parity. A single feature slice is not
"parity" unless every required row below is implemented, verified, linked to a
GitHub issue, and marked complete with evidence.

Reference source: `references/opencode` in the local harness workspace.

## Status Legend

| Status | Meaning |
| --- | --- |
| Supported | OpenAgent has the user-facing capability and tests/smoke evidence. |
| Partial | OpenAgent has adjacent or narrower behavior, but not OpenCode parity. |
| Missing | No OpenAgent user-facing capability exists yet. |
| Deferred | Explicitly accepted non-goal or lower-priority lifecycle behavior. |

| Priority | Meaning |
| --- | --- |
| P0 | Blocks credible CLI/TUI parity for daily coding-agent use. |
| P1 | Important operator workflow or high-frequency ergonomic gap. |
| P2 | Ecosystem, integration, or advanced workflow parity. |
| P3 | Low-level diagnostics or lifecycle parity; may become deferred by decision. |

## Completion Rules

1. Every row must keep an issue link.
2. Work starts only after the row issue is in progress or a narrower child issue
   is linked from the row.
3. A row can move to Supported only after the implementation is merged to
   `main`, the verification command is run, and completion evidence is recorded.
4. The goal is not complete while any P0/P1 row is Partial or Missing.
5. P2/P3 rows must either be Supported or explicitly Deferred with a recorded
   decision explaining why OpenAgent should not mirror OpenCode there.

## Current Baseline

OpenAgent already supports the baseline product CLI and curses TUI:

- CLI entry: `src/openagent/cli/main.py` exposes `tui`, `serve`, `web`,
  `client`, `run`, `session`, `models`, `stats`, `command`, `config`, `auth`,
  and `doctor`.
- TUI entry: `src/openagent/tui/app.py` and `src/openagent/tui/state.py` support
  local sessions, text submission, slash commands, session picker/resume,
  transcript rendering, file mentions, approval allow/deny, and cooperative
  interrupt.
- Baseline tests: `src/tests/test_openagent_cli.py`,
  `src/tests/test_tui_formatting.py`, `src/tests/test_app_server_runtime.py`,
  and `src/tests/test_app_server_server.py`.

Recently completed related slices:

| Issue | Capability | Evidence |
| --- | --- | --- |
| [#38](https://github.com/LianWeiSQ/openagent-ai/issues/38) | TUI `/transcript [limit]` | Merged in `8fda967`; `test_tui_formatting.py` covered transcript rendering. |
| [#39](https://github.com/LianWeiSQ/openagent-ai/issues/39) | `doctor --format json` | Merged in `8fda967`; `test_openagent_cli.py` covered text/json doctor output. |
| [#40](https://github.com/LianWeiSQ/openagent-ai/issues/40) | This parity matrix | Verify this document, issue links, and roadmap ordering. |

## CLI Matrix

| ID | Capability | OpenCode evidence | OpenAgent status | Gap | Priority | Issue | Verification command | Completion evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CLI-01 | `run` advanced flags | `packages/opencode/src/cli/cmd/run.ts` registers `--command`, `--continue`, `--session`, `--fork`, `--share`, `--model`, `--agent`, `--file`, `--format`, `--title`, `--attach`, `--dir`, `--variant`, `--thinking`, and `--dangerously-skip-permissions`. | Partial | OpenAgent `run` has prompt/stdin/file/json/session/custom-command, but lacks attach, fork, share, title, variant, thinking, agent, and skip-permission parity. | P1 | [#41](https://github.com/LianWeiSQ/openagent-ai/issues/41) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_openagent_cli.py` and `PYTHONPATH=src python -m openagent.cli.main run --help` | Pending |
| CLI-02 | Remote App Bridge attach workflow | `packages/opencode/src/cli/cmd/tui/attach.ts` provides `attach <url>` with dir/session/continue/fork/auth options. | Partial | OpenAgent now has `openagent attach <url>` from [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62) with workspace/session/continue and Bearer token support; remaining CLI parity includes OpenCode's fork and username/password auth attach options. | P1 | [#42](https://github.com/LianWeiSQ/openagent-ai/issues/42), [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62) | `PYTHONPATH=src python -m openagent.cli.main attach --help` plus an App Bridge smoke. | Attach CLI vertical path implemented in [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62); [#42](https://github.com/LianWeiSQ/openagent-ai/issues/42) remains open for remaining CLI attach option parity. |
| CLI-03 | MCP management commands | `packages/opencode/src/cli/cmd/mcp.ts` provides add/list/auth/logout/debug flows and remote OAuth support. | Partial | OpenAgent now supports `openagent mcp list/show/add/remove/doctor` for remote MCP config management, JSON/table output, redacted headers, default config resolution, and optional doctor refresh. OAuth auth/logout/debug flows and full OpenCode remote OAuth parity remain tracked in [#65](https://github.com/LianWeiSQ/openagent-ai/issues/65). | P0 | [#43](https://github.com/LianWeiSQ/openagent-ai/issues/43), [#65](https://github.com/LianWeiSQ/openagent-ai/issues/65) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_mcp_config.py src/tests/test_mcp_runtime.py src/tests/test_openagent_cli.py` | Config-management slice implemented in [#43](https://github.com/LianWeiSQ/openagent-ai/issues/43); 41 focused MCP/CLI/runtime tests pass locally. Row remains Partial until [#65](https://github.com/LianWeiSQ/openagent-ai/issues/65) lands. |
| CLI-04 | Provider-aware credentials | `packages/opencode/src/cli/cmd/providers.ts` implements provider login/list/logout, methods, and well-known provider behavior. | Partial | OpenAgent now supports provider-aware `auth` plus `providers` alias login/list/logout, normalized provider ids, provider-specific env metadata, redacted multi-provider credential records, env-only provider discovery, `providers methods [provider]`, and active-provider env/model visibility through the current OpenAI-compatible runtime. Full OpenCode parity still needs native non-OpenAI SDK routing and security-reviewed well-known provider URL login in [#68](https://github.com/LianWeiSQ/openagent-ai/issues/68). | P0 | [#44](https://github.com/LianWeiSQ/openagent-ai/issues/44), [#67](https://github.com/LianWeiSQ/openagent-ai/issues/67), [#68](https://github.com/LianWeiSQ/openagent-ai/issues/68) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_openagent_cli.py`, `PYTHONPATH=src python -m openagent.cli.main providers --help`, and `PYTHONPATH=src python -m openagent.cli.main providers methods --help` | Provider-aware credential layer implemented in [#44](https://github.com/LianWeiSQ/openagent-ai/issues/44). Env-only discovery, auth method metadata, and active provider runtime/model visibility implemented in [#67](https://github.com/LianWeiSQ/openagent-ai/issues/67); 44 focused CLI tests and 7 OpenAI provider tests pass locally. Row remains Partial until [#68](https://github.com/LianWeiSQ/openagent-ai/issues/68) lands native SDK/well-known login parity. |
| CLI-05 | Refreshable verbose model listing | `packages/opencode/src/cli/cmd/models.ts` supports provider filtering, refresh, and verbose output. | Partial | OpenAgent `models` lists current runtime models but lacks refresh and verbose provider detail. | P2 | [#45](https://github.com/LianWeiSQ/openagent-ai/issues/45) | `PYTHONPATH=src python -m openagent.cli.main models --help` and focused CLI tests. | Pending |
| CLI-06 | Session import/share/export parity | `packages/opencode/src/cli/cmd/export.ts`, `import.ts`, and `run.ts --share` cover share/export/import workflows. | Partial | OpenAgent has `session export`, but no import/share/top-level aliases. | P1 | [#46](https://github.com/LianWeiSQ/openagent-ai/issues/46) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_openagent_cli.py` | Pending |
| CLI-07 | Plugin install and config registration | `packages/opencode/src/cli/cmd/plug.ts` installs npm plugins and mutates config. | Missing | No OpenAgent plugin install/config-registration CLI. | P2 | [#47](https://github.com/LianWeiSQ/openagent-ai/issues/47) | `PYTHONPATH=src python -m openagent.cli.main plugin --help` | Pending |
| CLI-08 | Reusable agent profile management | `packages/opencode/src/cli/cmd/agent.ts` supports creating/listing agents with mode, model, and permissions. | Missing | No `openagent agent create/list` for local reusable profiles. | P2 | [#48](https://github.com/LianWeiSQ/openagent-ai/issues/48) | `PYTHONPATH=src python -m openagent.cli.main agent --help` | Pending |
| CLI-09 | Server network parity and ACP mode | `packages/opencode/src/cli/cmd/serve.ts`, `cli/network.ts`, and `cli/cmd/acp.ts` expose hostname/port/mdns/cors and ACP server mode. | Partial | OpenAgent has `serve/web/client`, but lacks mdns/cors parity and ACP mode. | P2 | [#49](https://github.com/LianWeiSQ/openagent-ai/issues/49) | `PYTHONPATH=src python -m openagent.cli.main serve --help` and `acp --help`. | Pending |
| CLI-10 | GitHub agent and PR helpers | `packages/opencode/src/cli/cmd/github.ts` and `pr.ts` implement GitHub agent install/run and PR checkout/share import. | Missing | No OpenAgent GitHub agent helper or PR checkout/import flow. | P2 | [#50](https://github.com/LianWeiSQ/openagent-ai/issues/50) | `PYTHONPATH=src python -m openagent.cli.main github --help` | Pending |
| CLI-11 | Debug and session-store inspection | `packages/opencode/src/cli/cmd/db.ts` and `debug/snapshot.ts` expose database and snapshot diagnostics. | Partial | OpenAgent has internal stores/traces and `doctor`, but no structured debug/session-store CLI. | P3 | [#51](https://github.com/LianWeiSQ/openagent-ai/issues/51) | `PYTHONPATH=src python -m openagent.cli.main debug --help` | Pending |
| CLI-12 | Lifecycle commands | OpenCode docs expose `upgrade` and `uninstall` lifecycle commands. | Missing | OpenAgent has no equivalent; this may be a packaging non-goal. | P3 | [#52](https://github.com/LianWeiSQ/openagent-ai/issues/52) | Decision record plus `openagent upgrade --help` if implemented. | Pending |

## TUI Matrix

| ID | Capability | OpenCode evidence | OpenAgent status | Gap | Priority | Issue | Verification command | Completion evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TUI-01 | Full session manager actions | `packages/opencode/src/cli/cmd/tui/app.tsx`, `dialog-session-list.tsx`, and `routes/session/index.tsx` support list/search/select/delete/rename/share/fork/compact/copy/child navigation. | Partial | OpenAgent has session picker/resume/transcript, but lacks fork, rename, delete, search, share, compact, and child navigation in TUI. | P1 | [#53](https://github.com/LianWeiSQ/openagent-ai/issues/53) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_tui_formatting.py` | Pending |
| TUI-02 | Composer history, stash, and keymap interactions | `footer.prompt.tsx`, `runtime.queue.ts`, and `component/prompt/index.tsx` implement history navigation, slash parsing, prompt stash, and keybindings. | Partial | OpenAgent composer supports text, slash, custom commands, and file mentions, but lacks history ring, stash, editor/leader keymaps. | P1 | [#54](https://github.com/LianWeiSQ/openagent-ai/issues/54) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_tui_formatting.py` | Pending |
| TUI-03 | Rich file and image attachments | `packages/app/src/components/prompt-input/attachments.ts`, `submit.ts`, and `footer.prompt.tsx` handle file/resource/image attachments and mentions. | Partial | OpenAgent supports `@file` text expansion only; no image rows, paste/drop media, resources, or line-range mentions. | P1 | [#55](https://github.com/LianWeiSQ/openagent-ai/issues/55) | `rg -n "image|attachment|file_picker|inject_file_references" src/openagent src/tests` plus focused tests. | Pending |
| TUI-04 | Rich approval dock with diff context | `footer.permission.tsx`, `permission.shared.ts`, and session permission routes support allow once, always allow, reject with note, and diff context. | Partial | OpenAgent supports allow/deny only; no allow-always, rejection note, or rich diff body. | P0 | [#56](https://github.com/LianWeiSQ/openagent-ai/issues/56) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_app_server_runtime.py src/tests/test_tui_formatting.py` | Pending |
| TUI-05 | Question prompt flow | `footer.question.tsx` and `session-data.ts` manage question queues and replies. | Missing | No active App Bridge/TUI question reply path. | P2 | [#57](https://github.com/LianWeiSQ/openagent-ai/issues/57) | `rg -n "question|elicitation|reply" src/openagent src/tests` plus focused tests. | Pending |
| TUI-06 | Diff review and revert workflow | `routes/session/index.tsx` and permission routes render diffs, undo/redo, revert markers, and snapshots. | Partial | OpenAgent formats patch summaries but has no rendered diff, revert marker, undo, or redo workflow. | P0 | [#58](https://github.com/LianWeiSQ/openagent-ai/issues/58) | `rg -n "patch|revert|undo|redo|diff" src/openagent/tui src/tests` plus focused tests. | Pending |
| TUI-07 | Model, agent, and variant switcher | OpenCode registers model/agent/variant list/cycle/favorite commands in `tui/app.tsx` and `run/runtime.ts`. | Partial | OpenAgent TUI header reflects env model only; no interactive picker/switcher. | P0 | [#59](https://github.com/LianWeiSQ/openagent-ai/issues/59) | `rg -n "model|agent|variant" src/openagent/tui src/tests` plus focused tests. | Pending |
| TUI-08 | Interrupt feedback and cancellation states | `run/runtime.ts` and prompt/session routes expose abort/interrupt feedback. | Supported | OpenAgent supports cooperative interrupt, but provider/tool boundary behavior needs clearer UX states. | P1 | [#60](https://github.com/LianWeiSQ/openagent-ai/issues/60) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_app_server_runtime.py -k interrupt` or equivalent unittest selection. | Pending |
| TUI-09 | Session panes and subagent navigation | `runtime.lifecycle.ts`, `routes/session/index.tsx`, and TUI types expose panes, child sessions, and subagent tabs. | Partial | OpenAgent has simple sidebar/details panels, but no subagent tabs, child navigation, or plugin panes. | P2 | [#61](https://github.com/LianWeiSQ/openagent-ai/issues/61) | `rg -n "subagent|sidebar|details|pane" src/openagent/tui src/tests` plus focused tests. | Pending |
| TUI-10 | Interactive App Bridge attach | `cli/cmd/tui/attach.ts`, `run.ts --interactive --attach`, and server TUI-control routes support attaching a TUI to an existing server. | Partial | OpenAgent now supports `openagent attach <url>` for a local curses TUI backed by App Bridge REST/SSE sessions, turns, interrupts, and approvals. Remaining OpenCode parity gaps include global `/event` transport in [#63](https://github.com/LianWeiSQ/openagent-ai/issues/63) and `/tui/control` style server-side TUI control in [#66](https://github.com/LianWeiSQ/openagent-ai/issues/66). | P0 | [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62), [#63](https://github.com/LianWeiSQ/openagent-ai/issues/63), [#66](https://github.com/LianWeiSQ/openagent-ai/issues/66) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_openagent_cli.py src/tests/test_app_server_server.py src/tests/test_tui_formatting.py src/tests/test_tui_remote_runtime.py` plus `PYTHONPATH=src python -m openagent.cli.main attach --help`. | [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62) interactive attach vertical path implemented: `attach` starts the TUI against remote App Bridge sessions/events, with focused CLI/runtime/TuiState tests. Row remains Partial until [#63](https://github.com/LianWeiSQ/openagent-ai/issues/63) and [#66](https://github.com/LianWeiSQ/openagent-ai/issues/66) land. |
| TUI-11 | Curses TUI consumes App Bridge SSE | `stream.transport.ts`, `stream.ts`, and `session-data.ts` separate event transport from rendering. | Partial | OpenAgent TUI consumes local `OpenAgentAppRuntime`; remote SSE event transport is client-only. | P1 | [#63](https://github.com/LianWeiSQ/openagent-ai/issues/63) | `PYTHONPATH=src:src/tests python -m unittest src/tests/test_app_server_protocol.py src/tests/test_app_server_server.py src/tests/test_tui_formatting.py` | Pending |
| TUI-12 | Command palette, keymap, and plugin layer | `tui/app.tsx`, `keymap.tsx`, and `TuiPluginRuntime` provide command palette/keymaps/plugin routes. | Missing | No OpenAgent TUI palette, configurable keymap, or plugin slots. | P2 | [#64](https://github.com/LianWeiSQ/openagent-ai/issues/64) | `rg -n "palette|keymap|plugin|leader" src/openagent/tui src/tests` plus focused tests. | Pending |

## P0 Implementation Queue

P0 rows are the first implementation tranche after this matrix:

1. [CLI-03 / #65](https://github.com/LianWeiSQ/openagent-ai/issues/65):
   MCP OAuth auth/logout/debug parity. The [#43](https://github.com/LianWeiSQ/openagent-ai/issues/43)
   config-management slice unlocks existing MCP runtime/config work, but
   OpenCode's OAuth-oriented MCP flows remain a P0 CLI gap.
2. [CLI-04 / #68](https://github.com/LianWeiSQ/openagent-ai/issues/68):
   native provider SDK routing and well-known login parity. The
   [#44](https://github.com/LianWeiSQ/openagent-ai/issues/44) credential slice
   and [#67](https://github.com/LianWeiSQ/openagent-ai/issues/67) env-discovery
   slice add the local storage, auth-method metadata, and active-provider base
   needed for this.
3. [TUI-10 / #63](https://github.com/LianWeiSQ/openagent-ai/issues/63) and
   [#66](https://github.com/LianWeiSQ/openagent-ai/issues/66): finish attach-era
   global App Bridge event transport and server-side TUI control routes after
   the [#62](https://github.com/LianWeiSQ/openagent-ai/issues/62) interactive
   attach vertical path.
4. [TUI-04 / #56](https://github.com/LianWeiSQ/openagent-ai/issues/56) and
   [TUI-06 / #58](https://github.com/LianWeiSQ/openagent-ai/issues/58): rich
   approval, diff, and revert UX. These are the trust boundary for coding-agent
   file changes.
5. [TUI-07 / #59](https://github.com/LianWeiSQ/openagent-ai/issues/59): model,
   agent, and variant switcher for everyday operator control.

## Maintenance Checklist

For every future parity slice:

1. Update the row issue from backlog to in progress.
2. Implement on a dedicated branch named `codex/<row-id>-<short-title>`.
3. Run the row verification command and the nearest CLI/TUI regression tests.
4. Update this matrix row with completion evidence.
5. Push the branch, then have the main agent review and merge to `main`.
6. Close the issue only after `origin/main` contains the commit and evidence.
