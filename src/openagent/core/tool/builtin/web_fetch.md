# web_fetch

Fetches content from a URL and returns it as text, markdown, or raw HTML.

## Usage
- `url` is required and must start with `http://` or `https://`.
- `http://` URLs are upgraded to `https://` automatically.
- `format` defaults to `markdown` and also accepts `text` or `html`.
- `timeout` is optional, measured in seconds, and capped at 120.

## Notes
- This tool is read-only.
- HTML responses are converted to plain text or lightweight markdown when requested.
- Responses larger than 5MB are rejected.
