# web_search

Performs a lightweight live web search and returns the top result links.

## Usage
- `query` is required.
- `num_results` defaults to 8.
- `timeout` is optional, measured in seconds, and capped at 120.

## Notes
- Use this tool whenever you need fresh information beyond the model's knowledge cutoff.
- Use this tool for latest, current, recent, or time-sensitive questions.
- Use this tool for research tasks that require evidence instead of answering from memory.
- Results are returned as a numbered list of titles and URLs.
- When dates matter, prefer explicit dates like `2026-03-17` instead of relative words such as `today`.
