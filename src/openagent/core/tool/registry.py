from __future__ import annotations

"""
ToolRegistry - 工具注册表（贴合 OpenCode 的 ToolRegistry 思路）。

设计目标：
- ToolDefinition 是唯一事实来源：id/description(parameters schema)/execute/危险标记/分组
- description 在注册完成后就是“纯字符串”（Markdown），对上层/模型语义一致
- 支持插件目录扫描：按文件名做 namespace，`foo.py` → `foo` / `foo_bar`

插件协议（更干净、可控）：
- 每个插件模块必须提供 `register(registry: ToolRegistry) -> None`
- registry 由宿主传入（避免 import 副作用写全局状态）
"""

import hashlib
import importlib.util
import sys
from dataclasses import replace
from pathlib import Path
from typing import Any, Callable, TypeVar

from .definition import ToolContext, ToolDefinition, ToolExecutionSchema, ToolExecutionScope, ToolOutput

T = TypeVar("T")


class ToolRegistry:
    def __init__(
        self,
        *,
        _tools: dict[str, ToolDefinition] | None = None,
        _namespace: str | None = None,
    ) -> None:
        self._tools = _tools if _tools is not None else {}
        self._namespace = _namespace

    def scoped(self, namespace: str) -> "ToolRegistry":
        """Return a registry view that prefixes tool ids with a namespace."""

        return ToolRegistry(_tools=self._tools, _namespace=namespace)

    def _qualify_id(self, tool_id: str) -> str:
        if not self._namespace:
            return tool_id
        if tool_id == "default":
            return self._namespace
        return f"{self._namespace}_{tool_id}"

    def register(self, tool: ToolDefinition) -> None:
        """Register/overwrite a tool definition."""

        qualified_id = self._qualify_id(tool.id)
        if qualified_id != tool.id:
            tool = replace(tool, id=qualified_id)
        self._tools[tool.id] = tool

    def get(self, tool_id: str) -> ToolDefinition | None:
        return self._tools.get(tool_id)

    def all(self) -> list[ToolDefinition]:
        return list(self._tools.values())

    def clear(self) -> None:
        self._tools.clear()

    def load_markdown(self, *, description_md: str, caller_file: str) -> str:
        """
        读取 Markdown 描述文件。

        - 相对路径：相对 caller_file 所在目录
        - 绝对路径：直接读取
        """

        md_path = Path(description_md)
        if not md_path.is_absolute():
            md_path = Path(caller_file).resolve().parent / md_path
        return md_path.read_text(encoding="utf-8").strip()

    def _resolve_description(
        self,
        *,
        execute_func: Callable[[Any, ToolContext], Any],
        description_md: str | None,
        description: str | None,
    ) -> str:
        caller_file = execute_func.__code__.co_filename
        if description_md:
            md_path = Path(description_md)
            if not md_path.is_absolute():
                md_path = Path(caller_file).resolve().parent / md_path
            if md_path.exists():
                return md_path.read_text(encoding="utf-8").strip()
        if description is not None:
            return description
        raise ValueError("Tool description is required: provide description_md or description")

    def define_tool(
        self,
        *,
        tool_id: str,
        parameters: type,
        description_md: str | None = None,
        description: str | None = None,
        group: str = "default",
        dangerous: bool = False,
        execution_scope: ToolExecutionScope = "host_only",
        execution_schema: ToolExecutionSchema | None = None,
    ) -> Callable[[Callable[[Any, ToolContext], Any]], Callable[[Any, ToolContext], Any]]:
        """
        装饰器：定义一个工具，并注册到当前 registry。

        注意：
        - description 读取/解析在“注册阶段”完成，后续 ToolDefinition.description 始终是字符串（Markdown）
        - 不做“缺 md 必失败”硬约束：md 不存在时允许 fallback 到 description 字符串
        """

        def decorator(execute_func: Callable[[Any, ToolContext], Any]) -> Callable[[Any, ToolContext], Any]:
            desc = self._resolve_description(execute_func=execute_func, description_md=description_md, description=description)
            tool = ToolDefinition(
                id=self._qualify_id(tool_id),
                description=desc,
                parameters=parameters,
                execute=execute_func,
                dangerous=dangerous,
                group=group,
                execution_scope=execution_scope,
                execution_schema=execution_schema or ToolExecutionSchema(),
            )
            self._tools[tool.id] = tool
            return execute_func

        return decorator

    def load_plugins(self, *, tool_paths: list[str], base_dir: Path) -> None:
        """
        从目录/文件加载插件工具。

        约定：
        - 每个插件模块必须提供 `register(registry)` 函数
        - namespace 取自文件名：foo.py → namespace=foo
        """

        for raw in tool_paths:
            p = Path(raw)
            if not p.is_absolute():
                p = (base_dir / p).resolve()
            if p.is_file():
                self._load_plugin_file(p)
                continue
            if p.is_dir():
                self._load_plugin_dir(p)
                continue
            raise FileNotFoundError(str(p))

    def _load_plugin_dir(self, directory: Path) -> None:
        # 贴合 OpenCode：如果目录下存在 tool/ 子目录，则优先扫描 tool/*.py
        scan_dir = directory / "tool" if (directory / "tool").is_dir() else directory
        candidates = sorted(scan_dir.glob("*.py"))
        for f in candidates:
            if f.name == "__init__.py":
                continue
            if f.name.startswith("_"):
                continue
            self._load_plugin_file(f)

    def _load_plugin_file(self, path: Path) -> None:
        if path.suffix.lower() != ".py":
            raise ValueError(f"Tool plugin must be a .py file: {path}")

        namespace = path.stem
        mod = _import_module_from_path(path)
        fn = getattr(mod, "register", None)
        if not callable(fn):
            raise ValueError(f"Tool plugin {path} must define register(registry)")
        fn(self.scoped(namespace))


def _import_module_from_path(path: Path):
    # 使用路径 hash 生成稳定且不冲突的模块名
    h = hashlib.sha256(str(path).encode("utf-8")).hexdigest()[:12]
    module_name = f"openagent_tool_{path.stem}_{h}"
    if module_name in sys.modules:
        return sys.modules[module_name]

    spec = importlib.util.spec_from_file_location(module_name, str(path))
    if spec is None or spec.loader is None:
        raise ImportError(f"Failed to load tool plugin: {path}")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = mod
    spec.loader.exec_module(mod)
    return mod


__all__ = ["ToolRegistry", "ToolDefinition", "ToolContext", "ToolOutput"]
