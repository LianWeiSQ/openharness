# OpenAgent 项目技术文档

> 当前版本：`openagent-core` 0.1.0
> 代码范围：`src/openagent/`

本文档以当前仓库代码为准，描述已经落地的运行时能力、真实目录结构、运行方式和扩展边界。需要后续整改的问题统一记录在 [整改清单](remediation-plan.md)。

## 1. 项目定位

OpenAgent Core 是一个以 `AgentLoop` 为中心的 Python 智能体运行时。它负责把用户输入、模型流式输出、工具调用、权限规则、会话状态、上下文预算、问题澄清和文件变更追踪串成一个可嵌入上层产品的核心引擎。

已经落地的核心能力：

- 流式文本输出与工具调用事件
- 内置工具注册、插件工具加载和远程 MCP 工具桥接
- `FULL` / `READONLY` / `PLAN_ONLY` / `NONE` 权限规则集
- 上下文预算检查、工具输出裁剪、上下文压缩和溢出降级
- 文件快照与 patch 事件
- 结构化问题澄清的暂停/恢复机制
- OpenAI-compatible 与 DashScope Provider
- 本地工作区与可选远端 sandbox runtime 抽象

当前仍未完整落地的部分：

- Anthropic、Gemini、Ollama Provider 仍是 stub
- CLI / Web Console 不属于当前公开 core 范围，后续如需要应作为独立 package 接入
- 会话 Storage 抽象尚未接入主执行链路
- Memory 仍是进程内字典，不是长期记忆系统

## 2. 仓库结构

```text
.
├── src/openagent/
│   ├── adapter/               # AgentAdapter、MemoryAdapter、MCP/Toolkit 兼容入口
│   ├── core/
│   │   ├── agent/             # BaseAgent / UniversalAgent / PlanAgent / ExploreAgent
│   │   ├── execution/         # local / optional sandbox 工作区 runtime
│   │   ├── loop/              # AgentLoop、快照、重试、doom-loop
│   │   ├── mcp/               # 远程 MCP 配置、运行时与工具桥接
│   │   ├── permission/        # 权限规则、规则集、管理器
│   │   ├── provider/          # LLM Provider 抽象与实现
│   │   ├── question/          # 结构化问题请求与回复
│   │   ├── session/           # 会话、todo、存储抽象
│   │   ├── skill/             # SKILL.md 加载与发现
│   │   ├── tool/              # 工具定义、注册表、内置工具、中间件
│   │   ├── context_budget.py  # 上下文预算配置与检查
│   │   ├── context_messages.py
│   │   ├── message_materializer.py
│   │   └── types.py
│   ├── prompts/               # build / plan / explore 默认提示词
│   └── sdk/http_runtime.py    # 对外聚合导出
├── src/examples/              # 示例脚本
├── src/tests/                 # unittest 测试
├── doc/                       # 项目文档与设计文档
├── skills/                    # 本仓库自带 skill
├── README.md                  # 项目入口
├── Agent.md                   # 代码对齐版架构分析
├── CLAUD.md                   # 历史分析归档说明
└── pyproject.toml
```

## 3. 运行方式

推荐安装为 editable 包：

```bash
python -m venv .venv
source .venv/bin/activate
python -m pip install -e .
```

不安装时可以临时指定源码路径：

```bash
PYTHONPATH=src python src/examples/run_query_only.py "你好"
```

完整测试命令：

```bash
PYTHONPATH=src python -m unittest discover -s src/tests -p "test_*.py"
```

当前本机验证注意事项：

- 未安装依赖时，内置工具加载会因 `yaml` 缺失失败。
- MCP 相关测试依赖 `httpx` 和 `mcp`，但 `httpx` 未写入 `pyproject.toml`。
- CLI / Web Console 已从公开 core 仓库剥离，当前测试集不覆盖交互式前端入口。
- 远端 sandbox 适配属于 optional 能力，公开 core 默认不要求安装 sandbox SDK。

## 4. 总体架构

```text
BaseAgent / UniversalAgent / PlanAgent / ExploreAgent
        │
        ▼
AgentLoop
  ├─ AgentAdapter → LanguageModel.stream()
  ├─ ToolkitAdapter → ToolRegistry → builtin / plugin / MCP tools
  ├─ PermissionManager
  ├─ QuestionManager
  ├─ SnapshotManager
  ├─ Session
  ├─ WorkspaceRuntime
  └─ context budget / tool policy / web convergence
```

`AgentLoop.run(user_text)` 是主入口。它是一个异步生成器，逐步 yield `StreamEvent`，上层 UI 或服务可以边运行边消费文本、工具调用结果、问题请求和 patch。

主要事件类型：

| 事件 | 说明 |
| --- | --- |
| `text-start` / `text-delta` / `text-end` | 模型文本流 |
| `tool-call` | 模型请求调用工具 |
| `tool-result` | 工具执行结果 |
| `question-request` | 工具请求宿主向用户提问 |
| `step-start` / `step-finish` | Loop 单步开始/结束 |
| `patch` | 当前 step 造成的文件变更 |
| `error` | 运行错误 |

## 5. 主执行链路

一次 `AgentLoop.run()` 大致流程如下：

1. 设置权限规则集，并把用户输入追加到 `Session.messages`。
2. 如果使用默认系统提示词，基于用户输入启用首轮工具策略守卫。
3. 每个 step 开始时创建工作目录快照。
4. 根据 Agent 配置、执行环境和 Web 研究状态过滤可用工具。
5. 准备模型消息：注入当前本地时间、检查上下文预算、必要时裁剪/压缩/降级。
6. 通过 `AgentAdapter.reply_stream()` 调用 `LanguageModel.stream()`。
7. 收集 assistant 文本与 tool calls，写回会话。
8. 逐个执行工具：经过权限中间件、日志中间件和工具实现。
9. 如果工具触发 `question`，Loop 发出 `question-request`，会话进入 `PAUSED`。
10. 将工具结果投影成 `tool` message，并按预算裁剪展示输出。
11. 对比快照，输出 patch。
12. 如果还有工具调用或后续提醒，进入下一 step；否则结束。

## 6. Agent 层

Agent 类型很薄，核心差异来自默认系统提示词：

| Agent | 默认提示词 | 典型用途 |
| --- | --- | --- |
| `UniversalAgent` | `prompts/build.txt` | 通用构建、编码、调试 |
| `PlanAgent` | `prompts/plan.txt` | 规划、架构设计 |
| `ExploreAgent` | `prompts/explore.txt` | 只读探索 |

`BaseAgent` 持有 `AgentConfig`、`LanguageModel` 和最终 system prompt。`resolve_system_prompt()` 会按“显式 system prompt → 默认 prompt → 追加 `config.prompt`”的规则解析提示词。

## 7. Provider 层

真正被 Loop 依赖的是 `LanguageModel` Protocol：

```python
async def stream(
    *,
    system: str | None,
    messages,
    tools,
    temperature=None,
    max_output_tokens=None,
    options=None,
):
    ...
```

当前 Provider 状态：

| Provider | 状态 | 说明 |
| --- | --- | --- |
| `OpenAIProvider` | 已实现 | OpenAI-compatible `/chat/completions`，支持 SSE、工具调用和非流式 fallback |
| `DashScopeProvider` | 已实现 | DashScope compatible-mode，支持 Qwen 模型和工具调用 |
| `AnthropicProvider` | stub | `get_language_model()` 抛 `NotImplementedError` |
| `GeminiProvider` | stub | `get_language_model()` 抛 `NotImplementedError` |
| `OllamaProvider` | stub | `get_language_model()` 抛 `NotImplementedError` |

OpenAI-compatible 环境变量：

- `OPENAI_API_KEY`
- `OPENAI_BASE_URL`
- `OPENAI_HOST_HEADER`
- `OPENAI_MODEL`
- `OPENAI_CONTEXT_WINDOW`
- `OPENAI_MAX_OUTPUT`

DashScope 环境变量：

- `DASHSCOPE_API_KEY`
- `DASHSCOPE_BASE_URL`
- `DASHSCOPE_MODEL`
- `DASHSCOPE_TEMPERATURE`
- `DASHSCOPE_STREAM`

## 8. 工具系统

工具系统由四层组成：

- `ToolDefinition`：工具元信息、参数类型、执行函数、危险标记和执行范围
- `ToolRegistry`：工具注册表，支持命名空间和插件加载
- `ToolkitAdapter`：工具暴露与执行入口
- `Middleware`：权限检查和日志等横切逻辑

内置工具从 `core/tool/builtin/__init__.py` 统一注册：

| 工具 | 分组 | 执行范围 | 危险 | 功能 |
| --- | --- | --- | --- | --- |
| `read` | file | workspace | 否 | 读取文件 |
| `write` | file | workspace | 是 | 写文件 |
| `edit` | file | workspace | 是 | 字符串替换 |
| `glob` | file | workspace | 否 | 文件匹配 |
| `grep` | file | workspace | 否 | 正则搜索 |
| `ls` | file | workspace | 否 | 目录列表 |
| `bash` | shell | workspace | 是 | 执行 shell 命令 |
| `code_search` | search | host_only | 否 | 主机侧代码搜索 |
| `web_fetch` | web | agnostic | 是 | 抓取 URL |
| `web_search` | web | agnostic | 是 | Exa MCP 搜索 |
| `skill` | skill | agnostic | 否 | 查询 skill |
| `memory_read` / `memory_write` | memory | agnostic | 否 | 进程内记忆读写 |
| `todoread` / `todowrite` | todo | agnostic | 否 | Todo 读写 |
| `question` | interactive | agnostic | 否 | 结构化提问 |

安全约束：

- workspace 工具通过 `WorkspaceRuntime` 解析路径，防止逃出工作区。
- 已存在文件的 `write` / `edit` 要求先 `read`。
- `bash` 拦截明显删除类命令。
- 工具输出默认按 2000 行、50KB 截断；完整输出必要时写入 `.openagent/tool_output/`。
- 远端 sandbox 模式下只暴露 `workspace` 和 `agnostic` 工具，不暴露 `host_only` 工具。

## 9. 权限系统

`PermissionManager` 使用三态决策：

- `ALLOW`
- `DENY`
- `ASK`

规则使用 `fnmatch` 匹配，遵循 last match wins。

内置 ruleset：

| Ruleset | 行为 |
| --- | --- |
| `FULL` | 全部允许 |
| `READONLY` | 默认拒绝，只允许 `read`、`glob`、`grep`、`ls`、`skill`、`todoread`、`question` |
| `PLAN_ONLY` | 默认 ASK，允许只读工具、`todowrite` 和 `question` |
| `NONE` | 全部拒绝 |

注意：`ASK` 需要宿主注入 `ask_user_func`。如果没有注入，当前会抛 `PermissionAskRequiredError`，最终表现为工具错误，而不是 UI 确认弹窗。

## 10. Session、Question 与状态

`Session` 是运行时状态容器，包含：

- `id`
- `directory`
- `status`
- `messages`
- `todos`
- `metadata`

它还提供 `remember_file_read()` / `has_read_file()`，用于“先读后写”的文件保护。

`QuestionManager` 支持结构化提问：

- 工具调用 `question` 后创建 `QuestionRequest`
- Loop 发出 `question-request` 事件
- `Session.status` 切到 `PAUSED`
- 宿主通过 `reply()` 或 `reject()` 恢复或拒绝

## 11. 上下文预算

上下文预算由 `context_budget.py`、`context_messages.py`、`message_materializer.py` 和 `token_counting.py` 协作完成。

支持策略：

- `auto`：裁剪旧工具输出 → 压缩摘要 → overflow trim → text-only final attempt
- `compact`：允许压缩，但不做最终 overflow trim
- `error`：溢出时直接报错

计数方式：

- `auto`
- `tiktoken`
- `heuristic`

Loop 会把最近预算诊断写入 `Session.metadata["last_context_budget"]`，把模型用量写入 `Session.metadata["last_model_usage"]`。

## 12. MCP 远程工具

`core/mcp/` 已经不是占位实现。当前包含：

- `config.py`：从 JSON、文件路径或 `OPENAGENT_MCP_CONFIG` 解析 MCP 配置
- `runtime.py`：`RemoteMcpManager`，支持 streamable HTTP 和 SSE，带传输回退
- `bridge.py`：把远程 MCP 工具注册为普通 `ToolDefinition`
- `types.py`：MCP 配置、快照、工具描述符和调用结果类型

远程工具命名规则：

```text
mcp_tool_{server_name}_{tool_name}
```

桥接后的 MCP 工具默认：

- `group="mcp"`
- `dangerous=True`
- `execution_scope="agnostic"`

## 13. Execution Runtime

执行层用于把“工具语义”和“执行位置”解耦。

`Session.metadata["execution"]` 缺省时使用本地模式：

```json
{
  "mode": "local"
}
```

远端 sandbox 绑定示例：

```json
{
  "mode": "opensandbox",
  "sandbox_id": "sbx_123",
  "remote_workdir": "/workspace/project"
}
```

约束：

- 每个 session 同一时刻只绑定一种执行模式。
- 远端 sandbox 模式要求 `sandbox_id` 和绝对 POSIX `remote_workdir`。
- 路径不能逃出 `remote_workdir`。
- 绑定失败直接报错，不自动回退到本地。
- 连接密钥、请求头等敏感连接信息只用于 runtime 初始化，不写入工具结果 metadata。

## 14. Skill 系统

Skill 由 `SKILL.md` 描述，格式是 YAML frontmatter + Markdown 正文。加载器依赖 `PyYAML`。

`SkillRegistry` 支持从显式 roots 或默认目录发现技能，默认搜索 `.openagent/skill`、`.openagent/skills`、`.opencode/skill`、`.claude/skills` 等位置。

内置 `skill` 工具会把可用技能暴露给模型。

## 15. SDK 入口

`src/openagent/sdk/http_runtime.py` 是面向外部服务的聚合入口，导出：

- Agent：`UniversalAgent`、`PlanAgent`、`ExploreAgent`
- Loop：`AgentLoop`
- Provider：`OpenAIProvider`、`DashScopeProvider`
- Permission：`PermissionManager`、`PermissionRuleset` 等
- MCP：`RemoteMcpManager`、`load_mcp_config_from_sources`
- Session、Skill、Toolkit、`AgentConfig`、`Model`

业务侧也可以直接从具体模块导入，便于更明确地控制依赖边界。

## 16. 扩展方式

新增 Provider：

1. 新建 `core/provider/<name>.py`。
2. 实现 `ProviderBase`。
3. 返回满足 `LanguageModel` Protocol 的对象。
4. 在需要的宿主入口中注册到 `ProviderManager`。

新增内置工具：

1. 在 `core/tool/builtin/` 新建模块。
2. 实现 `register(registry: ToolRegistry)`。
3. 使用 `registry.define_tool()` 声明参数、描述、分组和执行范围。
4. 在 `builtin/__init__.py` 中注册模块。

新增插件工具：

1. 插件文件或目录提供 `register(registry)`。
2. 通过 `AgentConfig.options["tool_paths"]` 传入。
3. 文件名会作为工具命名空间。

新增 MCP 服务器：

1. 编写 MCP JSON 配置。
2. 通过 `OPENAGENT_MCP_CONFIG` 或 API 参数传入。
3. `RemoteMcpManager.refresh_all()` 发现工具。
4. `ToolkitAdapter.register_mcp()` 桥接到工具系统。

## 17. 文档维护规则

- README 只保留入口、运行方式、已知限制和文档索引。
- 本文档描述当前代码事实，不写长期愿景。
- 设计细节放在专题文档，例如 Web 研究收敛和评测适配。
- 已发现但未处理的问题进入 [整改清单](remediation-plan.md)，不要散落在多份文档里。
