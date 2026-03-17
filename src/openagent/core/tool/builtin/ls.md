# ls

Lists files and directories in a path as a small tree view.

## Usage
- `path` is optional and defaults to `session_root`.
- `ignore` is optional and accepts extra glob patterns to hide paths.
- The tool applies a default ignore set for large generated folders such as `.git/`, `node_modules/`, `dist/`, and `__pycache__/`.
- At most 100 files are included in the rendered tree. If more exist, the result is marked as truncated.

## Notes
- Prefer `glob` and `grep` when you already know what to search for.
- The target path must stay inside `session_root`.
