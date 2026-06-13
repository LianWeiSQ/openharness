# web_scrape

Advanced optional web scraping powered by Scrapling.

## When to use
- Use `web_scrape` when `web_fetch` is not enough: dynamic pages, CSS/XPath extraction, or pages that need Scrapling's parser.
- Prefer `web_fetch` for ordinary single-page reading.
- Prefer `web_search` before scraping when you do not already know the target URL.

## Requirements
- This tool is optional. Install Scrapling first with `pip install 'openagent-core[scraping]'` or `pip install scrapling`.
- `stealthy` mode is disabled by default. Set `OPENAGENT_WEB_SCRAPE_ENABLE_STEALTHY=true` only for approved targets.

## Parameters
- `url` (required): target URL.
- `mode`: `http`, `dynamic`, or `stealthy`. Defaults to `http`.
- `format`: `markdown`, `text`, or `html`. Defaults to `markdown`.
- `selector`: optional CSS or XPath selector. If omitted, the whole page is returned.
- `selector_type`: `css` or `xpath`. Defaults to `css`.
- `limit`: maximum matched elements to return. Capped at 100.
- `timeout`: timeout in seconds. Capped at 120.
- `adaptive`: enable Scrapling adaptive selector config.
- `network_idle`: browser modes only, wait for network idle.
- `headless`: browser modes only, run headless.

## Safety
- Do not use this tool to bypass access controls or scrape content against site terms.
- Use `stealthy` mode only when the user explicitly approves the target and the environment enables it.
- Avoid broad crawling. This tool is for focused URL or selector extraction.
