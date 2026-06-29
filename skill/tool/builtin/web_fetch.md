# web_fetch

Fetches content from a URL and returns it as text, markdown, or raw HTML.

## Usage
- `url` is required and must start with `http://` or `https://`.
- `http://` URLs are upgraded to `https://` automatically.
- `format` defaults to `markdown` and also accepts `text` or `html`.
- `timeout` is optional, measured in seconds, and capped at `120`.

## Notes
- This tool is read-only.
- Requests use browser-like headers to improve compatibility with normal web pages.
- HTML responses are converted with higher-quality text and markdown extraction before being returned.
- Very large responses are still rejected above the 5MB limit.
- Some anti-bot or blocked pages may still fail and should be handled explicitly by the assistant.
