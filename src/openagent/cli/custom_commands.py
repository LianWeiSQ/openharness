from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

SHELL_OUTPUT_LIMIT_CHARS = 12000
FILE_REFERENCE_LIMIT_CHARS = 50000


@dataclass(frozen=True, slots=True)
class CustomCommand:
    name: str
    path: Path
    scope: str
    description: str = ""
    agent: str | None = None
    model: str | None = None
    template: str = ""

    def to_dict(self, *, include_template: bool = False) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "name": self.name,
            "path": str(self.path),
            "scope": self.scope,
            "description": self.description,
            "agent": self.agent,
            "model": self.model,
        }
        if include_template:
            payload["template"] = self.template
        return payload


def command_dirs(*, workspace: str | Path, extra_dirs: list[str] | None = None) -> list[tuple[str, Path]]:
    workspace_path = Path(workspace).expanduser().resolve()
    dirs = [
        ("global", Path.home() / ".config" / "openagent" / "commands"),
        ("project", workspace_path / ".openagent" / "commands"),
    ]
    for raw in extra_dirs or []:
        dirs.append(("custom", Path(raw).expanduser().resolve()))
    return dirs


def discover_commands(*, workspace: str | Path, extra_dirs: list[str] | None = None) -> list[CustomCommand]:
    commands: dict[str, CustomCommand] = {}
    for scope, directory in command_dirs(workspace=workspace, extra_dirs=extra_dirs):
        if not directory.exists() or not directory.is_dir():
            continue
        for path in sorted(directory.glob("*.md")):
            command = load_command_file(path, scope=scope)
            commands[command.name] = command
    return sorted(commands.values(), key=lambda item: item.name)


def resolve_command(name: str, *, workspace: str | Path, extra_dirs: list[str] | None = None) -> CustomCommand:
    normalized = Path(name).stem if name.endswith(".md") else name
    for command in discover_commands(workspace=workspace, extra_dirs=extra_dirs):
        if command.name == normalized:
            return command
    raise FileNotFoundError(f"Command not found: {name}")


def load_command_file(path: Path, *, scope: str) -> CustomCommand:
    metadata, template = split_frontmatter(path.read_text(encoding="utf-8"))
    return CustomCommand(
        name=path.stem,
        path=path,
        scope=scope,
        description=str(metadata.get("description") or ""),
        agent=str(metadata["agent"]) if metadata.get("agent") else None,
        model=str(metadata["model"]) if metadata.get("model") else None,
        template=template.strip(),
    )


def split_frontmatter(raw: str) -> tuple[dict[str, Any], str]:
    text = raw.lstrip("\ufeff")
    if not text.startswith("---\n"):
        return {}, text
    end = text.find("\n---", 4)
    if end < 0:
        return {}, text
    frontmatter = text[4:end].strip()
    template = text[end + len("\n---") :].lstrip("\r\n")
    metadata = yaml.safe_load(frontmatter) if frontmatter else {}
    return (metadata if isinstance(metadata, dict) else {}), template


def render_command(
    command: CustomCommand,
    arguments: list[str],
    *,
    workspace: str | Path,
    allow_shell: bool = True,
) -> str:
    workspace_path = Path(workspace).expanduser().resolve()
    text = apply_argument_placeholders(command.template, arguments)
    if allow_shell:
        text = inject_shell_outputs(text, workspace=workspace_path)
    text = inject_file_references(text, workspace=workspace_path)
    return text.strip()


def apply_argument_placeholders(template: str, arguments: list[str]) -> str:
    text = template.replace("$ARGUMENTS", " ".join(arguments))

    def replace_positional(match: re.Match[str]) -> str:
        index = int(match.group(1)) - 1
        if 0 <= index < len(arguments):
            return arguments[index]
        return ""

    return re.sub(r"\$(\d+)", replace_positional, text)


def inject_shell_outputs(template: str, *, workspace: Path) -> str:
    def replace_shell(match: re.Match[str]) -> str:
        command = match.group(1).strip()
        if not command:
            return ""
        try:
            result = subprocess.run(
                command,
                cwd=workspace,
                shell=True,
                text=True,
                capture_output=True,
                timeout=30,
                check=False,
            )
        except Exception as error:  # noqa: BLE001 - command rendering should surface compact diagnostics.
            return f"```text\n$ {command}\n[openagent command failed: {error}]\n```"
        output = "\n".join(part for part in (result.stdout.rstrip(), result.stderr.rstrip()) if part)
        if len(output) > SHELL_OUTPUT_LIMIT_CHARS:
            output = output[:SHELL_OUTPUT_LIMIT_CHARS].rstrip() + "\n[truncated]"
        status = f"\n[exit {result.returncode}]" if result.returncode else ""
        return f"```text\n$ {command}\n{output}{status}\n```"

    return re.sub(r"!\`([^`]+)\`", replace_shell, template)


def inject_file_references(template: str, *, workspace: Path) -> str:
    def replace_file(match: re.Match[str]) -> str:
        raw_path = match.group(1)
        lookup_path = raw_path.rstrip(".,;:)")
        suffix = raw_path[len(lookup_path) :]
        path = Path(lookup_path).expanduser()
        if not path.is_absolute():
            path = workspace / path
        if not path.exists() or not path.is_file():
            return match.group(0)
        content = path.read_text(encoding="utf-8", errors="replace")
        if len(content) > FILE_REFERENCE_LIMIT_CHARS:
            content = content[:FILE_REFERENCE_LIMIT_CHARS].rstrip() + "\n[truncated]"
        return f"Attached file: {path}\n\n```text\n{content}\n```{suffix}"

    return re.sub(r"(?<!\S)@([A-Za-z0-9_./~+-][A-Za-z0-9_./~+-]*)", replace_file, template)
