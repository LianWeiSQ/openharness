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


@dataclass(frozen=True, slots=True)
class SkillDocument(SkillInfo):
    content: str = ""
