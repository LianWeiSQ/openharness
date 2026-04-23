from __future__ import annotations

from dataclasses import dataclass, field

from ...skill import SkillRegistry
from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry


@dataclass
class SkillParameters:
    name: str | None = field(default=None, metadata={"description": "Skill name. Leave empty to list available skills."})


def _skill_roots(ctx: ToolContext) -> list[str] | None:
    options = ctx.extra.get("agent_options")
    if not isinstance(options, dict):
        return None
    roots = options.get("skill_roots")
    if not isinstance(roots, list):
        return None
    return [str(root) for root in roots if str(root).strip()]


async def skill_tool(args: SkillParameters, ctx: ToolContext) -> ToolOutput:
    registry = SkillRegistry(session_root=ctx.session_root, roots=_skill_roots(ctx))
    skills = registry.all()
    requested_name = str(args.name or "").strip()

    if not requested_name:
        if not skills:
            return ToolOutput(title="Available skills", output="No skills available.", metadata={"skill_count": 0})
        lines = ["Available skills:"]
        for skill in skills:
            lines.append(f"- `{skill.name}`: {skill.description}")
        return ToolOutput(
            title="Available skills",
            output="\n".join(lines),
            metadata={"skill_count": len(skills)},
        )

    document = registry.get(requested_name)
    if document is None:
        available = ", ".join(skill.name for skill in skills) or "none"
        raise RuntimeError(f'Skill "{requested_name}" not found. Available skills: {available}')

    body = "\n".join(
        [
            f"## Skill: {document.name}",
            "",
            f"**Base directory**: {document.directory}",
            "",
            document.content,
        ]
    ).strip()
    return ToolOutput(
        title=f"Loaded skill: {document.name}",
        output=body,
        metadata={
            "skill_name": document.name,
            "skill_location": document.location,
            "skill_dir": document.directory,
            "skill_count": len(skills),
        },
    )


def register(registry: ToolRegistry) -> None:
    registry.define_tool(
        tool_id="skill",
        parameters=SkillParameters,
        description_md="skill.md",
        group="skill",
        dangerous=False,
        execution_scope="agnostic",
    )(skill_tool)


__all__ = ["register"]
