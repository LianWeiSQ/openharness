# write

Writes UTF-8 text to a file on the local filesystem.

## Usage
- `file_path` and `content` are required.
- The tool overwrites the target file if it already exists.
- Parent directories are created automatically when needed.
- Prefer editing existing files instead of creating new files unless the task really requires a new file.

## Safety
- The file must stay inside `session_root`.
- When the tool runs inside an active `Session`, overwriting an existing file requires that you read the file first in the same session.
- Avoid creating documentation files such as `README.md` unless the user explicitly asks for them.
