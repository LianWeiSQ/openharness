from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import yaml

from .types import SkillDocument


class SkillLoadError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class ParsedFrontmatter:
    data: dict[str, object]
    content: str


def load_skill_document(path: str | Path) -> SkillDocument:
    skill_path = Path(path).expanduser().resolve()
    if not skill_path.is_file():
        raise SkillLoadError(f"Skill file not found: {skill_path}")

    parsed = _parse_frontmatter(skill_path.read_text(encoding="utf-8"), skill_path)
    name = str(parsed.data.get("name") or "").strip()
    description = str(parsed.data.get("description") or "").strip()
    if not name:
        raise SkillLoadError(f"Skill file missing required frontmatter field 'name': {skill_path}")
    if not description:
        raise SkillLoadError(f"Skill file missing required frontmatter field 'description': {skill_path}")

    metadata = {key: value for key, value in parsed.data.items() if key not in {"name", "description"}}
    return SkillDocument(
        name=name,
        description=description,
        location=str(skill_path),
        directory=str(skill_path.parent),
        metadata=metadata,
        content=parsed.content,
    )


def _parse_frontmatter(text: str, path: Path) -> ParsedFrontmatter:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        raise SkillLoadError(f"Skill file missing YAML frontmatter: {path}")

    closing_index: int | None = None
    for index in range(1, len(lines)):
        if lines[index].strip() == "---":
            closing_index = index
            break
    if closing_index is None:
        raise SkillLoadError(f"Skill file has unterminated YAML frontmatter: {path}")

    frontmatter_text = "\n".join(lines[1:closing_index])
    body = "\n".join(lines[closing_index + 1 :])
    try:
        data = yaml.safe_load(frontmatter_text) or {}
    except yaml.YAMLError as error:
        raise SkillLoadError(f"Failed to parse skill frontmatter: {path}: {error}") from error
    if not isinstance(data, dict):
        raise SkillLoadError(f"Skill frontmatter must be a YAML object: {path}")

    normalized = {str(key): value for key, value in data.items()}
    return ParsedFrontmatter(data=normalized, content=body)
