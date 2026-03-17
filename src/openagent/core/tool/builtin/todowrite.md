Use this tool to create and manage a structured task list for the current coding session. It helps track progress, organize complex work, and show the user a clear execution plan.

## When to use it
- For tasks with 3 or more meaningful steps
- For non-trivial refactors, debugging sessions, or multi-file changes
- When the user explicitly asks for a todo list or plan
- After new requirements arrive and the plan should be updated
- When finishing a task so it can be marked completed immediately
- When starting a new task so it can be marked `in_progress`

## Input shape
- Pass a `todos` array.
- Each todo item should contain:
  - `id`: unique identifier for the item
  - `content`: short actionable task description
  - `status`: one of `pending`, `in_progress`, `completed`, `cancelled`
  - `priority`: one of `high`, `medium`, `low`

## Good habits
- Keep only one todo in `in_progress` whenever possible.
- Mark items complete as soon as they are finished.
- Break large requests into smaller concrete items.
- Update the list whenever priorities or requirements change.

## When not to use it
- Single trivial one-step tasks
- Purely conversational or informational requests
- Simple commands with immediate results
