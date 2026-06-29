# read

Reads a UTF-8 text file from the local filesystem.

## Usage
- `file_path` is required and may be relative to `session_root` or absolute inside `session_root`.
- By default the tool reads up to 2000 lines starting at line 0.
- You can optionally provide `offset` and `limit`, but when you want the whole file it is usually better to omit them.
- Lines longer than 2000 characters are truncated.
- Output is returned in `cat -n` style using `00001| text` line prefixes.
- If the file still has more content, the tool tells you which line to continue from.
- If the file does not exist, the tool may suggest close matches in the same directory.

## Notes
- Binary and common image/archive formats are rejected with a tool error.
- The file must stay inside `session_root`.
- It is often better to read multiple potentially relevant files in parallel.
