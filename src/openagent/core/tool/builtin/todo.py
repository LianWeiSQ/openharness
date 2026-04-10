from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from ...session.session import Session
from ...session.todo import TodoItem, todos_from_payload, todos_to_dicts
from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry

TodoStatus = Literal["pending", "in_progress", "completed", "cancelled"]
TodoPriority = Literal["high", "medium", "low"]


@dataclass
class TodoEntry:
    content: str = field(metadata={"description": "Brief description of the task"})
    status: TodoStatus = field(default="pending", metadata={"description": "Current status of the task"})
    priority: TodoPriority = field(default="medium", metadata={"description": "Priority level of the task"})
    id: str = field(default="", metadata={"description": "Unique identifier for the todo item"})


@dataclass
class TodoWriteParameters:
    todos: list[TodoEntry] = field(metadata={"description": "The updated todo list"})


@dataclass
class TodoReadParameters:
    pass



def _session_from_ctx(ctx: ToolContext) -> Session | None:
    session = ctx.extra.get("session")
    if isinstance(session, Session):
        return session
    return None



def _todo_storage_path(ctx: ToolContext) -> Path:
    session_key = ctx.session_id or "default"
    safe_key = re.sub(r"[^A-Za-z0-9_.-]+", "_", session_key)
    return ctx.session_root / ".openagent" / "todo" / f"{safe_key}.json"



def _load_todos(ctx: ToolContext) -> list[TodoItem]:
    session = _session_from_ctx(ctx)
    path = _todo_storage_path(ctx)
    todos: list[TodoItem] = []

    if path.exists():
        raw = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(raw, list):
            todos = todos_from_payload(raw)

    if session is not None:
        if session.todos:
            return list(session.todos)
        if todos:
            session.set_todos(todos)

    return todos



def _save_todos(ctx: ToolContext, todos: list[TodoItem]) -> None:
    session = _session_from_ctx(ctx)
    if session is not None:
        session.set_todos(todos)

    path = _todo_storage_path(ctx)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(todos_to_dicts(todos), ensure_ascii=False, indent=2), encoding="utf-8")


async def todo_write_tool(args: TodoWriteParameters, ctx: ToolContext) -> ToolOutput:
    todos = todos_from_payload([
        {"content": item.content, "status": item.status, "priority": item.priority, "id": item.id}
        for item in args.todos
    ])
    _save_todos(ctx, todos)
    todo_dicts = todos_to_dicts(todos)
    open_count = len([item for item in todos if item.status != "completed"])
    return ToolOutput(
        title=f"{open_count} todos",
        output=json.dumps(todo_dicts, ensure_ascii=False, indent=2),
        metadata={"todos": todo_dicts},
    )


async def todo_read_tool(_args: TodoReadParameters, ctx: ToolContext) -> ToolOutput:
    todos = _load_todos(ctx)
    todo_dicts = todos_to_dicts(todos)
    open_count = len([item for item in todos if item.status != "completed"])
    return ToolOutput(
        title=f"{open_count} todos",
        output=json.dumps(todo_dicts, ensure_ascii=False, indent=2),
        metadata={"todos": todo_dicts},
    )



def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="todowrite", parameters=TodoWriteParameters, description_md="todowrite.md", group="todo", dangerous=False, execution_scope="agnostic")(
        todo_write_tool
    )
    registry.define_tool(tool_id="todoread", parameters=TodoReadParameters, description_md="todoread.md", group="todo", dangerous=False, execution_scope="agnostic")(
        todo_read_tool
    )


__all__ = ["register"]
