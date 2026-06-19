# Rust Rewrite Plan

This document is the control surface for the full OpenAgent Python-to-Rust
rewrite. The migration target is strict: production OpenAgent code must be Rust
only at the end of Goal 14.

## Mission

Rewrite OpenAgent from Python to Rust without losing user-visible behavior,
runtime safety, or observability. Python remains the behavior oracle only during
the migration. Each goal must produce verification evidence before it can be
closed or pushed.

## Terminal Acceptance

The rewrite is complete only when all of these are true:

- `cargo test --workspace` passes.
- `cargo clippy --all-targets -- -D warnings` passes.
- `cargo fmt --check` passes.
- CLI, HTTP runtime, App Bridge/TUI, swarm, trace/session, and eval smoke checks
  pass.
- Production paths contain no Python runtime files:

```bash
rg --files openagent openagent-runtime-http \
  -g '*.py' -g 'pyproject.toml' -g 'requirements*.txt' -g 'setup.py' -g 'setup.cfg'
```

The command must return no production files. Archived Python is allowed only in
an explicitly non-production archive path with no build, CI, Docker, or runtime
references.

## Goal Gates

| Goal | Outcome | Gate |
| --- | --- | --- |
| Goal 0 | Python behavior is frozen as migration oracle. | Golden fixtures, baseline tests, and module ownership matrix exist. |
| Goal 1 | Rust workspace exists. | Cargo workspace checks run in CI/local verification. |
| Goal 2 | Protocol/data models are Rust. | Rust serde snapshots match Goal 0 fixtures. |
| Goal 3 | Swarm kernel is Rust. | Rust `openagent-swarm` passes function/subprocess/http/a2a parity smoke. |
| Goal 4 | Session, trace, and observability are Rust. | JSONL/session fixtures are compatible or migrated with tests. |
| Goal 5 | Workspace runtime and tools are Rust. | Tool tests cover path safety, permissions, truncation, and metadata. |
| Goal 6 | Permission, policy, and context are Rust. | Context budget/pack/permission fixtures pass. |
| Goal 7 | Providers are Rust. | Mock streaming/tool-call/usage/error tests pass. |
| Goal 8 | AgentLoop is Rust. | Multi-step loop parity and pause/retry/warning tests pass. |
| Goal 9 | MCP is Rust. | MCP config, discovery, auth, redaction, and tool-call tests pass. |
| Goal 10 | CLI is Rust. | Existing command JSON/table fixtures pass. |
| Goal 11 | App Bridge/TUI are Rust. | SSE replay, auth, attach, interrupt, approval, and control tests pass. |
| Goal 12 | HTTP runtime is Rust. | API parity and Docker smoke pass without Python. |
| Goal 13 | Eval/benchmark/integrations are Rust. | Eval reports and benchmark adapters pass parity checks. |
| Goal 14 | Python is removed. | Terminal acceptance command returns no production Python files. |

## Goal 0 Definition

Goal 0 is complete when:

- The current Python test baseline has been run and recorded.
- Golden fixtures exist for stable protocol and low-side-effect behavior.
- A parity matrix maps every Python production area to a Rust ownership target.
- A repeatable fixture capture script exists.
- A test verifies fixture generation is stable.

Goal 0 deliberately avoids live model, network, or sandbox calls. Those are
covered by later mock/smoke gates so this first migration contract stays stable.

## Release Discipline

Each completed goal gets:

- a focused commit;
- verification commands in the commit/PR notes or progress receipt;
- a push to GitHub;
- no unrelated staged files.

If the worktree contains unrelated changes, stage explicit paths only.
