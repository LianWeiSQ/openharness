"""
Repo-local bootstrap package.

This makes `import openagent` work from the repo root without installing the package
or setting `PYTHONPATH=src`.

When installed with `pip install -e .`, the real package lives under
`src/openagent/` and this file is not included.
"""

from __future__ import annotations

from pathlib import Path


def _extend_package_path() -> None:
    pkg_dir = Path(__file__).resolve().parent
    src_pkg = pkg_dir / "src" / "openagent"
    if not src_pkg.is_dir():
        return
    # Allow `openagent.core...` to resolve from the src-layout folder.
    # - 本仓库的 Python 包采用 src/ 布局：实际源码在 src/openagent/
    # - 如果你没有执行 `pip install -e openagent`，直接运行脚本时可能找不到包
    # - 这里通过扩展 __path__，让 import openagent.* 能够自动找到 src 下的实现
    __path__.append(str(src_pkg))  # type: ignore[name-defined]


_extend_package_path()
