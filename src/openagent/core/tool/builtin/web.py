from __future__ import annotations

"""
Web tools (web_fetch/web_search).

Notes:
- `web_search` aligns with opencode's Exa MCP JSON-RPC + SSE flow.
- `web_fetch` keeps OpenAgent's existing interface, but uses more browser-like
  request headers and higher-quality HTML conversion.
- `metadata["preview"]` remains as OpenAgent's compatibility bridge for
  projecting tool output back into flat chat messages.
"""

from dataclasses import dataclass, field
from html.parser import HTMLParser
import json
import re
import socket
from typing import Any, Literal
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

try:
    from bs4 import BeautifulSoup
    from bs4 import Comment as BeautifulSoupComment
except ImportError:  # pragma: no cover - dependency is declared in pyproject
    BeautifulSoup = None
    BeautifulSoupComment = None

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry

MAX_RESPONSE_SIZE = 5 * 1024 * 1024
DEFAULT_TIMEOUT = 30
MAX_TIMEOUT = 120
EXA_MCP_URL = "https://mcp.exa.ai/mcp"
EXA_DEFAULT_NUM_RESULTS = 8
BROWSER_USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
)
BLOCK_SPLIT_RE = re.compile(r"\n\s*\n+")
WORD_RE = re.compile(r"[A-Za-z0-9_]+")
PUNCTUATION_RE = re.compile(r"[.!?;:,\u3002\uff01\uff1f\uff1b\uff1a\uff0c]")
HTML_NOISE_TAGS = ("script", "style", "noscript", "iframe", "object", "embed", "meta", "link")
RAW_NOISE_BLOCK_RE = re.compile(
    r"<(?:script|style|noscript|iframe|object|embed)\b[^>]*>.*?</(?:script|style|noscript|iframe|object|embed)>",
    re.IGNORECASE | re.DOTALL,
)
RAW_NOISE_VOID_RE = re.compile(r"<(?:meta|link)\b[^>]*>", re.IGNORECASE)


@dataclass
class WebFetchParameters:
    url: str = field(metadata={"description": "要抓取的 URL"})
    format: Literal["text", "markdown", "html"] = field(
        default="markdown",
        metadata={"description": "返回格式：text、markdown 或 html"},
    )
    timeout: int | None = field(default=None, metadata={"description": "超时秒数，最大 120"})


@dataclass
class WebSearchParameters:
    query: str = field(metadata={"description": "搜索关键词"})
    num_results: int = field(default=8, metadata={"description": "最多返回多少条结果"})
    timeout: int | None = field(default=None, metadata={"description": "超时秒数，最大 120"})
    livecrawl: Literal["fallback", "preferred"] | None = field(
        default=None,
        metadata={"description": "实时抓取策略：fallback 或 preferred"},
    )
    type: Literal["auto", "fast", "deep"] | None = field(
        default=None,
        metadata={"description": "搜索类型：auto、fast 或 deep"},
    )
    context_max_characters: int | None = field(
        default=None,
        metadata={"description": "限制返回给 LLM 的上下文字符数"},
    )


class WebRequestError(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        body: str | None = None,
        timeout: bool = False,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.body = body
        self.timeout = timeout


class _HTMLTextExtractor(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self._parts: list[str] = []
        self._skip_stack = 0

    def handle_starttag(self, tag: str, attrs) -> None:  # type: ignore[override]
        if tag in {"script", "style", "noscript", "iframe", "object", "embed"}:
            self._skip_stack += 1
            return
        if tag in {"p", "div", "section", "article", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6"}:
            self._parts.append("\n")

    def handle_endtag(self, tag: str) -> None:  # type: ignore[override]
        if tag in {"script", "style", "noscript", "iframe", "object", "embed"} and self._skip_stack > 0:
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


def _request_headers(*, accept: str, content_type: str | None = None) -> dict[str, str]:
    headers = {
        "User-Agent": BROWSER_USER_AGENT,
        "Accept": accept,
        "Accept-Language": "en-US,en;q=0.9",
    }
    if content_type:
        headers["Content-Type"] = content_type
    return headers


def _decode_http_error_body(error: HTTPError) -> str:
    try:
        payload = error.read()
    except Exception:  # noqa: BLE001
        return ""
    charset = "utf-8"
    try:
        charset = error.headers.get_content_charset() or "utf-8"
    except Exception:  # noqa: BLE001
        charset = "utf-8"
    return payload.decode(charset, errors="replace").strip()


def _is_timeout_error(error: object) -> bool:
    return isinstance(error, (TimeoutError, socket.timeout))


def _request_text(request: Request, *, timeout: int) -> tuple[str, str]:
    try:
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
    except HTTPError as error:
        raise WebRequestError(
            f"Request failed with status code: {error.code}",
            status_code=error.code,
            body=_decode_http_error_body(error),
        ) from error
    except URLError as error:
        if _is_timeout_error(error.reason):
            raise WebRequestError("Request timed out", timeout=True) from error
        reason = str(error.reason or error).strip() or "unknown network error"
        raise WebRequestError(f"Request failed: {reason}") from error
    except (TimeoutError, socket.timeout) as error:
        raise WebRequestError("Request timed out", timeout=True) from error


def _fetch(url: str, *, timeout: int, accept: str) -> tuple[str, str]:
    request = Request(url, headers=_request_headers(accept=accept))
    return _request_text(request, timeout=timeout)


def _post_json(url: str, *, payload: dict[str, Any], timeout: int, accept: str) -> tuple[str, str]:
    body = json.dumps(payload).encode("utf-8")
    request = Request(
        url,
        data=body,
        method="POST",
        headers=_request_headers(accept=accept, content_type="application/json"),
    )
    return _request_text(request, timeout=timeout)


def _strip_raw_noise_html(html: str) -> str:
    stripped = RAW_NOISE_BLOCK_RE.sub(" ", html)
    return RAW_NOISE_VOID_RE.sub(" ", stripped)


def _prepare_soup(html: str):
    if BeautifulSoup is None:
        return None

    soup = BeautifulSoup(_strip_raw_noise_html(html), "html.parser")
    for tag_name in HTML_NOISE_TAGS:
        for node in soup.find_all(tag_name):
            node.decompose()
    if BeautifulSoupComment is not None:
        for node in soup.find_all(string=lambda value: isinstance(value, BeautifulSoupComment)):
            node.extract()
    return soup


def _html_to_text(html: str) -> str:
    soup = _prepare_soup(html)
    if soup is None:
        parser = _HTMLTextExtractor()
        parser.feed(_strip_raw_noise_html(html))
        parser.close()
        return parser.get_text()

    text = soup.get_text("\n")
    lines: list[str] = []
    previous = ""
    for raw in text.splitlines():
        normalized = re.sub(r"\s+", " ", raw).strip()
        if not normalized or normalized == previous:
            continue
        lines.append(normalized)
        previous = normalized
    return "\n".join(lines)


def _dedupe_preserving_order(values: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        normalized = value.strip()
        if not normalized or normalized in seen:
            continue
        seen.add(normalized)
        result.append(normalized)
    return result


def _html_to_markdown_fallback(html: str) -> str:
    soup = _prepare_soup(html)
    if soup is None:
        return _html_to_text(html)

    blocks: list[str] = []
    for element in soup.find_all(["h1", "h2", "h3", "h4", "h5", "h6", "p", "li", "pre", "code"]):
        tag_name = getattr(element, "name", "") or ""
        if tag_name == "code" and getattr(getattr(element, "parent", None), "name", None) == "pre":
            continue

        if tag_name == "pre":
            text = element.get_text("\n", strip=True)
            if text:
                blocks.append(f"```\n{text}\n```")
            continue

        text = re.sub(r"\s+", " ", element.get_text(" ", strip=True)).strip()
        if not text:
            continue
        if tag_name.startswith("h") and len(tag_name) == 2 and tag_name[1].isdigit():
            level = max(1, min(int(tag_name[1]), 6))
            blocks.append(f"{'#' * level} {text}")
        elif tag_name == "li":
            blocks.append(f"- {text}")
        else:
            blocks.append(text)

    deduped = _dedupe_preserving_order(blocks)
    if not deduped:
        return _html_to_text(html)
    return "\n\n".join(deduped)


def _html_to_markdown(html: str) -> str:
    soup = _prepare_soup(html)
    cleaned_html = str(soup) if soup is not None else html

    try:
        from markdownify import markdownify as convert_html
    except ImportError:  # pragma: no cover - fallback is exercised only without dependency
        return _html_to_markdown_fallback(cleaned_html)

    markdown = convert_html(
        cleaned_html,
        heading_style="ATX",
        bullets="-",
    )
    markdown = re.sub(r"\n{3,}", "\n\n", markdown)
    return markdown.strip()


def _normalized_lines(text: str) -> list[str]:
    lines: list[str] = []
    for raw in text.splitlines():
        normalized = re.sub(r"\s+", " ", raw).strip()
        if normalized:
            lines.append(normalized)
    return lines


def _normalize_block(text: str) -> str:
    lines: list[str] = []
    previous = ""
    for line in _normalized_lines(text):
        if line == previous:
            continue
        lines.append(line)
        previous = line
    return "\n".join(lines)


def _blocks_from_text(text: str) -> list[str]:
    blocks: list[str] = []
    for chunk in BLOCK_SPLIT_RE.split(text):
        normalized = _normalize_block(chunk)
        if normalized:
            blocks.append(normalized)
    if len(blocks) >= 2:
        return blocks

    lines = _normalized_lines(text)
    if not lines:
        return []

    chunk_size = 3 if len(lines) > 6 else 2
    return ["\n".join(lines[index : index + chunk_size]) for index in range(0, len(lines), chunk_size)]


def _compact_block(block: str) -> str:
    return re.sub(r"\s+", " ", block).strip()


def _looks_like_navigation(compact: str) -> bool:
    if compact.count("|") >= 2:
        return True
    if compact.count(" / ") >= 2:
        return True
    if compact.count(" ? ") >= 2:
        return True
    if compact.count(" > ") >= 1 and len(compact) < 160:
        return True

    words = WORD_RE.findall(compact)
    if len(words) < 4:
        return False
    short_ratio = sum(len(word) <= 3 for word in words) / len(words)
    return short_ratio >= 0.75 and len(compact) < 96 and not PUNCTUATION_RE.search(compact)


def _is_low_signal_block(block: str, *, occurrences: dict[str, int]) -> bool:
    compact = _compact_block(block)
    if not compact:
        return True
    if occurrences.get(compact, 0) > 1 and len(compact) < 80:
        return True
    if _looks_like_navigation(compact):
        return True
    if len(compact) < 12 and not re.search(r"\d", compact):
        return True

    words = WORD_RE.findall(compact)
    if len(words) <= 2 and len(compact) < 24 and not re.search(r"\d", compact):
        return True
    return False


def _block_signal_score(block: str, *, index: int, total_blocks: int) -> int:
    compact = _compact_block(block)
    words = WORD_RE.findall(compact)
    score = 0
    length = len(compact)

    if 24 <= length <= 280:
        score += 4
    elif 12 <= length <= 420:
        score += 2
    elif length > 420:
        score += 1

    if "\n" in block:
        score += 2
    if re.search(r"\d", compact):
        score += 1

    punctuation_hits = len(PUNCTUATION_RE.findall(compact))
    if punctuation_hits >= 1:
        score += 2
    if punctuation_hits >= 3:
        score += 1
    if len(words) >= 8:
        score += 2
    if len(words) >= 16:
        score += 1
    if len({word.lower() for word in words}) >= max(4, len(words) // 2):
        score += 1
    if index == 0 or index == total_blocks - 1:
        score += 1
    return score


def _block_preview_from_text(text: str) -> str:
    blocks = _blocks_from_text(text)
    if not blocks:
        return ""

    occurrences: dict[str, int] = {}
    for block in blocks:
        compact = _compact_block(block)
        occurrences[compact] = occurrences.get(compact, 0) + 1

    meaningful = [index for index, block in enumerate(blocks) if not _is_low_signal_block(block, occurrences=occurrences)]
    if not meaningful:
        fallback_lines = _normalized_lines(text)
        return "\n".join(fallback_lines[:8])

    selected: set[int] = {meaningful[0]}
    if len(meaningful) > 1:
        selected.add(meaningful[-1])

    middle_candidates = sorted(
        meaningful[1:-1],
        key=lambda index: (-_block_signal_score(blocks[index], index=index, total_blocks=len(blocks)), index),
    )
    target_count = min(5, len(meaningful))
    for index in middle_candidates:
        selected.add(index)
        if len(selected) >= target_count:
            break

    if len(selected) < min(3, len(meaningful)):
        for index in meaningful:
            selected.add(index)
            if len(selected) >= min(3, len(meaningful)):
                break

    selected_blocks: list[str] = []
    seen: set[str] = set()
    for index, block in enumerate(blocks):
        if index not in selected:
            continue
        compact = _compact_block(block)
        if compact in seen:
            continue
        seen.add(compact)
        selected_blocks.append(block)
        if len(selected_blocks) >= 5:
            break

    if not selected_blocks:
        selected_blocks = blocks[: min(len(blocks), 3)]
    return "\n\n".join(selected_blocks)


def _build_exa_search_request(args: WebSearchParameters) -> dict[str, Any]:
    search_type = args.type or "auto"
    livecrawl = args.livecrawl or "fallback"
    num_results = max(1, int(args.num_results or EXA_DEFAULT_NUM_RESULTS))

    arguments: dict[str, Any] = {
        "query": args.query,
        "type": search_type,
        "numResults": num_results,
        "livecrawl": livecrawl,
    }
    if args.context_max_characters is not None:
        arguments["contextMaxCharacters"] = int(args.context_max_characters)

    return {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "web_search_exa",
            "arguments": arguments,
        },
    }


def _extract_exa_result_text(payload: dict[str, Any]) -> str | None:
    result = payload.get("result")
    if not isinstance(result, dict):
        return None
    content = result.get("content")
    if not isinstance(content, list):
        return None
    for item in content:
        if not isinstance(item, dict):
            continue
        text = item.get("text")
        if isinstance(text, str) and text.strip():
            return text.strip()
    return None


def _parse_exa_search_response(response_text: str) -> str | None:
    stripped = response_text.strip()
    if not stripped:
        return None

    if stripped.startswith("{"):
        try:
            payload = json.loads(stripped)
        except json.JSONDecodeError:
            return None
        return _extract_exa_result_text(payload)

    for raw_line in stripped.splitlines():
        line = raw_line.strip()
        if not line.startswith("data:"):
            continue
        data = line[5:].strip()
        if not data or data == "[DONE]":
            continue
        try:
            payload = json.loads(data)
        except json.JSONDecodeError:
            continue
        text = _extract_exa_result_text(payload)
        if text:
            return text
    return None


async def web_fetch_tool(args: WebFetchParameters, _ctx: ToolContext) -> ToolOutput:
    url = _normalize_url(args.url)
    timeout = _timeout_seconds(args.timeout)
    accept = {
        "markdown": "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1",
        "text": "text/plain;q=1.0, text/markdown;q=0.9, text/html;q=0.8, */*;q=0.1",
        "html": "text/html;q=1.0, application/xhtml+xml;q=0.9, text/plain;q=0.8, text/markdown;q=0.7, */*;q=0.1",
    }[args.format]

    try:
        content, content_type = _fetch(url, timeout=timeout, accept=accept)
    except WebRequestError as error:
        if error.timeout:
            raise RuntimeError("Request timed out") from error
        if error.status_code is not None:
            raise RuntimeError(f"Request failed with status code: {error.status_code}") from error
        raise RuntimeError(str(error)) from error

    if args.format == "html":
        output = content
    elif content_type == "text/html":
        output = _html_to_markdown(content) if args.format == "markdown" else _html_to_text(content)
    else:
        output = content

    preview_source = output
    if content_type == "text/html":
        preview_source = _html_to_text(content)
    preview = _block_preview_from_text(preview_source)
    metadata = {
        "url": url,
        "format": args.format,
        "content_type": content_type,
    }
    if preview:
        metadata["preview"] = preview
        metadata["preview_strategy"] = "block_extract"

    return ToolOutput(
        title=f"{url} ({content_type})",
        output=output,
        metadata=metadata,
    )


async def web_search_tool(args: WebSearchParameters, _ctx: ToolContext) -> ToolOutput:
    timeout = _timeout_seconds(args.timeout)
    request_payload = _build_exa_search_request(args)

    try:
        response_text, _content_type = _post_json(
            EXA_MCP_URL,
            payload=request_payload,
            timeout=timeout,
            accept="application/json, text/event-stream",
        )
    except WebRequestError as error:
        if error.timeout:
            raise RuntimeError("Search request timed out") from error
        if error.status_code is not None:
            detail = f": {error.body}" if error.body else ""
            raise RuntimeError(f"Search error ({error.status_code}){detail}") from error
        raise RuntimeError(f"Search request failed: {error}") from error

    output = _parse_exa_search_response(response_text)
    if not output:
        output = "No search results found. Please try a different query."

    metadata: dict[str, Any] = {
        "backend": "exa_mcp",
        "query": args.query,
        "num_results": request_payload["params"]["arguments"]["numResults"],
        "timeout": timeout,
        "livecrawl": request_payload["params"]["arguments"]["livecrawl"],
        "type": request_payload["params"]["arguments"]["type"],
        "context_max_characters": request_payload["params"]["arguments"].get("contextMaxCharacters"),
    }
    preview = _block_preview_from_text(output)
    if preview:
        metadata["preview"] = preview
        metadata["preview_strategy"] = "block_extract"

    return ToolOutput(
        title=f"Web search: {args.query}",
        output=output,
        metadata=metadata,
    )


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="web_fetch", parameters=WebFetchParameters, description_md="web_fetch.md", group="web", dangerous=True)(web_fetch_tool)
    registry.define_tool(tool_id="web_search", parameters=WebSearchParameters, description_md="web_search.md", group="web", dangerous=True)(web_search_tool)


__all__ = ["register"]
