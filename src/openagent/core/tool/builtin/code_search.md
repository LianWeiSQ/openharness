# code_search

Search code files using literal substring matching.

## Parameters
- `query` (required, string): literal substring to find.
- `glob` (optional, string, default `*`): filename glob filter such as `*.py`.
- `path` (optional, string): search root, defaults to `session_root`.

## Output
- One hit per line: `/abs/path/to/file:line_number:content`

## Notes
- This tool intentionally uses substring matching, not regex.
- When the hit cap is reached, the tool marks the result as logically truncated.
- Use `grep` when you need regex semantics.
