OpenAgent Skill 扩展功能实现计划
Context
用户希望在 OpenAgent 中添加 Skill 功能，允许定义 Prompt + Tools 组合 的预配置能力单元。

需求确认：

Skill 类型：Prompt + Tools 组合（预定义 prompt 模板 + 特定工具集）
注册方式：配置文件 YAML/JSON（支持热加载）
核心能力：工具绑定、Prompt 模板、权限控制、钩子系统
文件结构

openagent/src/openagent/
├── core/
│   ├── skill/                          # 新增 Skill 模块
│   │   ├── __init__.py                 # 模块导出
│   │   ├── types.py                    # Skill 数据结构定义
│   │   ├── registry.py                 # SkillRegistry 注册中心
│   │   ├── loader.py                   # 配置加载器（YAML/JSON）
│   │   ├── hooks.py                    # 钩子系统
│   │   └── executor.py                 # Skill 执行器
│   ├── types.py                        # 扩展 AgentConfig
│   └── loop/processor.py               # 集成 Skill 到 AgentLoop
├── skills/                             # 默认 Skill 配置目录
│   ├── code-review.yaml
│   ├── test-runner.yaml
│   └── doc-writer.yaml
└── examples/
    └── skill_usage.py
实现步骤
Step 1: 定义核心类型 (core/skill/types.py)

@dataclass
class SkillPromptTemplate:
    template: str                              # 模板内容
    variables: dict[str, Any]                  # 默认变量值
    system_append: bool = True                 # 是否追加到 system prompt

@dataclass
class SkillToolBinding:
    include: list[str] = []                    # 包含的工具
    exclude: list[str] = []                    # 排除的工具
    groups: list[str] | None = None            # 按组选择

@dataclass
class SkillHook:
    before_execute: str | None                 # 执行前钩子 "module:func"
    after_execute: str | None                  # 执行后钩子
    on_error: str | None                       # 错误钩子

@dataclass
class SkillConfig:
    id: str                                    # 唯一标识
    name: str                                  # 显示名称
    description: str = ""
    version: str = "1.0.0"

    prompt: SkillPromptTemplate | None = None
    tools: SkillToolBinding = field(default_factory=SkillToolBinding)
    permission: PermissionRulesetName = "FULL"
    permission_overrides: list[dict] = []

    max_steps: int | None = None
    timeout: int | None = None
    hooks: SkillHook = field(default_factory=SkillHook)
    tags: list[str] = []
    metadata: dict[str, Any] = {}
Step 2: 配置加载器 (core/skill/loader.py)
支持 YAML 和 JSON 格式
解析配置为 SkillConfig 对象
简单变量替换 {{ variable }}
条件块支持 {{#if var}}...{{/if}}
Step 3: 注册中心 (core/skill/registry.py)

class SkillRegistry:
    def register(skill: SkillConfig) -> None
    def unregister(skill_id: str) -> bool
    def get(skill_id: str) -> SkillConfig
    def list_skills(tags=None, trigger_type=None) -> list[SkillConfig]
    def load_from_file(path: Path) -> SkillConfig
    def load_from_directory(directory: Path, pattern="*.yaml") -> list[SkillConfig]
    def get_hook(hook_path: str) -> HookFunc | None
特性：

从目录扫描加载
文件变化监视（watchdog）+ 自动重载
钩子函数缓存
Step 4: 执行器 (core/skill/executor.py)

class SkillExecutor:
    async def execute(skill_id, variables, context) -> SkillExecutionResult
执行流程：

获取 Skill 配置
执行 before_execute 钩子
应用权限配置
过滤工具
构建 Prompt（变量替换）
返回准备好的执行配置
执行 after_execute 钩子
Step 5: 钩子系统 (core/skill/hooks.py)
钩子函数签名：


async def hook_func(context: SkillExecutionContext) -> SkillExecutionContext | None
内置钩子示例：

log_execution - 记录日志
validate_variables - 验证变量
before_review / after_review - 代码审查钩子
Step 6: 集成 AgentLoop (core/loop/processor.py)
修改 AgentLoop：


class AgentLoop:
    def __init__(self, ..., skill_registry=None, skill_directory=None):
        self.skill_registry = skill_registry or SkillRegistry()
        if skill_directory:
            self.skill_registry.load_from_directory(skill_directory)

    async def run_with_skill(self, skill_id, variables=None, user_text=None):
        """使用指定 Skill 运行"""
        skill = self.skill_registry.get(skill_id)

        # 执行钩子
        # 应用权限
        # 过滤工具
        # 构建 prompt
        # 运行循环
Step 7: 扩展 AgentConfig (core/types.py)

@dataclass
class AgentConfig:
    # ... 现有字段 ...

    # Skill 相关
    default_skill: str | None = None
    skill_directory: str | None = None
    skill_allowlist: list[str] | None = None
    skill_blocklist: list[str] | None = None
YAML 配置示例

# skills/code-review.yaml
id: code-review
name: Code Review
description: 代码审查 Skill
version: "1.0.0"
tags: [code, review]

prompt:
  template: |
    You are a code reviewer. Review the code at {{ target_path }}.
    Focus areas: {{ focus_areas }}
  variables:
    target_path: "."
    focus_areas: "all"
  system_append: true

tools:
  groups: [file, search]
  exclude: [write, edit, bash]

permission: READONLY
max_steps: 30

hooks:
  before_execute: "hooks.code_review:before_review"
  after_execute: "hooks.code_review:after_review"
关键文件
文件	修改类型	说明
core/skill/types.py	新增	Skill 数据结构
core/skill/registry.py	新增	注册中心
core/skill/loader.py	新增	配置加载器
core/skill/executor.py	新增	执行器
core/skill/hooks.py	新增	钩子系统
core/loop/processor.py	修改	集成 Skill
core/types.py	修改	扩展 AgentConfig
skills/*.yaml	新增	示例 Skill 配置
依赖
需要添加到 pyproject.toml：

pyyaml - YAML 解析
watchdog - 文件监视（可选，用于热加载）
验证方式
单元测试：

test_skill_loader.py - 配置加载
test_skill_registry.py - 注册/查询
test_skill_executor.py - 执行流程
集成测试：


# 运行 Skill 示例
python openagent/examples/skill_usage.py
手动验证：

创建 skills/test.yaml
使用 run_with_skill() 执行
检查钩子是否触发
修改 YAML 文件，验证热加载
使用示例

from openagent import AgentLoop, Session
from openagent.core.skill.registry import SkillRegistry

# 初始化
registry = SkillRegistry()
registry.load_from_directory(Path("skills"))

loop = AgentLoop(
    agent=agent,
    session=Session(directory=Path(".")),
    permission_manager=pm,
    skill_registry=registry,
)

# 执行 Skill
async for event in loop.run_with_skill(
    skill_id="code-review",
    variables={"target_path": "src/", "focus_areas": "security"},
):
    print(event)