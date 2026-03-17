# glob

Fast file pattern matching tool for the current workspace.

## Usage
- `pattern` is required and supports glob syntax such as `**/*.py` or `src/**/*.ts`.
- `path` is optional and defaults to `session_root`.
- Results are sorted by modification time, newest first.
- At most 100 matches are returned. If more exist, the tool marks the result as truncated.

## Notes
- Use this tool when you need to find files by name or path pattern.
- The search root must stay inside `session_root`.
- When you already know the file names to read, it is often useful to call multiple `read` tools in parallel after globbing.
