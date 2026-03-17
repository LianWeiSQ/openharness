from __future__ import annotations

"""
Builtin tools entrypoint.

- 内置工具也按“插件”形式组织：每个模块提供 register(registry)。
- ToolkitAdapter.load_builtin() 会调用 register_builtin_tools() 完成注册。
"""

from ..registry import ToolRegistry



def register_builtin_tools(registry: ToolRegistry) -> None:
    from . import file as file_tools
    from . import memory as memory_tools
    from . import search as search_tools
    from . import shell as shell_tools
    from . import todo as todo_tools
    from . import web as web_tools

    file_tools.register(registry)
    shell_tools.register(registry)
    search_tools.register(registry)
    web_tools.register(registry)
    memory_tools.register(registry)
    todo_tools.register(registry)


__all__ = ["register_builtin_tools"]
