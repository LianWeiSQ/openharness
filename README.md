# OpenHarness

OpenHarness is now a Rust-only agent harness workspace. The legacy runtime,
package metadata, examples, and old unittest tree have been removed as part of
the Goal 0-14 rewrite.

The repository keeps the Python golden JSON files captured during the rewrite
as stable compatibility artifacts. Runtime code, CLIs, protocol models, tools,
session/trace handling, provider adapters, swarm orchestration, App Bridge,
TUI state, HTTP runtime contracts, and eval/benchmark contracts now live as
top-level Rust workspace modules.

## Binaries

```bash
cargo run -p openagent-cli --bin openagent
cargo run -p openagent-cli --bin openagent -- doctor --format json
cargo run -p openagent-swarm --bin openagent-swarm -- --help
cargo run -p openagent-tui --bin openagent-tui -- --help
cargo run -p openagent-http-runtime --bin openagent-http-runtime -- --health-json
```

## Workspace Layout

```text
src/              Core agent engine crate plus internal Rust libraries
  protocol/      Shared serde protocol types
  provider/      Provider metadata and stream normalization
  session/       Session store, trace, observability
  tools/         Tool registry and built-in workspace tools
  mcp/           MCP config/runtime contracts
cli/             OpenAgent CLI command layer
runtime/         App Bridge, HTTP runtime, TUI, and bundled web static assets
swarm/           Swarm runner orchestration
eval/            Eval, CI gate, benchmark integrations
skill/           Built-in prompts, tool descriptions, and skill libraries
```

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Additional legacy runtime check:

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
