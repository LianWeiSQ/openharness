# OpenAgent Core 设计文档

> Adapter架构：上层暴露通用智能体接口，底层调用AgentScope SDK

---

## 1. 架构概览

### 1.1 分层架构

```
┌─────────────────────────────────────────────────────────────────┐
│                        Application Layer                         │
│   UniversalAgent    PlanAgent    ExploreAgent    CustomAgent    │
│      (build)        (plan)        (explore)       (custom)      │
└─────────────────────────────┬───────────────────────────────────┘
                              │
┌─────────────────────────────┴───────────────────────────────────┐
│                          Loop Layer                              │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                      AgentLoop                               ││
│  │   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   ││
│  │   │ DoomLoop │  │  Retry   │  │ Snapshot │  │  Step    │   ││
│  │   │ Detector │  │ Manager  │  │ Manager  │  │ Tracker  │   ││
│  │   └──────────┘  └──────────┘  └──────────┘  └──────────┘   ││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────┬───────────────────────────────────┘
                              │
┌─────────────────────────────┴───────────────────────────────────┐
│                         Adapter Layer                            │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌─────────┐│
│  │ AgentAdapter │ │ToolkitAdapter│ │SessionAdapter│ │Provider ││
│  │              │ │              │ │              │ │ Adapter ││
│  └──────────────┘ └──────────────┘ └──────────────┘ └─────────┘│
│  ┌──────────────────────────────────────────────────────────────┐│
│  │                    PermissionManager                          ││
│  └──────────────────────────────────────────────────────────────┘│
└─────────────────────────────┬───────────────────────────────────┘
                              │
┌─────────────────────────────┴───────────────────────────────────┐
│                       SDK Layer (AgentScope)                     │
│    ReActAgent      Toolkit       Memory       SessionBase       │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 模块依赖关系

```
UniversalAgent ─────┬──> AgentAdapter ───> ReActAgent
                    │         │
                    │         v
                    ├──> ToolkitAdapter ──> Toolkit
                    │         │
                    │         v
                    ├──> PermissionManager
                    │
                    v
              AgentLoop ───> SessionAdapter ───> Memory
                    │
                    ├──> DoomLoopDetector
                    ├──> RetryManager
                    └──> SnapshotManager
```

---

## 2. Agent Loop 核心流程

### 2.1 主循环

```
                    ┌─────────────────┐
                    │  接收用户消息    │
                    └────────┬────────┘
                             │
                             v
              ┌──────────────────────────────┐
              │         while (true)          │
              │  ┌─────────────────────────┐  │
              │  │  1. 创建文件快照         │  │
              │  │  2. 调用 AgentAdapter    │  │
              │  │     .reply_stream()      │  │
              │  │  3. 处理流事件:          │  │
              │  │     - text-delta → 输出  │  │
              │  │     - tool-call → 执行   │  │
              │  │     - tool-result → 结果 │  │
              │  │     - error → 重试/停止  │  │
              │  │  4. 计算文件变更patch    │  │
              │  │  5. 检查循环控制:        │  │
              │  │     - 需要压缩? → break  │  │
              │  │     - 被阻止? → break    │  │
              │  │     - 达到最大步数? break│  │
              │  └─────────────────────────┘  │
              └──────────────────────────────┘
                             │
                             v
                    ┌─────────────────┐
                    │     完成/停止    │
                    └─────────────────┘
```

### 2.2 流事件类型

| 事件 | 触发时机 | 数据 |
|------|----------|------|
| `text-start` | 开始输出文本 | id, metadata |
| `text-delta` | 文本片段 | id, text |
| `text-end` | 结束输出文本 | id |
| `tool-call` | 调用工具 | name, input, call_id |
| `tool-result` | 工具返回 | call_id, output, error |
| `step-start` | 步骤开始 | snapshot_id |
| `step-finish` | 步骤结束 | tokens, cost, finish_reason |
| `error` | 发生错误 | error |

### 2.3 循环控制参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `max_steps` | 50 | 最大执行步数 |
| `doom_loop_threshold` | 3 | 连续相同调用触发死循环检测 |
| `max_retry` | 2 | API错误最大重试次数 |
| `retry_base_delay` | 1.0s | 重试基础延迟（指数退避） |

---

## 3. Provider 模块

### 3.1 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     ProviderManager                          │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  get_model(provider_id, model_id) → Model               ││
│  │  get_language(model) → LanguageModel                    ││
│  │  list_providers() → List[ProviderInfo]                  ││
│  │  default_model() → Model                                ││
│  └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        │                     │                     │
        v                     v                     v
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│ OpenAIProvider│   │ AnthropicProvider│ │ GeminiProvider│
│               │   │                 │ │               │
│ - gpt-4       │   │ - claude-3-opus │ │ - gemini-pro  │
│ - gpt-3.5     │   │ - claude-3-sonnet│ │ - gemini-flash│
└───────────────┘   └───────────────┘   └───────────────┘
```

### 3.2 Provider 接口

```python
@dataclass
class Model:
    id: str                      # 模型ID
    provider_id: str             # 提供商ID
    name: str                    # 显示名称
    context_window: int          # 上下文窗口
    max_output: int              # 最大输出token
    capabilities: ModelCapabilities
    pricing: ModelPricing

@dataclass
class ModelCapabilities:
    vision: bool = False         # 支持图像
    tools: bool = True           # 支持工具调用
    streaming: bool = True       # 支持流式
    reasoning: bool = False      # 支持扩展思考

class ProviderBase(ABC):
    @abstractmethod
    async def get_language_model(self, model: Model) -> LanguageModel:
        """获取语言模型实例"""

    @abstractmethod
    async def list_models(self) -> list[Model]:
        """列出可用模型"""

    @abstractmethod
    def get_model_config(self, model: Model) -> dict:
        """获取模型配置"""
```

### 3.3 内置 Provider

| Provider | ID | 模型 | 特性 |
|----------|-----|------|------|
| OpenAI | `openai` | gpt-4, gpt-4o, gpt-3.5-turbo | 标准工具调用 |
| Anthropic | `anthropic` | claude-3-opus, claude-3-sonnet | 扩展思考 |
| Gemini | `google` | gemini-pro, gemini-flash | 多模态 |
| DashScope | `dashscope` | qwen-max, qwen-plus | 阿里云 |
| Ollama | `ollama` | llama3, mistral | 本地部署 |

---

## 4. Permission 模块

### 4.1 权限模型

```
┌─────────────────────────────────────────────────────────────┐
│                    PermissionManager                         │
│                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │  Ruleset    │  │   Rules     │  │   Check Flow        │  │
│  │             │  │             │  │                     │  │
│  │ - FULL      │  │ tool: "*"   │  │ 1. 查找匹配规则     │  │
│  │ - READONLY  │  │ action:     │  │ 2. 检查危险工具     │  │
│  │ - PLAN_ONLY │  │   allow/    │  │ 3. 返回决策         │  │
│  │ - NONE      │  │   deny/ask  │  │                     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 权限规则集

| Ruleset | 允许的工具 | 拒绝的工具 | 需确认的工具 |
|---------|-----------|-----------|-------------|
| `FULL` | 全部 | - | 危险操作(bash,write) |
| `READONLY` | read, glob, grep, ls | write, edit, bash | - |
| `PLAN_ONLY` | read, write(仅.plan文件) | 其他全部 | - |
| `NONE` | - | 全部 | - |

### 4.3 权限检查流程

```
┌─────────────────┐
│  工具调用请求    │
└────────┬────────┘
         │
         v
┌─────────────────┐     是      ┌─────────────────┐
│ 工具在危险列表?  │──────────> │ 查找显式允许规则 │
└────────┬────────┘             └────────┬────────┘
         │ 否                            │
         v                               │
┌─────────────────┐                      │
│  按顺序匹配规则  │<─────────────────────┘
└────────┬────────┘
         │
         v
┌─────────────────┐
│  返回决策结果    │
│  allow/deny/ask │
└─────────────────┘
```

### 4.4 权限接口

```python
class PermissionAction(str, Enum):
    ALLOW = "allow"    # 直接允许
    DENY = "deny"      # 直接拒绝
    ASK = "ask"        # 询问用户

@dataclass
class PermissionRule:
    tool: str                    # 工具名 (支持通配符 *)
    action: PermissionAction     # 动作
    pattern: str | None = None   # 参数模式匹配
    condition: Callable | None = None  # 条件函数

class PermissionManager:
    def set_ruleset(self, ruleset: PermissionRuleset) -> None:
        """应用预定义规则集"""

    def add_rule(self, rule: PermissionRule) -> None:
        """添加自定义规则"""

    async def check(self, tool_call: dict) -> PermissionAction:
        """检查权限"""

    async def ask_user(self, tool_call: dict) -> PermissionAction:
        """询问用户确认"""
```

### 4.5 危险工具列表

| 工具 | 风险等级 | 说明 |
|------|----------|------|
| `bash` | 高 | 执行任意shell命令 |
| `write` | 高 | 写入文件 |
| `edit` | 高 | 编辑文件 |
| `delete` | 极高 | 删除文件 |
| `web_fetch` | 中 | 访问外部URL |

---

## 5. Tool 模块

### 5.1 工具架构

```
┌─────────────────────────────────────────────────────────────┐
│                      ToolkitAdapter                          │
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐│
│  │                    Toolkit (AgentScope)                  ││
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐   ││
│  │  │ Built-in │ │   MCP    │ │ Function │ │  Group   │   ││
│  │  │  Tools   │ │  Tools   │ │  Tools   │ │  Tools   │   ││
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘   ││
│  └─────────────────────────────────────────────────────────┘│
│                              │                               │
│  ┌───────────────────────────┴───────────────────────────┐  │
│  │                    Middleware Chain                    │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐  │  │
│  │  │Permission│ │  Logging │ │  Retry   │ │  Custom  │  │  │
│  │  │  Check   │ │          │ │          │ │          │  │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 工具分类

| 类别 | 工具 | 说明 |
|------|------|------|
| **文件操作** | `read`, `write`, `edit`, `glob`, `grep` | 文件读写和搜索 |
| **Shell** | `bash` | 执行shell命令 |
| **网络** | `web_fetch`, `web_search` | 网络请求和搜索 |
| **代码** | `code_search`, `list_definitions` | 代码分析 |
| **记忆** | `memory_read`, `memory_write` | 记忆管理 |
| **用户交互** | `ask`, `question` | 用户确认和问答 |

### 5.3 工具注册

```python
class ToolkitAdapter:
    def register_tool(
        self,
        name: str,
        func: Callable,
        description: str = "",
        schema: dict | None = None,
        group: str = "default",
        dangerous: bool = False,
    ) -> None:
        """注册工具"""

    def register_mcp(self, client: MCPClientBase, group: str = "mcp") -> None:
        """注册MCP工具"""

    def get_tools_by_group(self, groups: list[str]) -> list[ToolSchema]:
        """按组获取工具"""

    def register_middleware(self, middleware: MiddlewareFunc) -> None:
        """注册中间件"""
```

### 5.4 中间件链

```
工具调用请求
     │
     v
┌─────────────┐
│ Permission  │ ──deny──> 返回错误
│  中间件     │
└──────┬──────┘
       │allow
       v
┌─────────────┐
│  Logging    │ ──记录调用
│  中间件     │
└──────┬──────┘
       │
       v
┌─────────────┐
│   Retry     │ ──失败重试
│  中间件     │
└──────┬──────┘
       │
       v
┌─────────────┐
│   工具执行   │
└─────────────┘
       │
       v
   返回结果
```

### 5.5 内置工具详情

#### 文件工具

```python
# read - 读取文件
{
    "name": "read",
    "parameters": {
        "file_path": "string (required)",
        "offset": "integer (optional)",
        "limit": "integer (optional)"
    }
}

# write - 写入文件
{
    "name": "write",
    "parameters": {
        "file_path": "string (required)",
        "content": "string (required)"
    }
}

# edit - 编辑文件
{
    "name": "edit",
    "parameters": {
        "file_path": "string (required)",
        "old_string": "string (required)",
        "new_string": "string (required)"
    }
}

# glob - 文件模式匹配
{
    "name": "glob",
    "parameters": {
        "pattern": "string (required)",
        "path": "string (optional)"
    }
}

# grep - 内容搜索
{
    "name": "grep",
    "parameters": {
        "pattern": "string (required)",
        "path": "string (optional)",
        "glob": "string (optional)"
    }
}
```

#### Shell工具

```python
# bash - 执行命令
{
    "name": "bash",
    "parameters": {
        "command": "string (required)",
        "timeout": "integer (optional, default=120000)",
        "dangerously_disable_sandbox": "boolean (optional)"
    }
}
```

---

## 6. Agent 类型

### 6.1 类型对比

| 类型 | 模式 | 权限 | 工具 | 使用场景 |
|------|------|------|------|----------|
| `UniversalAgent` | primary | FULL | 全部 | 代码编写、重构、调试 |
| `PlanAgent` | primary | PLAN_ONLY | read + plan写入 | 需求分析、架构设计 |
| `ExploreAgent` | subagent | READONLY | grep/glob/read/ls | 快速代码探索 |
| `CompactionAgent` | internal | NONE | 无 | 上下文压缩 |
| `TitleAgent` | internal | NONE | 无 | 生成会话标题 |

### 6.2 Agent 配置

```python
@dataclass
class AgentConfig:
    name: str                              # Agent名称
    mode: Literal["primary", "subagent"]   # 运行模式
    prompt: str | None = None              # 系统提示词
    model: Model | None = None             # 指定模型
    tools: list[str] | Literal["all", "readonly"] = "all"
    permission: PermissionRuleset = PermissionRuleset.FULL
    max_steps: int = 50                    # 最大步数
    temperature: float | None = None       # 温度
    options: dict[str, Any] = field(default_factory=dict)
```

---

## 7. Session 模块

### 7.1 会话状态机

```
    ┌───────┐
    │ IDLE  │◄────────────────────────────┐
    └───┬───┘                             │
        │ 开始执行                         │
        v                                 │
    ┌───────┐                             │
    │RUNNING│◄──────┐                     │
    └───┬───┘       │                     │
        │ 暂停      │ 恢复                │
        v       ┌───┴───┐                 │
    ┌───────┐   │PAUSED │                 │
    │PAUSED │──>└───────┘                 │
    └───────┘                             │
        │                                 │
        │ 完成/停止/中断                   │
        v                                 │
    ┌───────┐                             │
    │ STOP  │─────────────────────────────┘
    └───────┘         新消息
        │
        │ 需要压缩
        v
    ┌──────────┐
    │COMPACTING│──> IDLE
    └──────────┘
```

### 7.2 Fork/Revert 机制

```
原会话                          Fork新会话
┌──────────────────┐           ┌──────────────────┐
│ msg_1            │──────────>│ msg_1 (copy)     │
│ msg_2            │           │ msg_2 (copy)     │
│ msg_3 (fork点)   │           │                  │
│ msg_4            │           │ 新消息从这里开始  │
│ msg_5            │           │                  │
└──────────────────┘           └──────────────────┘

Revert回滚:
┌──────────────────┐           ┌──────────────────┐
│ msg_1            │           │ msg_1            │
│ msg_2            │           │ msg_2            │
│ msg_3 (revert点) │──保存快照──>│ msg_3            │
│ msg_4            │  删除后续   │                  │
│ msg_5            │           │                  │
└──────────────────┘           └──────────────────┘
```

---

## 8. 文件结构

```
openagent/src/openagent/
├── core/
│   ├── loop/                     # Agent Loop
│   │   ├── processor.py          # AgentLoop
│   │   ├── doom_loop.py          # 死循环检测
│   │   ├── retry.py              # 重试管理
│   │   └── snapshot.py           # 快照管理
│   ├── agent/
│   │   ├── base.py               # BaseAgent
│   │   ├── universal.py          # UniversalAgent
│   │   ├── plan.py               # PlanAgent
│   │   └── explore.py            # ExploreAgent
│   ├── provider/                 # ★ Provider模块
│   │   ├── base.py               # ProviderBase
│   │   ├── manager.py            # ProviderManager
│   │   ├── openai.py             # OpenAI Provider
│   │   ├── anthropic.py          # Anthropic Provider
│   │   ├── gemini.py             # Gemini Provider
│   │   ├── dashscope.py          # DashScope Provider
│   │   └── ollama.py             # Ollama Provider
│   ├── permission/               # ★ Permission模块
│   │   ├── manager.py            # PermissionManager
│   │   ├── ruleset.py            # 预定义规则集
│   │   └── rule.py               # 权限规则
│   ├── tool/                     # ★ Tool模块
│   │   ├── toolkit.py            # ToolkitAdapter
│   │   ├── registry.py           # 工具注册表
│   │   ├── middleware.py         # 中间件
│   │   └── builtin/              # 内置工具
│   │       ├── file.py           # 文件工具
│   │       ├── shell.py          # Shell工具
│   │       ├── memory.py         # 记忆工具（memory_read/memory_write）
│   │       ├── web.py            # 网络工具
│   │       └── search.py         # 搜索工具
│   ├── session/
│   │   ├── session.py            # SessionAdapter
│   │   └── storage.py            # 存储后端
│   └── types.py                  # 核心类型
├── adapter/
│   ├── agent_adapter.py
│   ├── toolkit_adapter.py
│   ├── memory_adapter.py
│   └── mcp_adapter.py
└── prompts/
    ├── build.txt
    ├── plan.txt
    └── explore.txt
```

---

## 9. 实现路线

### Phase 1: 核心基础设施
- [x] Provider模块 (ProviderBase, ProviderManager)
- [x] Permission模块 (PermissionManager, Rulesets)
- [x] Tool模块基础 (ToolkitAdapter, 中间件)

### Phase 2: Agent Loop
- [x] AgentLoop处理器
- [x] DoomLoopDetector
- [x] RetryManager
- [x] SnapshotManager

### Phase 3: Agent封装
- [x] BaseAgent
- [x] UniversalAgent
- [x] PlanAgent
- [x] ExploreAgent

### Phase 4: 内置工具
- [x] 文件工具 (read, write, edit, glob, grep, ls)
- [x] Shell工具 (bash)
- [ ] 网络工具 (web_fetch, web_search)（默认 stub，按需接入网络实现）
- [x] 记忆工具 (memory_read, memory_write)
- [x] 搜索工具 (code_search)

---

## 11. Python 依赖与安装（实现说明）

> 说明：本仓库 `openagent/` 的 Python 实现以 **标准库为主**，Provider 对接（OpenAI/Anthropic/Gemini/DashScope/Ollama）目前为 stub，需要你在对应 Provider 中接入真实 SDK。

### 11.1 推荐安装方式

项目使用 `src/` 布局，建议在仓库根目录执行：

```bash
pip install -e openagent
```

或者临时运行（不安装）：

```bash
PYTHONPATH=openagent/src python openagent/examples/run_mock.py
```

在仓库根目录也可直接运行（通过 `openagent/__init__.py` bootstrap 自动补齐 `src` 路径）：

```bash
python openagent/examples/run_mock.py
```

### 11.2 DashScope（阿里云）用例 Demo

实现文件：
- `openagent/src/openagent/core/provider/dashscope.py`：DashScope Provider + LanguageModel 适配器（兼容模式）
- `openagent/examples/run_dashscope_universal.py`：UniversalAgent 问答 demo

准备环境变量：

```bash
export DASHSCOPE_API_KEY="你的Key"
export DASHSCOPE_MODEL="qwen-plus"   # 可选，默认 qwen-plus
export DASHSCOPE_TEMPERATURE="0.2"   # 可选
```

运行：

```bash
python openagent/examples/run_dashscope_universal.py "给我一句关于软件工程的建议"
```

说明：
- demo 走 DashScope 的 OpenAI 兼容接口（`compatible-mode/v1/chat/completions`）
- demo 默认不向模型传 tools，避免模型输出 tool-call 造成额外依赖/权限交互

### 11.3 AgentScope 外壳（ReActAgent + DashScope/Qwen）

定位：
- OpenAgent 继续负责 `AgentLoop`（step-start/patch/step-finish 等语义不变）
- AgentScope 负责底层推理与工具执行（ReActAgent）
- 对外输出仍是 OpenAgent 的 `StreamEvent`（text/tool/patch/error）

安装（可选依赖，不影响现有功能）：

```bash
pip install -e "openagent[agentscope]"
```

准备环境变量：

```bash
export DASHSCOPE_API_KEY="你的Key"
export DASHSCOPE_MODEL="qwen-plus"   # 可选
```

运行 demo：

```bash
python openagent/examples/run_agentscope_universal.py "请解释一下什么是幂等性？"
```

实现要点（避免工具二次执行）：
- 工具执行发生在 AgentScope 内部（ReActAgent 调用工具）
- 但工具实现复用 OpenAgent 的 `ToolkitAdapter`（包含 Permission middleware）
- adapter 会把 tool-call/tool-result 直接翻译成 StreamEvent 输出
- `StepInfo.tool_calls` 置空，使 `AgentLoop` 不再执行工具（防止双执行）


---

## 10. 参考

| 项目 | 文件 | 说明 |
|------|------|------|
| OpenCode | `session/processor.ts` | Agent Loop核心 |
| OpenCode | `provider/provider.ts` | Provider管理 |
| OpenCode | `permission/next.ts` | 权限系统 |
| OpenCode | `tool/*.ts` | 工具实现 |
| AgentScope | `agent/_agent_base.py` | Agent基类 |
| AgentScope | `tool/_toolkit.py` | Toolkit实现 |
| AgentScope | `model/*.py` | Model实现 |
