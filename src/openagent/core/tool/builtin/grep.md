# grep

Fast content search tool that uses regular expressions.

## Usage
- `pattern` is required and supports full regex syntax.
- `path` is optional and defaults to `session_root`.
- Use `include` to restrict the search to matching file globs such as `*.py` or `*.{ts,tsx}`.
- `glob` is still accepted as a compatibility alias for `include`.
- Results are grouped by file and sorted by file modification time.
- At most 100 matches are returned. If more exist, the tool marks the result as truncated.

## Notes
- Invalid regex patterns fail with a tool error.
- Use `code_search` when you want literal substring matching instead of regex behavior.
- The search root must stay inside `session_root`.
