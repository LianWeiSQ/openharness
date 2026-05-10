from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import uuid4

from openagent.core.tool.builtin import web as web_tools
from openagent.core.tool.toolkit import ToolkitAdapter


class SearchToolTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root

    def _make_toolkit(self) -> ToolkitAdapter:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()
        return toolkit

    async def test_code_search_hits_and_miss(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("alpha\nbeta\n", encoding="utf-8")
            (root / "b.txt").write_text("gamma\n", encoding="utf-8")

            toolkit = self._make_toolkit()
            ctx = {"session_root": str(root)}

            res = await toolkit.execute(
                name="code_search",
                input={"query": "beta", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertIn("a.py:2:beta", res.output)
            self.assertFalse(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])
            self.assertIn("a.py:2:beta", res.metadata["preview"])
            self.assertEqual(res.metadata["returned_count"], 1)

            res = await toolkit.execute(
                name="code_search",
                input={"query": "nope", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertEqual(res.output, "")
            self.assertFalse(res.metadata["truncated"])
            self.assertEqual(res.metadata["returned_count"], 0)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_code_search_marks_semantic_truncation_at_hit_limit(self) -> None:
        root = self._make_temp_root()
        try:
            lines = [f"beta {i}" for i in range(210)]
            (root / "a.py").write_text("\n".join(lines), encoding="utf-8")

            toolkit = self._make_toolkit()
            res = await toolkit.execute(
                name="code_search",
                input={"query": "beta", "glob": "*.py"},
                context={"session_root": str(root)},
            )

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])
            self.assertEqual(res.metadata["count"], 200)
            self.assertEqual(res.metadata["returned_count"], 200)
            self.assertEqual(len([line for line in res.output.splitlines() if line.strip()]), 200)
            self.assertLessEqual(len([line for line in res.metadata["preview"].splitlines() if line.strip()]), 20)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_code_search_clips_long_lines_in_output_and_preview(self) -> None:
        root = self._make_temp_root()
        try:
            long_line = "beta " + ("x" * 500)
            (root / "a.py").write_text(long_line + "\n", encoding="utf-8")

            toolkit = self._make_toolkit()
            res = await toolkit.execute(
                name="code_search",
                input={"query": "beta", "glob": "*.py"},
                context={"session_root": str(root)},
            )

            self.assertIsNone(res.error)
            last_segment = res.output.rsplit(":", 1)[-1]
            self.assertLessEqual(len(last_segment), 243)
            self.assertTrue(last_segment.endswith("..."))
            self.assertIn(res.metadata["preview"], res.output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_grep_uses_regex_while_code_search_uses_substring(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("abc\n", encoding="utf-8")

            toolkit = self._make_toolkit()
            ctx = {"session_root": str(root)}

            grep_res = await toolkit.execute(
                name="grep",
                input={"pattern": "a.c", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(grep_res.error)
            self.assertIn("abc", grep_res.output)

            code_search_res = await toolkit.execute(
                name="code_search",
                input={"query": "a.c", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(code_search_res.error)
            self.assertEqual(code_search_res.output, "")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_grep_invalid_regex_returns_tool_error(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("abc\n", encoding="utf-8")

            toolkit = self._make_toolkit()
            res = await toolkit.execute(
                name="grep",
                input={"pattern": "[", "glob": "*.py"},
                context={"session_root": str(root)},
            )

            self.assertIsNotNone(res.error)
            self.assertEqual(res.output, "")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_uses_exa_defaults_and_parses_sse_response(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            sse_text = "\n".join(
                [
                    "event: message",
                    "data: " + json.dumps({"jsonrpc": "2.0", "result": {"content": [{"type": "text", "text": "Fresh search context for the latest model releases."}]}}),
                    "",
                ]
            )

            with patch.object(web_tools, "_post_json", return_value=(sse_text, "text/event-stream")) as mocked_post:
                res = await toolkit.execute(
                    name="web_search",
                    input={"query": "latest ai model releases"},
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertEqual(res.output, "Fresh search context for the latest model releases.")
            self.assertEqual(res.metadata["backend"], "exa_mcp")
            self.assertEqual(res.metadata["num_results"], 8)
            self.assertEqual(res.metadata["type"], "auto")
            self.assertEqual(res.metadata["livecrawl"], "fallback")
            self.assertEqual(res.metadata["preview_strategy"], "search_summary")
            self.assertEqual(res.metadata["returned_count"], 1)
            self.assertEqual(res.metadata["count"], 1)
            self.assertIn("1. Fresh search context", res.metadata["preview"])

            self.assertEqual(mocked_post.call_args.kwargs["accept"], "application/json, text/event-stream")
            self.assertEqual(mocked_post.call_args.kwargs["timeout"], 30)
            payload = mocked_post.call_args.kwargs["payload"]
            self.assertEqual(payload["params"]["name"], "web_search_exa")
            self.assertEqual(
                payload["params"]["arguments"],
                {
                    "query": "latest ai model releases",
                    "type": "auto",
                    "numResults": 8,
                    "livecrawl": "fallback",
                    "contextMaxCharacters": 10000,
                },
            )
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_maps_custom_exa_parameters(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            response_text = json.dumps(
                {
                    "jsonrpc": "2.0",
                    "result": {"content": [{"type": "text", "text": "Deep search synthesis with extended context."}]},
                }
            )

            with patch.object(web_tools, "_post_json", return_value=(response_text, "application/json")) as mocked_post:
                res = await toolkit.execute(
                    name="web_search",
                    input={
                        "query": "distributed tracing migration plan",
                        "num_results": 3,
                        "timeout": 12,
                        "livecrawl": "preferred",
                        "type": "deep",
                        "context_max_characters": 16000,
                    },
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertEqual(res.metadata["num_results"], 3)
            self.assertEqual(res.metadata["type"], "deep")
            self.assertEqual(res.metadata["livecrawl"], "preferred")
            self.assertEqual(res.metadata["context_max_characters"], 16000)
            self.assertEqual(mocked_post.call_args.kwargs["timeout"], 12)
            self.assertEqual(
                mocked_post.call_args.kwargs["payload"]["params"]["arguments"],
                {
                    "query": "distributed tracing migration plan",
                    "type": "deep",
                    "numResults": 3,
                    "livecrawl": "preferred",
                    "contextMaxCharacters": 16000,
                },
            )
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_returns_no_results_message_when_response_has_no_text(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            with patch.object(
                web_tools,
                "_post_json",
                return_value=('data: {"jsonrpc":"2.0","result":{"content":[]}}\n', "text/event-stream"),
            ):
                res = await toolkit.execute(
                    name="web_search",
                    input={"query": "query with no hits"},
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertEqual(res.output, "No search results found. Please try a different query.")
            self.assertEqual(res.metadata["returned_count"], 0)
            self.assertEqual(res.metadata["count"], 0)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_surfaces_http_errors(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            with patch.object(
                web_tools,
                "_post_json",
                side_effect=web_tools.WebRequestError(
                    "Request failed with status code: 502",
                    status_code=502,
                    body="upstream unavailable",
                ),
            ):
                res = await toolkit.execute(
                    name="web_search",
                    input={"query": "latest outage report"},
                    context={"session_root": str(root)},
                )

            self.assertIsNotNone(res.error)
            self.assertIn("Search error (502): upstream unavailable", res.error)
            self.assertEqual(res.metadata["error_kind"], "web_search_error")
            self.assertEqual(res.metadata["status_code"], 502)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_classifies_quota_errors_without_large_body(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            long_body = "free credits exceeded " + ("x" * 2000)
            with patch.object(
                web_tools,
                "_post_json",
                side_effect=web_tools.WebRequestError(
                    "Request failed with status code: 429",
                    status_code=429,
                    body=long_body,
                ),
            ):
                res = await toolkit.execute(
                    name="web_search",
                    input={"query": "latest outage report"},
                    context={"session_root": str(root)},
                )

            self.assertIsNotNone(res.error)
            self.assertEqual(res.metadata["error_kind"], "web_search_quota")
            self.assertEqual(res.metadata["status_code"], 429)
            self.assertLess(len(res.error or ""), 1000)
            self.assertNotIn("x" * 1000, res.error or "")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_passes_exa_api_key_header_from_env(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            response_text = json.dumps(
                {
                    "jsonrpc": "2.0",
                    "result": {"content": [{"type": "text", "text": "Search context."}]},
                }
            )
            with patch.dict("os.environ", {"OPENAGENT_WEB_SEARCH_EXA_API_KEY": "test-exa-key"}, clear=False):
                with patch.object(web_tools, "_post_json", return_value=(response_text, "application/json")) as mocked_post:
                    res = await toolkit.execute(
                        name="web_search",
                        input={"query": "configured search"},
                        context={"session_root": str(root)},
                    )

            self.assertIsNone(res.error)
            self.assertEqual(mocked_post.call_args.kwargs["extra_headers"], {"x-api-key": "test-exa-key"})
            self.assertNotIn("test-exa-key", str(res.metadata))
            self.assertNotIn("test-exa-key", res.output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_search_surfaces_timeout_errors(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = self._make_toolkit()
            with patch.object(
                web_tools,
                "_post_json",
                side_effect=web_tools.WebRequestError("Request timed out", timeout=True),
            ):
                res = await toolkit.execute(
                    name="web_search",
                    input={"query": "breaking news"},
                    context={"session_root": str(root)},
                )

            self.assertIsNotNone(res.error)
            self.assertIn("Search request timed out", res.error)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_fetch_text_strips_noise_tags(self) -> None:
        root = self._make_temp_root()
        try:
            html = """
<html>
  <head>
    <style>.hidden { display:none; }</style>
    <script>console.log('noise')</script>
  </head>
  <body>
    <h1>Incident Update</h1>
    <p>The primary region has recovered and traffic is stable.</p>
    <noscript>noscript noise</noscript>
    <iframe>iframe noise</iframe>
    <object>object noise</object>
    <embed>embed noise</embed>
  </body>
</html>
"""
            toolkit = self._make_toolkit()
            with patch.object(web_tools, "_fetch", return_value=(html, "text/html")):
                res = await toolkit.execute(
                    name="web_fetch",
                    input={"url": "https://example.com/status", "format": "text"},
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertIn("Incident Update", res.output)
            self.assertIn("traffic is stable", res.output)
            self.assertNotIn("console.log", res.output)
            self.assertNotIn("noscript noise", res.output)
            self.assertNotIn("iframe noise", res.output)
            self.assertNotIn("object noise", res.output)
            self.assertNotIn("embed noise", res.output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_fetch_markdown_keeps_basic_structure(self) -> None:
        root = self._make_temp_root()
        try:
            html = """
<html>
  <body>
    <h1>Quarterly Update</h1>
    <p>Service migration stays on schedule.</p>
    <ul>
      <li>Stage one complete</li>
      <li>Stage two ready</li>
    </ul>
    <script>console.log('noise')</script>
  </body>
</html>
"""
            toolkit = self._make_toolkit()
            with patch.object(web_tools, "_fetch", return_value=(html, "text/html")):
                res = await toolkit.execute(
                    name="web_fetch",
                    input={"url": "https://example.com/update", "format": "markdown"},
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertIn("Quarterly Update", res.output)
            self.assertIn("Service migration stays on schedule.", res.output)
            self.assertIn("Stage one complete", res.output)
            self.assertNotIn("console.log", res.output)
            self.assertEqual(res.metadata["preview_strategy"], "block_extract")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_web_fetch_uses_block_preview_instead_of_top_lines_only(self) -> None:
        root = self._make_temp_root()
        try:
            noise = "\n".join("<div>Home</div>" if index % 2 == 0 else "<div>Docs</div>" for index in range(44))
            html = f"""<html><body>{noise}
<div>Implementation update: the service now materializes provider payloads through a shared adapter, which reduced duplicated serialization logic.</div>
<div>Release note: rollout pauses automatically if post-deploy validation fails in the final region.</div>
</body></html>"""

            toolkit = self._make_toolkit()
            with patch.object(web_tools, "_fetch", return_value=(html, "text/html")):
                res = await toolkit.execute(
                    name="web_fetch",
                    input={"url": "https://example.com/report", "format": "text"},
                    context={"session_root": str(root)},
                )

            self.assertIsNone(res.error)
            self.assertEqual(res.metadata["preview_strategy"], "block_extract")
            self.assertIn("Implementation update", res.metadata["preview"])
            self.assertIn("Release note", res.metadata["preview"])
            self.assertNotIn("Home\n\nDocs", res.metadata["preview"])
        finally:
            shutil.rmtree(root, ignore_errors=True)
