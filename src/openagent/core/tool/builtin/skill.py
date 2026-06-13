from __future__ import annotations

from dataclasses import dataclass, field

from ...skill import SkillRegistry
from ..definition import ToolContext, ToolExecutionSchema, ToolOutput
from ..registry import ToolRegistry


@dataclass
class SkillParameters:
    name: str | None = field(default=None, metadata={"description": "Skill name. Leave empty to list available skills."})
    query: str | None = field(default=None, metadata={"description": "Optional keyword query used when listing skills."})
    limit: int | None = field(default=None, metadata={"description": "Maximum number of skills to list."})
    include_content: bool = field(default=False, metadata={"description": "Include skill content when listing matched skills."})
    include_diagnostics: bool = field(default=False, metadata={"description": "Include discovery diagnostics for invalid or duplicate skills."})


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
    requested_name = str(args.name or "").strip()
    requested_query = str(args.query or "").strip()
    limit = _normalize_limit(args.limit)

    if not requested_name:
        report = registry.report(query=requested_query or None, limit=limit)
        skills = report.skills
        if not skills:
            lines = ["No skills available."]
            if requested_query:
                lines = [f'No skills matched query "{requested_query}".']
            if args.include_diagnostics:
                lines.extend(_diagnostic_lines(report))
            return ToolOutput(
                title="Available skills",
                output="\n".join(lines),
                metadata=_report_metadata(report, query=requested_query or None),
            )
        lines = ["Available skills:" if not requested_query else f'Matched skills for "{requested_query}":']
        for skill in skills:
            score = f" score={skill.score}" if skill.score is not None else ""
            lines.append(f"- `{skill.name}`:{score} {skill.description}")
            if args.include_content:
                document = registry.get(skill.name)
                if document is not None:
                    lines.append(_render_skill_document(document, include_header=False))
        if args.include_diagnostics:
            lines.extend(_diagnostic_lines(report))
        return ToolOutput(
            title="Available skills",
            output="\n".join(lines),
            metadata=_report_metadata(report, query=requested_query or None),
        )

    document = registry.get(requested_name)
    if document is None:
        skills = registry.search(requested_query, limit=limit) if requested_query else registry.all()
        available = ", ".join(skill.name for skill in skills) or "none"
        raise RuntimeError(f'Skill "{requested_name}" not found. Available skills: {available}')

    report = registry.report()
    body = _render_skill_document(document)
    return ToolOutput(
        title=f"Loaded skill: {document.name}",
        output=body,
        metadata={
            "skill_name": document.name,
            "skill_location": document.location,
            "skill_dir": document.directory,
            "skill_count": report.loaded_count,
            "scanned_files": report.scanned_files,
            "invalid_count": report.invalid_count,
            "duplicate_count": report.duplicate_count,
        },
    )


def _normalize_limit(value: int | None) -> int | None:
    if value is None:
        return None
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return None
    return parsed if parsed > 0 else None


def _render_skill_document(document, *, include_header: bool = True) -> str:
    lines: list[str] = []
    if include_header:
        lines.extend([f"## Skill: {document.name}", "", f"**Base directory**: {document.directory}", ""])
    lines.append(document.content)
    return "\n".join(lines).strip()


def _report_metadata(report, *, query: str | None = None) -> dict[str, object]:
    payload: dict[str, object] = {
        "skill_count": len(report.skills),
        "loaded_count": report.loaded_count,
        "scanned_files": report.scanned_files,
        "invalid_count": report.invalid_count,
        "duplicate_count": report.duplicate_count,
    }
    if query:
        payload["query"] = query
    if report.issues:
        payload["issues"] = [
            {
                "kind": issue.kind,
                "path": issue.path,
                "message": issue.message,
                "duplicate_of": issue.duplicate_of,
            }
            for issue in report.issues
        ]
    return payload


def _diagnostic_lines(report) -> list[str]:
    if not report.issues:
        return []
    lines = ["", "Diagnostics:"]
    for issue in report.issues:
        suffix = f" duplicate_of={issue.duplicate_of}" if issue.duplicate_of else ""
        lines.append(f"- {issue.kind}: {issue.path} - {issue.message}{suffix}")
    return lines


def register(registry: ToolRegistry) -> None:
    registry.define_tool(
        tool_id="skill",
        parameters=SkillParameters,
        description_md="skill.md",
        group="skill",
        dangerous=False,
        execution_scope="agnostic",
        execution_schema=ToolExecutionSchema.readonly(batch_group="skill"),
    )(skill_tool)


__all__ = ["register"]
