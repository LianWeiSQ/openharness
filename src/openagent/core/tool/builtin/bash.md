# bash

Executes a shell command inside the current session workspace.

## Usage
- `command` is required.
- `timeout` is optional and defaults to `120000` milliseconds.
- `workdir` is optional and defaults to `session_root`.
- `description` is optional but helpful for explaining what the command is doing.

## Safety
- `workdir` must stay inside `session_root`.
- Delete commands such as `rm`, `rmdir`, `del`, `erase`, `Remove-Item`, `shred`, and `unlink` are blocked.
- Prefer specialized tools for file operations instead of shell commands:
  read files with `read`, search with `glob` or `grep`, edit with `edit`, and write with `write`.

## Output
- Returns combined `stdout` and `stderr`.
- Output truncation is handled by `ToolkitAdapter`, which writes the full output to `.openagent/tool_output/<call_id>.txt` when necessary.
