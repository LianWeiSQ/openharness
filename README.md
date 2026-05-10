# OpenAgent Core

OpenAgent Core 是一个 Python 智能体运行时内核，围绕 `AgentLoop` 编排模型调用、工具执行、权限控制、会话状态、上下文预算和文件变更追踪。

当前仓库采用标准 `src/` 布局，实际源码在 `src/openagent/`。

## 当前定位

项目已经具备一套可运行的 Agent core，而不是单纯设计稿：

- Agent 层：`UniversalAgent`、`PlanAgent`、`ExploreAgent`
- Loop 层：流式模型调用、工具调用、多步循环、重试、doom-loop 检测
- Tool 层：内置文件、Shell、搜索、Web、记忆、Todo、Question、Skill 工具
- Permission 层：`FULL`、`READONLY`、`PLAN_ONLY`、`NONE` 四组规则
- Provider 层：OpenAI-compatible 与 DashScope 已实现，Anthropic/Gemini/Ollama 仍是 stub
- MCP 层：远程 MCP 工具发现、桥接与调用
- Execution 层：本地工作区与 OpenSandbox 工作区 runtime 抽象
- Context 层：上下文预算、工具输出裁剪、压缩摘要、溢出降级

## 目录结构

```text
.
├── src/openagent/             # Python 包源码
│   ├── core/                  # AgentLoop、工具、Provider、权限、会话等核心模块
│   ├── adapter/               # AgentAdapter、MemoryAdapter、MCP/Toolkit 兼容导出
│   ├── prompts/               # build/plan/explore 默认系统提示词
│   └── sdk/                   # 对外 SDK 入口
├── src/examples/              # 最小示例脚本
├── src/tests/                 # unittest 测试
├── doc/                       # 项目技术文档和设计文档
├── skills/                    # 本仓库自带 skill
├── Agent.md                   # 代码对齐版架构分析
├── CLAUD.md                   # 早期深度分析，部分内容已过期
├── REVIEW.md                  # 代码审查 skill/prompt 规范
└── pyproject.toml             # Python 包配置
```

## 安装与运行

建议先创建虚拟环境，再安装为 editable 包：

```bash
python -m venv .venv
source .venv/bin/activate
python -m pip install -e .
```

不安装时，也可以临时指定 `PYTHONPATH`：

```bash
PYTHONPATH=src python src/examples/run_query_only.py "你好，介绍一下 OpenAgent"
```

运行本地 scripted demo：

```bash
PYTHONPATH=src python src/examples/run_query_only.py "你好"
```

运行 DashScope demo：

```bash
export DASHSCOPE_API_KEY="你的 Key"
PYTHONPATH=src python src/examples/run_dashscope_universal.py "给我一句软件工程建议"
```

OpenAI-compatible Provider 使用这些环境变量：

- `OPENAI_API_KEY`
- `OPENAI_BASE_URL`
- `OPENAI_HOST_HEADER`
- `OPENAI_MODEL`
- `OPENAI_CONTEXT_WINDOW`
- `OPENAI_MAX_OUTPUT`

DashScope Provider 使用这些环境变量：

- `DASHSCOPE_API_KEY`
- `DASHSCOPE_BASE_URL`
- `DASHSCOPE_MODEL`
- `DASHSCOPE_TEMPERATURE`
- `DASHSCOPE_STREAM`

Web search 默认走 Exa MCP 兼容接口，可配置：

- `OPENAGENT_WEB_SEARCH_EXA_API_KEY`
- `EXA_API_KEY`
- `OPENAGENT_WEB_SEARCH_EXA_MCP_URL`

远程 MCP 配置入口：

- `OPENAGENT_MCP_CONFIG`

## 测试

完整测试命令：

```bash
PYTHONPATH=src python -m unittest discover -s src/tests -p "test_*.py"
```

当前本机验证结果显示，未安装项目依赖时测试会因 `yaml`、`httpx`、`mcp` 等导入失败；同时 `src/tests/test_legacy_cli.py` 引用了仓库内不存在的 `legacy_cli.py`。这些已整理到 [整改清单](doc/remediation-plan.md)。

## 文档索引

- [项目技术文档](doc/openagent-project-doc.md)：按当前代码整理的项目总览、架构、运行链路和扩展说明
- [整改清单](doc/remediation-plan.md)：本次梳理出的文档、依赖、测试和实现整改项
- [Agent 架构分析](Agent.md)：更细的代码对齐版架构分析
- [Remote Sandbox 执行后端设计](doc/remote-sandbox-runtime.md)：本地和 OpenSandbox 工作区 runtime 设计
- [Web 研究收敛设计](doc/web-research-convergence-design.md)：web_search/web_fetch 收敛策略说明

## 当前已知限制

- `AnthropicProvider`、`GeminiProvider`、`OllamaProvider` 仍是 stub。
- `PermissionAction.ASK` 需要宿主注入 `ask_user_func`，否则会表现为工具错误。
- `StorageBase`、`JsonFileStorage` 已存在，但主执行链路尚未接入会话持久化。
- `MemoryAdapter` 目前是进程内字典，不是长期记忆。
- `legacy_cli.py` 测试目标缺失，需要恢复 CLI/Web Console 入口，或移除对应测试。
