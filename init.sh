#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

echo "== OpenAgent quick maintenance check =="
echo "repo: $(pwd)"

python -m json.tool tasks.json >/dev/null
python -m py_compile \
  src/openagent/core/trace/exporter.py \
  src/openagent/core/eval/runner.py \
  src/openagent/core/message_materializer.py

PYTHONPATH=src:src/tests python -m unittest \
  src/tests/test_trace.py \
  src/tests/test_eval_runner.py \
  src/tests/test_context_budget.py

echo "== git status =="
git status --short

echo "== next =="
echo "Read tasks.json and pick the first high-priority pending or in_progress task."
