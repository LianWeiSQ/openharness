from __future__ import annotations

"""
Web tools (web_fetch/web_search).

中文说明：
- 使用 Python 标准库完成轻量网页抓取和检索
- 默认限制响应大小，避免一次性拉取超大页面
- 对 HTML 内容会按需要转换为 text / markdown / html 输出
"""

from dataclasses import dataclass, field
from html import unescape
from html.parser import HTMLParser
from typing import Literal
from urllib.parse import parse_qs, quote_plus, unquote, urlparse
from urllib.request import Request, urlopen
import re

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry

MAX_RESPONSE_SIZE = 5 * 1024 * 1024
DEFAULT_TIMEOUT = 30
MAX_TIMEOUT = 120
SEARCH_RESULT_RE = re.compile(
    r'<a[^>]+class="result__a"[^>]+href="(?P<href>[^"]+)"[^>]*>(?P<title>.*?)</a>',
    re.IGNORECASE | re.DOTALL,
)


@dataclass
class WebFetchParameters:
    url: str = field(metadata={"description": "要抓取的 URL"})
    format: Literal["text", "markdown", "html"] = field(
        default="markdown", metadata={"description": "返回格式：text、markdown 或 html"}
    )
    timeout: int | None = field(default=None, metadata={"description": "超时秒数，最大 120"})


@dataclass
class WebSearchParameters:
    query: str = field(metadata={"description": "搜索关键词"})
    num_results: int = field(default=8, metadata={"description": "最多返回多少条结果"})
    timeout: int | None = field(default=None, metadata={"description": "超时秒数，最大 120"})


class _HTMLTextExtractor(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self._parts: list[str] = []
        self._skip_stack = 0

    def handle_starttag(self, tag: str, attrs) -> None:  # type: ignore[override]
        if tag in {"script", "style", "noscript"}:
            self._skip_stack += 1
            return
        if tag in {"p", "div", "section", "article", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6"}:
            self._parts.append("\n")

    def handle_endtag(self, tag: str) -> None:  # type: ignore[override]
        if tag in {"script", "style", "noscript"} and self._skip_stack > 0:
            self._skip_stack -= 1
            return
        if tag in {"p", "div", "section", "article", "li"}:
            self._parts.append("\n")

    def handle_data(self, data: str) -> None:  # type: ignore[override]
        if self._skip_stack > 0:
            return
        text = data.strip()
        if text:
            self._parts.append(text)

    def get_text(self) -> str:
        joined = " ".join(self._parts)
        joined = re.sub(r"\n\s*\n+", "\n\n", joined)
        joined = re.sub(r"[ \t]+", " ", joined)
        return joined.strip()



def _normalize_url(url: str) -> str:
    normalized = url.strip()
    if normalized.startswith("http://"):
        normalized = "https://" + normalized[len("http://") :]
    if not normalized.startswith("https://"):
        raise ValueError("URL must start with http:// or https://")
    return normalized



def _timeout_seconds(value: int | None) -> int:
    if value is None:
        return DEFAULT_TIMEOUT
    return max(1, min(int(value), MAX_TIMEOUT))



def _fetch(url: str, *, timeout: int, accept: str) -> tuple[str, str]:
    request = Request(
        url,
        headers={
            "User-Agent": "OpenAgent/1.0 (+https://example.invalid)",
            "Accept": accept,
            "Accept-Language": "en-US,en;q=0.9",
        },
    )
    with urlopen(request, timeout=timeout) as response:
        content_length = response.headers.get("content-length")
        if content_length and int(content_length) > MAX_RESPONSE_SIZE:
            raise ValueError("Response too large (exceeds 5MB limit)")
        content_type = response.headers.get_content_type() or "text/plain"
        payload = response.read(MAX_RESPONSE_SIZE + 1)
        if len(payload) > MAX_RESPONSE_SIZE:
            raise ValueError("Response too large (exceeds 5MB limit)")
        charset = response.headers.get_content_charset() or "utf-8"
        text = payload.decode(charset, errors="replace")
        return text, content_type



def _html_to_text(html: str) -> str:
    parser = _HTMLTextExtractor()
    parser.feed(html)
    parser.close()
    return parser.get_text()



def _html_to_markdown(html: str) -> str:
    text = _html_to_text(html)
    paragraphs = [segment.strip() for segment in text.split("\n\n") if segment.strip()]
    return "\n\n".join(paragraphs)



def _clean_html_fragment(fragment: str) -> str:
    text = re.sub(r"<[^>]+>", " ", fragment)
    text = unescape(text)
    text = re.sub(r"\s+", " ", text)
    return text.strip()



def _search_results(html: str, *, limit: int) -> list[tuple[str, str]]:
    results: list[tuple[str, str]] = []
    for match in SEARCH_RESULT_RE.finditer(html):
        href = unescape(match.group("href"))
        title = _clean_html_fragment(match.group("title"))
        parsed = urlparse(href)
        if parsed.netloc.endswith("duckduckgo.com") and parsed.query:
            query = parse_qs(parsed.query)
            target = query.get("uddg")
            if target:
                href = unquote(target[0])
        if title and href:
            results.append((title, href))
        if len(results) >= limit:
            break
    return results


async def web_fetch_tool(args: WebFetchParameters, _ctx: ToolContext) -> ToolOutput:
    url = _normalize_url(args.url)
    timeout = _timeout_seconds(args.timeout)
    accept = {
        "markdown": "text/markdown, text/plain, text/html;q=0.8, */*;q=0.1",
        "text": "text/plain, text/html;q=0.8, */*;q=0.1",
        "html": "text/html, application/xhtml+xml;q=0.9, */*;q=0.1",
    }[args.format]
    content, content_type = _fetch(url, timeout=timeout, accept=accept)

    if args.format == "html":
        output = content
    elif content_type == "text/html":
        output = _html_to_markdown(content) if args.format == "markdown" else _html_to_text(content)
    else:
        output = content

    return ToolOutput(
        title=f"{url} ({content_type})",
        output=output,
        metadata={"url": url, "format": args.format, "content_type": content_type},
    )


async def web_search_tool(args: WebSearchParameters, _ctx: ToolContext) -> ToolOutput:
    timeout = _timeout_seconds(args.timeout)
    search_url = f"https://html.duckduckgo.com/html/?q={quote_plus(args.query)}"
    html, _content_type = _fetch(search_url, timeout=timeout, accept="text/html, */*;q=0.1")
    results = _search_results(html, limit=max(1, args.num_results))

    if not results:
        output = "No search results found. Please try a different query."
    else:
        lines: list[str] = []
        for index, (title, href) in enumerate(results, start=1):
            lines.append(f"{index}. {title}")
            lines.append(f"   {href}")
        output = "\n".join(lines)

    return ToolOutput(
        title=f"Web search: {args.query}",
        output=output,
        metadata={"query": args.query, "num_results": max(1, args.num_results)},
    )



def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="web_fetch", parameters=WebFetchParameters, description_md="web_fetch.md", group="web", dangerous=True)(web_fetch_tool)
    registry.define_tool(tool_id="web_search", parameters=WebSearchParameters, description_md="web_search.md", group="web", dangerous=True)(web_search_tool)


__all__ = ["register"]
