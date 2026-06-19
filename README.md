# OpenAgent

OpenAgent is now a Rust-only agent harness workspace. The Python runtime,
Python package metadata, examples, and Python unittest tree have been removed
as part of the Goal 0-14 rewrite.

The repository keeps the Python golden JSON files captured during the rewrite
as stable compatibility artifacts. Runtime code, CLIs, protocol models, tools,
session/trace handling, provider adapters, swarm orchestration, App Bridge,
TUI state, HTTP runtime contracts, and eval/benchmark contracts now live in
Rust crates under `crates/`.

## Binaries

```bash
cargo run -p openagent-cli --bin openagent
cargo run -p openagent-cli --bin openagent -- doctor --format json
cargo run -p openagent-swarm --bin openagent-swarm -- --help
cargo run -p openagent-tui --bin openagent-tui -- --help
cargo run -p openagent-http-runtime --bin openagent-http-runtime -- --health-json
```

## Workspace

```text
crates/openagent-protocol          Shared serde protocol types
crates/openagent-core              Agent loop, context, permissions, skills
crates/openagent-tools             Tool registry and built-in workspace tools
crates/openagent-session           Session store, trace, observability
crates/openagent-provider          Provider metadata and stream normalization
crates/openagent-mcp               MCP config/runtime contracts
crates/openagent-swarm             Swarm runner orchestration
crates/openagent-cli               OpenAgent CLI command layer
crates/openagent-app-server        App Bridge server protocol/state
crates/openagent-app-server-client App Bridge client helpers
crates/openagent-tui               TUI control state
crates/openagent-http-runtime      HTTP runtime binary contracts
crates/openagent-eval              Eval, CI gate, benchmark integrations
```

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Additional no-Python check:

```bash
git ls-files '*.py' pyproject.toml
```

The command above should print nothing.

## Docker Smoke

The HTTP runtime has a dedicated Dockerfile contract:

```bash
docker build -f Dockerfile.openagent-http-runtime -t openagent-http-runtime .
docker run --rm openagent-http-runtime --health-json
```

Local Docker must be running for the image smoke test.

## Rewrite Receipts

- [Rust rewrite plan](doc/rust-rewrite-plan.md)
- [Rust rewrite parity matrix](doc/rust-rewrite-parity-matrix.md)
- [Rust rewrite progress](doc/rust-rewrite-progress.md)

## License

UNLICENSED.
