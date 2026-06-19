# Rust Rewrite Progress

Record goal receipts at the top of this file. A goal is not complete until its
verification evidence is listed here.

---

## 2026-06-19 Goal 1 - Rust Workspace

Status: complete.

Changed:

- Added a root Cargo workspace.
- Added initial Rust crate boundaries for protocol, core, tools, provider,
  session, swarm, MCP, eval, CLI, App Bridge server/client, TUI, and HTTP
  runtime.
- Added lightweight crate smoke tests so each crate compiles and proves its
  intended dependency boundary.
- Added placeholder binaries for `openagent`, `openagent-swarm`,
  `openagent-tui`, and `openagent-http-runtime`.
- Added a Rust GitHub Actions workflow for fmt, clippy, and tests.
- Added `target/` to `.gitignore`.

Verification:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
PYTHONPATH=src:src/tests python -m unittest src/tests/test_rust_rewrite_fixtures.py
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p 'test_*.py'
```

Evidence:

- Rust workspace tests: 13 crate libraries plus 4 placeholder binaries compile;
  all crate smoke tests pass.
- Goal 0 fixture drift test: 1 test OK.
- Full Python baseline: 422 tests OK.

Residual risks:

- Goal 1 intentionally contains only crate boundaries and smoke tests. Runtime
  behavior migration starts in Goal 2.
- GitHub Actions was added but remote CI status must be checked after the branch
  is pushed.

Next:

- Goal 2: implement Rust protocol/data model types and compare serde JSON
  against the Goal 0 golden fixtures.

---

## 2026-06-19 Goal 0 - Python Behavior Oracle

Status: complete.

Changed:

- Added `doc/rust-rewrite-plan.md` with Goal 0 through Goal 14 gates and final
  no-Python acceptance criteria.
- Added `doc/rust-rewrite-parity-matrix.md` mapping Python production surfaces
  to Rust crate ownership and removal gates.
- Added deterministic golden fixtures under `tests/golden/rust_rewrite/`.
- Added `scripts/rust_rewrite/capture_golden_fixtures.py` to regenerate Goal 0
  fixtures from the current Python runtime.
- Added `src/tests/test_rust_rewrite_fixtures.py` to detect fixture drift.
- Changed `openagent.core.provider.__init__` to lazy-load `create_provider`,
  removing an eager import cycle so low-level provider metadata and message
  materialization can be imported independently.

Verification:

```bash
python scripts/rust_rewrite/capture_golden_fixtures.py --output tests/golden/rust_rewrite
PYTHONPATH=src:src/tests python -m unittest src/tests/test_rust_rewrite_fixtures.py
python -m py_compile src/openagent/core/provider/__init__.py scripts/rust_rewrite/capture_golden_fixtures.py
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p 'test_*.py'
```

Evidence:

- Goal 0 fixture drift test: 1 test OK.
- Full Python baseline: 422 tests OK.

Residual risks:

- Goal 0 intentionally excludes live model, network, MCP server, remote sandbox,
  and Docker smoke checks. Those move into later goal gates with mock and smoke
  tests.
- The current branch was created from `origin/main` to avoid mixing an unrelated
  local `main` commit that was ahead of origin.

Next:

- Goal 1: create the Rust Cargo workspace and first empty crates with
  `cargo fmt`, `cargo clippy`, and `cargo test` gates.
