from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction, PermissionRule
from openagent.core.permission.ruleset import PermissionRuleset
from openagent.core.skill import SkillRegistry
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.sdk import SkillDocument as ExportedSkillDocument
from openagent.sdk import SkillDiscoveryReport as ExportedSkillDiscoveryReport
from openagent.sdk import SkillInfo as ExportedSkillInfo
from openagent.sdk import SkillIssue as ExportedSkillIssue
from openagent.sdk import SkillRegistry as ExportedSkillRegistry


def _write_skill(base: Path, relative: str, *, name: str, description: str, body: str = "") -> Path:
    path = base / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "\n".join(
            [
                "---",
                f"name: {name}",
                f"description: {description}",
                "---",
                "",
                body or f"# {name}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    return path


class SkillRegistryTests(unittest.TestCase):
    def test_registry_discovers_skills_with_priority_and_skips_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "home"
            workspace = root / "repo" / "project" / "workspace"
            workspace.mkdir(parents=True, exist_ok=True)

            _write_skill(root / "repo", ".claude/skills/shared/SKILL.md", name="shared", description="claude")
            _write_skill(root / "repo", ".opencode/skill/shared/SKILL.md", name="shared", description="opencode")
            _write_skill(root / "repo", ".openagent/skill/shared/SKILL.md", name="shared", description="openagent")
            _write_skill(root / "repo", ".openagent/skills/outer/SKILL.md", name="outer", description="outer-skill")
            _write_skill(root / "repo" / "project", ".opencode/skill/near/SKILL.md", name="near", description="inner-opencode")
            _write_skill(root / "repo", ".openagent/skill/near/SKILL.md", name="near", description="outer-openagent")
            _write_skill(home, ".openagent/skills/global/SKILL.md", name="global", description="home-skill")

            invalid_missing = root / "repo" / ".openagent" / "skills" / "missing" / "SKILL.md"
            invalid_missing.parent.mkdir(parents=True, exist_ok=True)
            invalid_missing.write_text("# no frontmatter\n", encoding="utf-8")

            invalid_yaml = root / "repo" / ".openagent" / "skills" / "broken" / "SKILL.md"
            invalid_yaml.parent.mkdir(parents=True, exist_ok=True)
            invalid_yaml.write_text("---\nname: [\ndescription: broken\n---\n", encoding="utf-8")

            registry = SkillRegistry(session_root=workspace, home_dir=home)
            skills = registry.all()
            by_name = {skill.name: skill for skill in skills}

            self.assertEqual(by_name["shared"].description, "openagent")
            self.assertEqual(by_name["near"].description, "inner-opencode")
            self.assertEqual(by_name["global"].description, "home-skill")
            self.assertEqual(by_name["outer"].description, "outer-skill")
            self.assertNotIn("missing", by_name)
            self.assertNotIn("broken", by_name)

            report = registry.report()
            self.assertEqual(report.loaded_count, 4)
            self.assertEqual(report.invalid_count, 2)
            self.assertEqual(report.duplicate_count, 3)
            self.assertEqual(report.scanned_files, 9)

            matches = registry.search("inner opencode")
            self.assertEqual(matches[0].name, "near")
            self.assertGreater(matches[0].score or 0, 0)

    def test_registry_uses_explicit_skill_roots_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            workspace = root / "repo" / "workspace"
            workspace.mkdir(parents=True, exist_ok=True)
            explicit_root = root / "custom-skills"

            _write_skill(root / "repo", ".openagent/skills/default/SKILL.md", name="default", description="default")
            _write_skill(explicit_root, "team/code-review/SKILL.md", name="code-review", description="custom")

            registry = SkillRegistry(session_root=workspace, roots=[str(explicit_root)])
            skills = registry.all()

            self.assertEqual([skill.name for skill in skills], ["code-review"])

    def test_sdk_exports_skill_types(self) -> None:
        self.assertIs(ExportedSkillRegistry, SkillRegistry)
        self.assertEqual(ExportedSkillInfo.__name__, "SkillInfo")
        self.assertEqual(ExportedSkillDocument.__name__, "SkillDocument")
        self.assertEqual(ExportedSkillDiscoveryReport.__name__, "SkillDiscoveryReport")
        self.assertEqual(ExportedSkillIssue.__name__, "SkillIssue")


class SkillToolTests(unittest.IsolatedAsyncioTestCase):
    async def test_skill_tool_lists_loads_and_validates_names(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp) / "workspace"
            workspace.mkdir(parents=True, exist_ok=True)
            _write_skill(workspace, ".openagent/skills/code-review/SKILL.md", name="code-review", description="Review code", body="Use read before edit.")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()

            listed = await toolkit.execute(name="skill", input={}, context={"session_root": str(workspace)})
            self.assertIsNone(listed.error)
            self.assertIn("code-review", listed.output)
            self.assertEqual(listed.metadata["skill_count"], 1)

            loaded = await toolkit.execute(name="skill", input={"name": "code-review"}, context={"session_root": str(workspace)})
            self.assertIsNone(loaded.error)
            self.assertIn("## Skill: code-review", loaded.output)
            self.assertIn("Use read before edit.", loaded.output)
            self.assertEqual(loaded.metadata["skill_name"], "code-review")
            self.assertEqual(
                Path(loaded.metadata["skill_dir"]).resolve(),
                (workspace / ".openagent" / "skills" / "code-review").resolve(),
            )

            missing = await toolkit.execute(name="skill", input={"name": "missing"}, context={"session_root": str(workspace)})
            self.assertIsNotNone(missing.error)
            self.assertIn('Skill "missing" not found.', missing.error or "")
            self.assertIn("code-review", missing.error or "")

    async def test_skill_tool_filters_lists_content_and_reports_diagnostics(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp) / "workspace"
            workspace.mkdir(parents=True, exist_ok=True)
            _write_skill(
                workspace,
                ".openagent/skills/code-review/SKILL.md",
                name="code-review",
                description="Review code carefully",
                body="Inspect diffs and tests.",
            )
            _write_skill(
                workspace,
                ".openagent/skills/research/SKILL.md",
                name="research",
                description="Research external sources",
                body="Collect evidence.",
            )
            _write_skill(
                workspace,
                ".claude/skills/code-review/SKILL.md",
                name="code-review",
                description="duplicate",
                body="Duplicate should not win.",
            )
            broken = workspace / ".openagent" / "skills" / "broken" / "SKILL.md"
            broken.parent.mkdir(parents=True, exist_ok=True)
            broken.write_text("# no frontmatter\n", encoding="utf-8")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()

            listed = await toolkit.execute(
                name="skill",
                input={"query": "review", "include_content": True, "include_diagnostics": True},
                context={"session_root": str(workspace)},
            )

            self.assertIsNone(listed.error)
            self.assertIn('Matched skills for "review"', listed.output)
            self.assertIn("code-review", listed.output)
            self.assertIn("Inspect diffs and tests.", listed.output)
            self.assertNotIn("research", listed.output)
            self.assertIn("Diagnostics:", listed.output)
            self.assertEqual(listed.metadata["skill_count"], 1)
            self.assertEqual(listed.metadata["invalid_count"], 1)
            self.assertEqual(listed.metadata["duplicate_count"], 1)
            self.assertEqual(listed.metadata["query"], "review")

    async def test_skill_tool_respects_explicit_skill_roots_and_is_visible_in_opensandbox(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp) / "workspace"
            workspace.mkdir(parents=True, exist_ok=True)
            explicit_root = Path(tmp) / "shared-skills"

            _write_skill(workspace, ".openagent/skills/default/SKILL.md", name="default", description="default")
            _write_skill(explicit_root, "review/SKILL.md", name="review", description="review")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()

            names = {tool.name for tool in toolkit.get_all_tools(execution_mode="opensandbox")}
            self.assertIn("skill", names)

            listed = await toolkit.execute(
                name="skill",
                input={},
                context={
                    "session_root": str(workspace),
                    "agent_options": {"skill_roots": [str(explicit_root)]},
                },
            )
            self.assertIsNone(listed.error)
            self.assertIn("review", listed.output)
            self.assertNotIn("default", listed.output)


class SkillPermissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_pattern_for_name_and_skill_rule_matching(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.NONE)
        pm.add_rule(PermissionRule(tool="skill", action=PermissionAction.ALLOW, pattern="code-review"))

        self.assertEqual(pm._pattern_for({"name": "code-review"}), "code-review")
        action = await pm.check({"name": "skill", "input": {"name": "code-review"}})
        self.assertEqual(action, PermissionAction.ALLOW)

    async def test_readonly_allows_skill(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        action = await pm.check({"name": "skill", "input": {"name": "code-review"}})
        self.assertEqual(action, PermissionAction.ALLOW)
