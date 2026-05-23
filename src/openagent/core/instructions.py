from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from .context_pack import ContextItem

DEFAULT_MAX_FILE_BYTES = 16 * 1024
DEFAULT_MAX_TOTAL_BYTES = 48 * 1024
DEFAULT_WORKSPACE_FILES = ("OPENAGENT.md", "AGENTS.md", "CLAUDE.md")
DEFAULT_USER_FILES = ("OPENAGENT.md", "instructions.md")

InstructionScope = Literal["workspace", "user"]


@dataclass(frozen=True, slots=True)
class InstructionLoadOptions:
    max_file_bytes: int = DEFAULT_MAX_FILE_BYTES
    max_total_bytes: int = DEFAULT_MAX_TOTAL_BYTES
    include_user: bool = True
    user_config_dir: Path | None = None
    workspace_files: tuple[str, ...] = DEFAULT_WORKSPACE_FILES
    user_files: tuple[str, ...] = DEFAULT_USER_FILES


@dataclass(frozen=True, slots=True)
class InstructionItem:
    path: Path
    display_path: str
    source: str
    scope: InstructionScope
    content: str
    bytes_read: int
    truncated: bool = False

    def to_context_item(self) -> ContextItem:
        digest = hashlib.sha1(str(self.path.resolve()).encode("utf-8")).hexdigest()[:12]
        return ContextItem(
            id=f"instruction:{self.scope}:{digest}",
            kind="instruction",
            source=self.source,
            content=f"[Instruction: {self.display_path}]\n{self.content}".strip(),
            priority=100,
            pinned=True,
            stable_prefix=True,
            metadata={
                "path": str(self.path),
                "display_path": self.display_path,
                "scope": self.scope,
                "bytes_read": self.bytes_read,
                "truncated": self.truncated,
            },
        )


@dataclass(frozen=True, slots=True)
class InstructionContext:
    items: list[InstructionItem]
    total_bytes: int
    truncated: bool
    issues: list[str]

    def to_context_items(self) -> list[ContextItem]:
        return [item.to_context_item() for item in self.items]


@dataclass(frozen=True, slots=True)
class _InstructionCandidate:
    path: Path
    display_path: str
    source: str
    scope: InstructionScope


class InstructionContextLoader:
    def __init__(self, workspace_root: str | Path, options: InstructionLoadOptions | None = None) -> None:
        self.workspace_root = Path(workspace_root).expanduser().resolve()
        self.options = options or InstructionLoadOptions()

    def load(self) -> InstructionContext:
        issues: list[str] = []
        items: list[InstructionItem] = []
        total_bytes = 0
        truncated = False
        seen: set[Path] = set()

        for candidate in self._candidates():
            path = candidate.path.expanduser().resolve()
            if path in seen or not path.is_file():
                continue
            seen.add(path)
            if not self._is_allowed_path(path):
                issues.append(f"skipped_out_of_scope:{candidate.display_path}")
                continue
            if total_bytes >= self.options.max_total_bytes:
                truncated = True
                issues.append("total_limit_reached")
                break

            loaded = self._load_candidate(candidate, remaining_bytes=self.options.max_total_bytes - total_bytes)
            if loaded is None:
                issues.append(f"skipped_unreadable:{candidate.display_path}")
                continue
            item, item_issue = loaded
            if item_issue:
                issues.append(item_issue)
            items.append(item)
            total_bytes += item.bytes_read
            truncated = truncated or item.truncated

        return InstructionContext(items=items, total_bytes=total_bytes, truncated=truncated, issues=issues)

    def _candidates(self) -> list[_InstructionCandidate]:
        candidates: list[_InstructionCandidate] = []
        for base in self._workspace_ancestors():
            for filename in self.options.workspace_files:
                path = base / filename
                candidates.append(
                    _InstructionCandidate(
                        path=path,
                        display_path=self._display_workspace_path(path),
                        source=f"instructions.workspace:{self._display_workspace_path(path)}",
                        scope="workspace",
                    )
                )
            path = base / ".openagent" / "instructions.md"
            candidates.append(
                _InstructionCandidate(
                    path=path,
                    display_path=self._display_workspace_path(path),
                    source=f"instructions.workspace:{self._display_workspace_path(path)}",
                    scope="workspace",
                )
            )
            rules_dir = base / ".openagent" / "rules"
            for rule in sorted(rules_dir.glob("*.md")):
                candidates.append(
                    _InstructionCandidate(
                        path=rule,
                        display_path=self._display_workspace_path(rule),
                        source=f"instructions.workspace:{self._display_workspace_path(rule)}",
                        scope="workspace",
                    )
                )

        if self.options.include_user:
            user_dir = self._user_config_dir()
            for filename in self.options.user_files:
                path = user_dir / filename
                candidates.append(
                    _InstructionCandidate(
                        path=path,
                        display_path=f"~/.openagent/{filename}",
                        source=f"instructions.user:{filename}",
                        scope="user",
                    )
                )
            for rule in sorted((user_dir / "rules").glob("*.md")):
                candidates.append(
                    _InstructionCandidate(
                        path=rule,
                        display_path=f"~/.openagent/rules/{rule.name}",
                        source=f"instructions.user:rules/{rule.name}",
                        scope="user",
                    )
                )
        return candidates

    def _load_candidate(
        self,
        candidate: _InstructionCandidate,
        *,
        remaining_bytes: int,
    ) -> tuple[InstructionItem, str | None] | None:
        try:
            raw = candidate.path.read_bytes()
        except OSError:
            return None
        if b"\x00" in raw[:1024]:
            return None
        try:
            raw.decode("utf-8")
        except UnicodeDecodeError:
            return None

        allowed = max(min(len(raw), self.options.max_file_bytes, remaining_bytes), 0)
        if allowed <= 0:
            return None
        truncated = allowed < len(raw)
        content = raw[:allowed].decode("utf-8").strip()
        item = InstructionItem(
            path=candidate.path.resolve(),
            display_path=candidate.display_path,
            source=candidate.source,
            scope=candidate.scope,
            content=content,
            bytes_read=len(raw[:allowed]),
            truncated=truncated,
        )
        issue = f"truncated:{candidate.display_path}" if truncated else None
        return item, issue

    def _workspace_ancestors(self) -> list[Path]:
        ancestors = [self.workspace_root]
        ancestors.extend(self.workspace_root.parents)
        return ancestors

    def _user_config_dir(self) -> Path:
        if self.options.user_config_dir is not None:
            return self.options.user_config_dir.expanduser().resolve()
        return (Path.home() / ".openagent").resolve()

    def _is_allowed_path(self, path: Path) -> bool:
        if _is_relative_to(path, self.workspace_root):
            return True
        for ancestor in self.workspace_root.parents:
            if path.parent == ancestor or _is_relative_to(path, ancestor / ".openagent"):
                return True
        if self.options.include_user and _is_relative_to(path, self._user_config_dir()):
            return True
        return False

    def _display_workspace_path(self, path: Path) -> str:
        resolved = path.resolve()
        if _is_relative_to(resolved, self.workspace_root):
            return resolved.relative_to(self.workspace_root).as_posix()
        return resolved.name


def _is_relative_to(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


__all__ = [
    "DEFAULT_MAX_FILE_BYTES",
    "DEFAULT_MAX_TOTAL_BYTES",
    "InstructionContext",
    "InstructionContextLoader",
    "InstructionItem",
    "InstructionLoadOptions",
    "InstructionScope",
]
