from __future__ import annotations

import logging
from pathlib import Path
from typing import Iterable

from .loader import SkillLoadError, load_skill_document
from .types import SkillDiscoveryReport, SkillDocument, SkillInfo, SkillIssue

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

    def search(self, query: str, *, limit: int | None = None) -> list[SkillInfo]:
        terms = _query_terms(query)
        if not terms:
            results = self.all()
            return results[:limit] if limit is not None else results
        scored: list[SkillInfo] = []
        for document in self._discover().values():
            score = _score_document(document, terms)
            if score <= 0:
                continue
            scored.append(self._to_info(document, score=score))
        scored.sort(key=lambda skill: (-int(skill.score or 0), skill.name))
        return scored[:limit] if limit is not None else scored

    def get(self, name: str) -> SkillDocument | None:
        return self._discover().get(str(name).strip())

    def report(self, *, query: str | None = None, limit: int | None = None) -> SkillDiscoveryReport:
        discovery = self._discover_with_issues()
        if query:
            terms = _query_terms(query)
            skills = [
                self._to_info(document, score=score)
                for document in discovery.documents.values()
                if (score := _score_document(document, terms)) > 0
            ]
            skills.sort(key=lambda skill: (-int(skill.score or 0), skill.name))
        else:
            skills = [self._to_info(document) for document in discovery.documents.values()]
        if limit is not None:
            skills = skills[:limit]
        invalid_count = sum(1 for issue in discovery.issues if issue.kind == "invalid")
        duplicate_count = sum(1 for issue in discovery.issues if issue.kind == "duplicate")
        return SkillDiscoveryReport(
            skills=skills,
            scanned_files=discovery.scanned_files,
            loaded_count=len(discovery.documents),
            invalid_count=invalid_count,
            duplicate_count=duplicate_count,
            issues=list(discovery.issues),
        )

    def _discover(self) -> dict[str, SkillDocument]:
        return self._discover_with_issues().documents

    def _discover_with_issues(self) -> "_DiscoveryResult":
        discovered: dict[str, SkillDocument] = {}
        issues: list[SkillIssue] = []
        scanned_files = 0
        for path in self._iter_skill_files():
            scanned_files += 1
            try:
                document = load_skill_document(path)
            except SkillLoadError as error:
                _LOGGER.warning("Skipping invalid skill file %s: %s", path, error)
                issues.append(SkillIssue(kind="invalid", path=str(path), message=str(error)))
                continue
            if document.name in discovered:
                _LOGGER.warning(
                    "Skipping duplicate skill name %s from %s; already loaded from %s",
                    document.name,
                    document.location,
                    discovered[document.name].location,
                )
                issues.append(
                    SkillIssue(
                        kind="duplicate",
                        path=document.location,
                        message=f"Duplicate skill name: {document.name}",
                        duplicate_of=discovered[document.name].location,
                    )
                )
                continue
            discovered[document.name] = document
        return _DiscoveryResult(documents=discovered, issues=issues, scanned_files=scanned_files)

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
    def _to_info(document: SkillDocument, *, score: int | None = None) -> SkillInfo:
        return SkillInfo(
            name=document.name,
            description=document.description,
            location=document.location,
            directory=document.directory,
            metadata=dict(document.metadata),
            score=score,
        )


class _DiscoveryResult:
    def __init__(self, *, documents: dict[str, SkillDocument], issues: list[SkillIssue], scanned_files: int) -> None:
        self.documents = documents
        self.issues = issues
        self.scanned_files = scanned_files


def _query_terms(query: str | None) -> list[str]:
    if not query:
        return []
    return [term for term in str(query).lower().replace("_", " ").replace("-", " ").split() if term]


def _score_document(document: SkillDocument, terms: list[str]) -> int:
    haystacks = {
        "name": document.name.lower(),
        "description": document.description.lower(),
        "content": document.content.lower(),
    }
    metadata_text = " ".join(f"{key} {value}" for key, value in document.metadata.items()).lower()
    score = 0
    for term in terms:
        if term in haystacks["name"]:
            score += 8
        if term in haystacks["description"]:
            score += 5
        if term in metadata_text:
            score += 3
        if term in haystacks["content"]:
            score += 1
    return score
