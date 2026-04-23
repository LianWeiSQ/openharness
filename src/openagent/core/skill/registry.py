from __future__ import annotations

import logging
from pathlib import Path
from typing import Iterable

from .loader import SkillLoadError, load_skill_document
from .types import SkillDocument, SkillInfo

_LOGGER = logging.getLogger(__name__)
_DEFAULT_PATTERNS: tuple[tuple[str, ...], ...] = (
    (".openagent", "skill"),
    (".openagent", "skills"),
    (".opencode", "skill"),
    (".opencode", "skills"),
    (".claude", "skills"),
)


class SkillRegistry:
    def __init__(
        self,
        *,
        session_root: str | Path | None = None,
        roots: list[str] | tuple[str, ...] | None = None,
        home_dir: str | Path | None = None,
    ) -> None:
        self.session_root = Path(session_root or Path.cwd()).expanduser().resolve()
        self.roots = [str(root) for root in (roots or []) if str(root).strip()]
        self.home_dir = Path(home_dir or Path.home()).expanduser().resolve()

    def all(self) -> list[SkillInfo]:
        return [self._to_info(document) for document in self._discover().values()]

    def get(self, name: str) -> SkillDocument | None:
        return self._discover().get(str(name).strip())

    def _discover(self) -> dict[str, SkillDocument]:
        discovered: dict[str, SkillDocument] = {}
        for path in self._iter_skill_files():
            try:
                document = load_skill_document(path)
            except SkillLoadError as error:
                _LOGGER.warning("Skipping invalid skill file %s: %s", path, error)
                continue
            if document.name in discovered:
                _LOGGER.warning(
                    "Skipping duplicate skill name %s from %s; already loaded from %s",
                    document.name,
                    document.location,
                    discovered[document.name].location,
                )
                continue
            discovered[document.name] = document
        return discovered

    def _iter_skill_files(self) -> Iterable[Path]:
        if self.roots:
            yield from self._iter_explicit_skill_files()
            return

        seen: set[Path] = set()
        for base_dir in self._iter_workspace_ancestors():
            yield from self._iter_pattern_matches(base_dir, seen)
        yield from self._iter_pattern_matches(self.home_dir, seen)

    def _iter_explicit_skill_files(self) -> Iterable[Path]:
        seen: set[Path] = set()
        for raw_root in self.roots:
            root = Path(raw_root).expanduser()
            if not root.is_absolute():
                root = (self.session_root / root).resolve()
            else:
                root = root.resolve()
            if root.is_file() and root.name == "SKILL.md":
                if root not in seen:
                    seen.add(root)
                    yield root
                continue
            if not root.exists():
                continue
            if root.is_dir():
                for match in sorted(root.rglob("SKILL.md")):
                    resolved = match.resolve()
                    if resolved in seen:
                        continue
                    seen.add(resolved)
                    yield resolved

    def _iter_workspace_ancestors(self) -> Iterable[Path]:
        current = self.session_root if self.session_root.is_dir() else self.session_root.parent
        while True:
            if current != self.home_dir:
                yield current
            if current.parent == current:
                break
            current = current.parent

    def _iter_pattern_matches(self, base_dir: Path, seen: set[Path]) -> Iterable[Path]:
        for parts in _DEFAULT_PATTERNS:
            candidate = base_dir.joinpath(*parts)
            if not candidate.is_dir():
                continue
            for match in sorted(candidate.rglob("SKILL.md")):
                resolved = match.resolve()
                if resolved in seen:
                    continue
                seen.add(resolved)
                yield resolved

    @staticmethod
    def _to_info(document: SkillDocument) -> SkillInfo:
        return SkillInfo(
            name=document.name,
            description=document.description,
            location=document.location,
            directory=document.directory,
            metadata=dict(document.metadata),
        )
