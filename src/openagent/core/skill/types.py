from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True, slots=True)
class SkillInfo:
    name: str
    description: str
    location: str
    directory: str
    metadata: dict[str, Any] = field(default_factory=dict)
    score: int | None = None


@dataclass(frozen=True, slots=True)
class SkillDocument(SkillInfo):
    content: str = ""


@dataclass(frozen=True, slots=True)
class SkillIssue:
    kind: str
    path: str
    message: str
    duplicate_of: str | None = None


@dataclass(frozen=True, slots=True)
class SkillDiscoveryReport:
    skills: list[SkillInfo]
    scanned_files: int
    loaded_count: int
    invalid_count: int
    duplicate_count: int
    issues: list[SkillIssue] = field(default_factory=list)
