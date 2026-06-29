# Official Skill Snapshots

This directory contains vendor snapshots of official Codex/OpenAI and Claude skills that are useful for developing and evaluating OpenAgent.

Included groups:

- `codex-system/`: Codex system skills from the local Codex installation.
- `codex-official/`: Codex bundled or curated workflow skills used for browser, Chrome, and GitHub work.
- `claude-official/`: Claude official superpowers focused on planning, debugging, testing, review, and branch workflow.

Personal, home-level skills are intentionally excluded. Keep project-specific and private workflows out of this public repository unless they are rewritten as public OpenAgent examples.

OpenAgent discovers these files automatically because `SkillRegistry` scans `.openagent/skills/**/SKILL.md`.
