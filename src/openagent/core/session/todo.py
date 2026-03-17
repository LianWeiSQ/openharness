from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

TodoStatus = Literal["pending", "in_progress", "completed", "cancelled"]
TodoPriority = Literal["high", "medium", "low"]

VALID_TODO_STATUSES = {"pending", "in_progress", "completed", "cancelled"}
VALID_TODO_PRIORITIES = {"high", "medium", "low"}


@dataclass(frozen=True, slots=True)
class TodoItem:
    content: str
    status: TodoStatus = "pending"
    priority: TodoPriority = "medium"
    id: str = ""

    def to_dict(self) -> dict[str, str]:
        return {
            "content": self.content,
            "status": self.status,
            "priority": self.priority,
            "id": self.id,
        }



def todo_from_payload(payload: dict[str, Any], *, index: int = 0) -> TodoItem:
    content = str(payload.get("content") or "").strip()
    if not content:
        raise ValueError("Todo content is required")

    status = str(payload.get("status") or "pending").strip() or "pending"
    if status not in VALID_TODO_STATUSES:
        raise ValueError(f"Invalid todo status: {status}")

    priority = str(payload.get("priority") or "medium").strip() or "medium"
    if priority not in VALID_TODO_PRIORITIES:
        raise ValueError(f"Invalid todo priority: {priority}")

    todo_id = str(payload.get("id") or f"todo-{index + 1}").strip() or f"todo-{index + 1}"
    return TodoItem(content=content, status=status, priority=priority, id=todo_id)



def todos_from_payload(payload: list[dict[str, Any]] | list[TodoItem] | None) -> list[TodoItem]:
    if not payload:
        return []
    todos: list[TodoItem] = []
    for index, item in enumerate(payload):
        if isinstance(item, TodoItem):
            todos.append(item)
            continue
        if not isinstance(item, dict):
            raise ValueError("Each todo must be an object")
        todos.append(todo_from_payload(item, index=index))
    return todos



def todos_to_dicts(todos: list[TodoItem]) -> list[dict[str, str]]:
    return [todo.to_dict() for todo in todos]


__all__ = [
    "TodoItem",
    "TodoPriority",
    "TodoStatus",
    "VALID_TODO_PRIORITIES",
    "VALID_TODO_STATUSES",
    "todo_from_payload",
    "todos_from_payload",
    "todos_to_dicts",
]
