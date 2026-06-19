from __future__ import annotations

import filecmp
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "rust_rewrite" / "capture_golden_fixtures.py"
GOLDEN_DIR = REPO_ROOT / "tests" / "golden" / "rust_rewrite"


class RustRewriteFixtureTests(unittest.TestCase):
    def test_goal0_fixtures_are_reproducible(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            completed = subprocess.run(
                [sys.executable, str(SCRIPT), "--output", str(tmp)],
                cwd=str(REPO_ROOT),
                check=False,
                text=True,
                capture_output=True,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)

            expected = sorted(path.name for path in GOLDEN_DIR.glob("*.json"))
            actual = sorted(path.name for path in tmp.glob("*.json"))
            self.assertEqual(actual, expected)
            for name in expected:
                self.assertTrue(
                    filecmp.cmp(GOLDEN_DIR / name, tmp / name, shallow=False),
                    f"fixture drifted: {name}",
                )


if __name__ == "__main__":
    unittest.main()
