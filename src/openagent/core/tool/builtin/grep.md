# grep

Search file contents with a Python `re` regular expression.

## Parameters
- `pattern` (required, string): regular expression pattern.
- `path` (optional, string): search root, defaults to `session_root`.
- `glob` (optional, string, default `*`): filename glob filter such as `*.py`.

## Output
- One hit per line: `/abs/path/to/file:line_number:content`

## Notes
- This tool is regex-based on purpose.
- Invalid regex patterns fail with a tool error; they are not downgraded to substring search.
- Use `code_search` when you want fast literal substring matching.
