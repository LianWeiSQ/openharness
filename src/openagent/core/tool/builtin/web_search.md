# web_search

Performs a live web search through Exa MCP and returns LLM-friendly context text instead of a plain link list.

## Usage
- `query` is required.
- `num_results` defaults to `8`.
- `timeout` is optional, measured in seconds, and capped at `120`.
- `livecrawl` optionally chooses between `fallback` and `preferred`.
- `type` optionally chooses between `auto`, `fast`, and `deep`.
- `context_max_characters` can limit how much context Exa returns for the model.

## Notes
- Use this tool whenever you need fresh information beyond the model's knowledge cutoff.
- Use this tool for latest, current, recent, or time-sensitive questions.
- Use this tool for research tasks that require evidence instead of answering from memory.
- The result is optimized as model-readable context, so it may not be formatted as numbered URLs.
- When dates matter, prefer explicit dates like `2026-03-25` instead of relative words such as `today`.
