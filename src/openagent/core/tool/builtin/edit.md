# edit

Performs exact string replacement in a file.

## Usage
- `file_path`, `old_string`, and `new_string` are required.
- `replace_all` is optional. Set it to `true` when every occurrence should be replaced.
- Read the file first so you can provide enough surrounding context for a unique match.
- Preserve indentation exactly as it appears in the file.

## Behavior
- The edit fails if `old_string` is not found.
- The edit fails if `old_string` appears multiple times and `replace_all` is not enabled.
- The edit fails if `old_string` and `new_string` are identical.
- Setting `old_string` to an empty string writes `new_string` directly to the target path.

## Safety
- The file must stay inside `session_root`.
- When the tool runs inside an active `Session`, editing an existing file requires that you read the file first in the same session.
