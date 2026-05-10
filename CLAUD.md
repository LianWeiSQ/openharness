# OpenAgent 项目深度分析（历史归档）

> 归档说明：本文档保留早期架构分析和设计脉络，但部分内容已经不再代表当前代码事实。
> 当前项目入口请以 `README.md` 为准；当前技术总览请以 `doc/openagent-project-doc.md` 为准；待整改事项请看 `doc/remediation-plan.md`。
>
> 已知过期点包括：源码路径仍按旧的 `openagent/src/openagent/` 描述、Provider 状态仍把 OpenAI/Web/MCP 部分能力写成 stub、依赖描述仍称“核心功能仅依赖标准库”。当前代码实际采用 `src/openagent/` 布局，并已实现 OpenAI-compatible、DashScope、Web 工具、MCP runtime、OpenSandbox runtime 等能力。

## 1. 项目概述

OpenAgent 是一个轻量级的 AI Agent 核心运行时框架，采用 Python 实现。其设计目标是提供一个最小化的、可扩展的 Agent 执行引擎，支持：

- 多种 LLM Provider 的统一接入
- 灵活的工具注册与执行系统
- 可配置的权限管理
- 会话状态管理与快照
- 流式事件输出

**技术特点**：
- 仅依赖 Python 标准库（核心功能）
- 采用 Protocol/ABC 抽象接口设计
- 异步流式处理架构
- 中间件模式扩展

---

## 2. 项目结构

```
openagent/
├── src/openagent/
│   ├── core/                    # 核心模块
│   │   ├── agent/              # Agent 定义
│   │   │   ├── base.py         # BaseAgent 基类
│   │   │   ├── universal.py    # UniversalAgent（主Agent）
│   │   │   ├── plan.py         # PlanAgent（规划Agent）
│   │   │   └── explore.py      # ExploreAgent（只读探索Agent）
│   │   │
│   │   ├── loop/               # 执行循环
│   │   │   ├── processor.py    # AgentLoop 主循环引擎
│   │   │   ├── doom_loop.py    # 循环检测器
│   │   │   ├── snapshot.py     # 文件快照管理
│   │   │   └── retry.py        # 重试管理器
│   │   │
│   │   ├── permission/         # 权限系统
│   │   │   ├── manager.py      # PermissionManager
│   │   │   ├── rule.py         # PermissionRule
│   │   │   └── ruleset.py      # 预定义规则集
│   │   │
│   │   ├── tool/               # 工具系统
│   │   │   ├── toolkit.py      # ToolkitAdapter（工具注册/执行）
│   │   │   ├── middleware.py   # 中间件
│   │   │   ├── registry.py     # RegisteredTool
│   │   │   └── builtin/        # 内置工具
│   │   │       ├── file.py     # 文件操作工具
│   │   │       ├── shell.py    # Shell 命令工具
│   │   │       ├── search.py   # 代码搜索工具
│   │   │       ├── web.py      # Web 工具（stub）
│   │   │       └── memory.py   # 记忆工具
│   │   │
│   │   ├── session/            # 会话管理
│   │   │   ├── session.py      # Session 类
│   │   │   └── storage.py      # 存储后端
│   │   │
│   │   ├── provider/           # LLM Provider
│   │   │   ├── base.py         # LanguageModel Protocol
│   │   │   ├── manager.py      # ProviderManager
│   │   │   ├── anthropic.py    # Anthropic Provider（stub）
│   │   │   ├── openai.py       # OpenAI Provider（stub）
│   │   │   ├── gemini.py       # Gemini Provider（stub）
│   │   │   ├── ollama.py       # Ollama Provider（stub）
│   │   │   └── dashscope.py    # DashScope Provider（已实现）
│   │   │
│   │   ├── types.py            # 核心类型定义
│   │   └── id.py               # ID 生成器
│   │
│   ├── adapter/                # 适配器层
│   │   ├── agent_adapter.py    # AgentAdapter（模型输出转换）
│   │   ├── memory_adapter.py   # MemoryAdapter（键值存储）
│   │   ├── mcp_adapter.py      # MCP 适配器（stub）
│   │   └── toolkit_adapter.py  # ToolkitAdapter（导出 core/tool/toolkit.py）
│   │
│   └── prompts/                # 提示词模板
│
├── tests/                      # 测试
│   ├── _mock_model.py          # Mock 模型
│   ├── test_loop.py            # Loop 测试
│   ├── test_permission.py      # 权限测试
│   └── test_toolkit.py         # 工具测试
│
└── examples/                   # 示例
    ├── run_mock.py             # Mock 模型示例
    ├── run_dashscope_universal.py
```

---

## 3. 核心架构

### 3.1 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                         用户/上层应用                              │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                         AgentLoop                                │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │   Session   │  │ Permission  │  │    SnapshotManager      │  │
│  │   Manager   │  │  Manager    │  │   (文件变更追踪)          │  │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘  │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    Main Loop                              │   │
│  │  1. step-start → 创建快照                                  │   │
│  │  2. 调用 AgentAdapter.reply_stream()                      │   │
│  │  3. 处理 tool-calls → ToolkitAdapter.execute()            │   │
│  │  4. 生成 patch (文件变更)                                   │   │
│  │  5. step-finish → 判断是否继续循环                          │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                                │
            ┌───────────────────┼───────────────────┐
            ▼                   ▼                   ▼
┌───────────────────┐ ┌─────────────────┐ ┌─────────────────────┐
│   AgentAdapter    │ │  ToolkitAdapter  │ │   BaseAgent         │
│                   │ │                  │ │                     │
│ ┌───────────────┐ │ │ ┌─────────────┐ │ │ ┌─────────────────┐ │
│ │ LanguageModel │ │ │ │ Middleware  │ │ │ │  AgentConfig    │ │
│ │   Protocol    │ │ │ │   Chain     │ │ │ │  system_prompt  │ │
│ └───────────────┘ │ │ └─────────────┘ │ │ └─────────────────┘ │
│                   │ │                  │ │                     │
│ 转换 Provider 事件 │ │ ┌─────────────┐ │ │  UniversalAgent    │
│ 为统一 StreamEvent │ │ │   Tools     │ │ │  PlanAgent         │
│                   │ │ │  - read     │ │ │  ExploreAgent      │
│                   │ │ │  - write    │ │ │                     │
│                   │ │ │  - edit     │ │ │                     │
│                   │ │ │  - bash     │ │ │                     │
│                   │ │ │  - glob     │ │ │                     │
│                   │ │ │  - grep     │ │ │                     │
│                   │ │ │  - ...      │ │ │                     │
│                   │ │ └─────────────┘ │ │                     │
└───────────────────┘ └─────────────────┘ └─────────────────────┘
            │
            ▼
┌───────────────────────────────────────────────────────────────┐
│                     Provider Layer                             │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌──────────┐ │
│  │  Anthropic  │ │   OpenAI    │ │   Gemini    │ │ DashScope│ │
│  │   (stub)    │ │   (stub)    │ │   (stub)    │ │ (已实现)  │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └──────────┘ │
└───────────────────────────────────────────────────────────────┘
```

### 3.2 核心类型定义 (`core/types.py`)

```python
# 消息类型
Role = Literal["system", "user", "assistant", "tool"]

# 核心数据结构
@dataclass
class ChatMessage:      # 对话消息
    role: Role
    content: str
    name: str | None
    tool_call_id: str | None

@dataclass
class ToolCall:         # 工具调用请求
    name: str
    input: dict
    call_id: str

@dataclass
class ToolResult:       # 工具执行结果
    call_id: str
    output: str
    error: str | None

@dataclass
class AgentConfig:      # Agent 配置
    name: str
    mode: Literal["primary", "subagent"]
    prompt: str | None
    model: Model | None
    tools: list[str] | Literal["all", "readonly"]
    permission: PermissionRulesetName  # FULL/READONLY/PLAN_ONLY/NONE
    max_steps: int

# 流式事件类型
StreamEvent = TextStartEvent | TextDeltaEvent | TextEndEvent |
              ToolCallEvent | ToolResultEvent |
              StepStartEvent | StepFinishEvent |
              PatchEvent | ErrorEvent
```

---

## 4. 核心流程详解

### 4.1 AgentLoop 执行流程

`AgentLoop.run()` 是整个系统的核心入口，其执行流程如下：

```
┌────────────────────────────────────────────────────────────────┐
│                      AgentLoop.run(user_text)                   │
└────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────────┐
│ 1. 初始化                                                       │
│    - 设置权限规则集 (PermissionRuleset)                          │
│    - 添加用户消息到 Session                                      │
└────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────────┐
│ 2. 主循环 (while steps < max_steps)                             │
│                                                                 │
│    ┌─────────────────────────────────────────────────────────┐ │
│    │ 2.1 step-start                                          │ │
│    │     - 创建文件快照 (SnapshotManager.track)               │ │
│    │     - yield {"type": "step-start", "snapshot_id": ...}  │ │
│    └─────────────────────────────────────────────────────────┘ │
│                          │                                      │
│                          ▼                                      │
│    ┌─────────────────────────────────────────────────────────┐ │
│    │ 2.2 调用模型 (带重试)                                     │ │
│    │     - adapter.reply_stream(system, messages, tools)     │ │
│    │     - 流式 yield 模型事件 (text-delta/tool-call/finish)  │ │
│    │     - 获取 StepInfo (usage, finish_reason, tool_calls)  │ │
│    └─────────────────────────────────────────────────────────┘ │
│                          │                                      │
│                          ▼                                      │
│    ┌─────────────────────────────────────────────────────────┐ │
│    │ 2.3 执行工具调用                                          │ │
│    │     for call in info.tool_calls:                        │ │
│    │       - DoomLoop 检测 (连续相同调用)                      │ │
│    │       - toolkit.execute(name, input, context)           │ │
│    │       - yield {"type": "tool-result", ...}              │ │
│    │       - 将结果写入 Session                                │ │
│    └─────────────────────────────────────────────────────────┘ │
│                          │                                      │
│                          ▼                                      │
│    ┌─────────────────────────────────────────────────────────┐ │
│    │ 2.4 生成 Patch                                           │ │
│    │     - snapshot_manager.patch(snapshot_id)               │ │
│    │     - yield {"type": "patch", "files": [...]}           │ │
│    └─────────────────────────────────────────────────────────┘ │
│                          │                                      │
│                          ▼                                      │
│    ┌─────────────────────────────────────────────────────────┐ │
│    │ 2.5 step-finish                                          │ │
│    │     - yield {"type": "step-finish", "tokens": ...,      │ │
│    │                               "cost": ...,              │ │
│    │                               "finish_reason": ...}     │ │
│    │     - 判断是否继续:                                       │ │
│    │       - blocked → return                                 │ │
│    │       - has tool_calls → continue (让模型处理结果)        │ │
│    │       - finish_reason == "stop" → return                │ │
│    └─────────────────────────────────────────────────────────┘ │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

### 4.2 关键代码分析

#### AgentLoop 核心循环 (`core/loop/processor.py`)

```python
async def run(self, user_text: str) -> AsyncIterator[StreamEvent]:
    # 1. 设置权限
    self.permission_manager.set_ruleset(PermissionRuleset[self.agent.config.permission])

    # 2. 记录用户消息
    self.session.add(ChatMessage(role="user", content=user_text))

    steps = 0
    while steps < self.config.max_steps:
        steps += 1

        # 3. 创建快照
        snapshot_id = self.snapshot_manager.track(Path(self.session.directory))
        yield {"type": "step-start", "snapshot_id": snapshot_id}

        # 4. 调用模型（带重试）
        adapter = self.agent.adapter()
        stream = adapter.reply_stream(...)
        async for ev in stream:
            yield ev  # 透传流事件
        info = await stream.info()

        # 5. 执行工具调用
        for call in info.tool_calls:
            # Doom-loop 检测
            if self.doom_loop_detector.record(call):
                # 检测到循环，触发权限检查
                ...

            # 执行工具
            result = await self.toolkit.execute(
                name=call.name,
                input=call.input,
                context={"session_root": str(self.session.directory), "memory": self.memory}
            )
            yield {"type": "tool-result", ...}

            # 写回会话
            self.session.add(ChatMessage(role="tool", ...))

        # 6. 生成 patch
        patch = self.snapshot_manager.patch(snapshot_id)
        if patch.get("files"):
            yield {"type": "patch", ...}

        # 7. 判断是否继续
        yield {"type": "step-finish", ...}
        if blocked or finish_reason == "stop":
            return
```

---

## 5. 各模块实现分析

### 5.1 Provider 层（模型适配器）

#### LanguageModel Protocol (`core/provider/base.py`)

定义了所有 Provider 必须实现的接口：

```python
class LanguageModel(Protocol):
    async def stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """
        Yield model events:
        - {"type": "text-delta", "id": "...", "text": "..."}
        - {"type": "tool-call", "call_id": "...", "name": "...", "input": {...}}
        - {"type": "finish", "finish_reason": "...", "usage": Usage | dict}
        """
```

#### DashScope Provider 实现示例 (`core/provider/dashscope.py`)

这是目前唯一完整实现的 Provider：

```python
@dataclass
class DashScopeLanguageModel(LanguageModel):
    api_key: str
    model_id: str
    base_url: str = "https://dashscope.aliyuncs.com/compatible-mode/v1"

    async def stream(self, ...):
        # 1. 转换消息格式为 OpenAI 兼容格式
        chat_messages = [...]
        if system:
            chat_messages.append({"role": "system", "content": system})

        # 2. 发起 HTTP 请求（使用 urllib，无第三方依赖）
        data = await asyncio.to_thread(_post_json, ...)

        # 3. 返回流事件
        yield {"type": "text-delta", "id": "dashscope_text", "text": content}
        yield {"type": "finish", "finish_reason": "stop", "usage": u}
```

**待实现 Provider**：
- `anthropic.py` - Anthropic Claude API
- `openai.py` - OpenAI GPT API
- `gemini.py` - Google Gemini API
- `ollama.py` - Ollama 本地模型

### 5.2 Tool 工具系统

#### ToolkitAdapter (`core/tool/toolkit.py`)

工具注册与执行的中枢：

```python
class ToolkitAdapter:
    def __init__(self):
        self._tools: dict[str, RegisteredTool] = {}
        self._middleware: list[Middleware] = []

    def register_tool(self, name, func, description, schema, group, dangerous):
        """注册工具"""
        self._tools[name] = RegisteredTool(schema=..., func=func)

    def register_middleware(self, middleware: Middleware):
        """注册中间件（权限、日志等）"""

    async def execute(self, *, name, input, call_id, context) -> ToolResult:
        """执行工具（通过中间件链）"""

        # 组装"洋葱模型"中间件链
        handler = _invoke  # 最内层：真正执行工具
        for mw in reversed(self._middleware):
            handler = lambda c: mw(c, handler, ctx)

        return await handler(call)
```

#### 内置工具

| 工具名 | 分组 | 危险 | 功能 |
|--------|------|------|------|
| `read` | file | No | 读取文件（支持 offset/limit） |
| `write` | file | Yes | 写入文件（覆盖） |
| `edit` | file | Yes | 编辑文件（字符串替换） |
| `glob` | file | No | 文件名模式匹配 |
| `grep` | file | No | 文件内容搜索 |
| `ls` | file | No | 以树形方式列出目录 |
| `bash` | shell | Yes | 执行 Shell 命令 |
| `code_search` | search | No | 代码搜索 |
| `list_definitions` | search | No | 列出代码定义（stub） |
| `web_fetch` | web | Yes | 获取 URL 内容（stub） |
| `web_search` | web | Yes | 网络搜索（stub） |
| `memory_read` | memory | No | 读取记忆 |
| `memory_write` | memory | No | 写入记忆 |
| `todo` | todo | No | 读取当前 todo 列表 |
| `todowrite` | todo | No | 更新当前 todo 列表 |

#### 中间件系统 (`core/tool/middleware.py`)

```python
# 权限中间件
def permission_middleware(permission_manager) -> Middleware:
    async def _mw(call, nxt, ctx):
        action = await permission_manager.check({"name": call.name, ...})
        if action == PermissionAction.DENY:
            raise PermissionDeniedError(...)
        return await nxt(call)
    return _mw

# 日志中间件
def logging_middleware(logger: list) -> Middleware:
    async def _mw(call, nxt, ctx):
        logger.append({"event": "tool.call", ...})
        result = await nxt(call)
        logger.append({"event": "tool.result", ...})
        return result
    return _mw
```

### 5.3 Permission 权限系统

#### PermissionRule (`core/permission/rule.py`)

```python
class PermissionAction(str, Enum):
    ALLOW = "allow"  # 直接允许
    DENY = "deny"    # 直接拒绝
    ASK = "ask"      # 需要用户确认

@dataclass
class PermissionRule:
    tool: str                           # 工具名（支持通配符）
    action: PermissionAction
    pattern: str | None = None          # 参数匹配模式
    condition: Callable | None = None   # 自定义条件
```

#### 预定义规则集 (`core/permission/ruleset.py`)

| 规则集 | 描述 | 权限 |
|--------|------|------|
| `FULL` | 完全权限 | 允许所有工具 |
| `READONLY` | 只读模式 | 仅允许 read/glob/grep/ls |
| `PLAN_ONLY` | 规划模式 | 读操作允许，其他需要确认 |
| `NONE` | 无权限 | 拒绝所有工具 |

#### PermissionManager (`core/permission/manager.py`)

```python
class PermissionManager:
    def _evaluate(self, tool: str, pattern: str) -> PermissionRule | None:
        """规则匹配（last match wins）"""
        for rule in self._rules:
            if fnmatch.fnmatch(tool, rule.tool) and
               fnmatch.fnmatch(pattern, rule.pattern):
                match = rule
        return match

    async def check(self, tool_call: dict) -> PermissionAction:
        """检查权限"""
        rule = self._evaluate(tool, pattern)
        if rule.action == PermissionAction.ASK:
            return await self.ask_user(tool_call)
        return rule.action
```

### 5.4 Session 会话管理

#### Session (`core/session/session.py`)

```python
@dataclass
class Session:
    id: str
    directory: Path              # 工作目录
    status: SessionStatus        # IDLE/RUNNING/PAUSED/STOP/COMPACTING
    messages: list[ChatMessage]  # 对话历史
    metadata: dict

    def add(self, message: ChatMessage):
        """添加消息"""

    def fork(self, *, at: int | None = None) -> "Session":
        """分支会话（从指定位置复制消息）"""

    def revert(self, *, to: int):
        """回滚会话（删除指定位置之后的消息）"""
```

#### Storage (`core/session/storage.py`)

```python
class InMemoryStorage(StorageBase):
    """内存存储"""

class JsonFileStorage(StorageBase):
    """JSON 文件存储"""
    def read(self, key: str) -> dict | None
    def write(self, key: str, value: dict)
```

### 5.5 Loop 辅助模块

#### DoomLoopDetector (`core/loop/doom_loop.py`)

检测连续相同的工具调用（防止死循环）：

```python
class DoomLoopDetector:
    def __init__(self, threshold: int = 3):
        self._history: deque[str] = deque(maxlen=threshold)

    def record(self, call: ToolCall) -> bool:
        """记录调用，返回是否检测到循环"""
        self._history.append(call.key())  # "name:json(input)"
        return all(x == self._history[0] for x in self._history)
```

#### SnapshotManager (`core/loop/snapshot.py`)

文件快照管理，用于追踪文件变更：

```python
class SnapshotManager:
    def track(self, root: Path) -> str:
        """创建快照，返回 snapshot_id"""
        file_map = self._scan(root)  # 扫描所有文件，计算 SHA256
        self._snapshots[snap_id] = Snapshot(...)
        return snap_id

    def patch(self, snapshot_id: str) -> dict:
        """计算快照与当前状态的差异"""
        before = self._snapshots[snapshot_id].files
        now = self._scan(root)
        # 比较 before/now，生成 diff
        return {"hash": ..., "files": [{"path", "status", "diff"}]}
```

#### RetryManager (`core/loop/retry.py`)

指数退避重试：

```python
class RetryManager:
    async def run(self, func: Callable[[], Awaitable]) -> object:
        while True:
            try:
                return await func()
            except Exception:
                attempt += 1
                if attempt > max_retry:
                    raise
                await asyncio.sleep(base_delay * (2 ** (attempt - 1)))
```

### 5.6 Adapter 适配器层

#### AgentAdapter (`adapter/agent_adapter.py`)

将 Provider 的 LanguageModel 转换为统一的流事件：

```python
class AgentAdapter:
    def reply_stream(self, *, system, messages, tools) -> AgentReplyStream:
        async def _gen():
            async for ev in self._model.stream(...):
                if ev["type"] == "text-delta":
                    yield {"type": "text-start", ...}
                    yield {"type": "text-delta", ...}
                elif ev["type"] == "tool-call":
                    tool_calls.append(call)
                    yield {"type": "tool-call", ...}
                elif ev["type"] == "finish":
                    finish_reason = ev["finish_reason"]
                    usage = coerce_usage(ev["usage"])

            yield {"type": "text-end", ...}
            info_future.set_result(StepInfo(...))

        return AgentReplyStream(_gen(), info_future)
```

## 6. 数据流图

### 6.1 完整执行流程

```
User Input
    │
    ▼
┌─────────────┐
│   Session   │ ◄─── 添加用户消息
└─────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│                      AgentLoop                              │
│                                                             │
│  ┌──────────┐    ┌──────────────┐    ┌────────────────┐    │
│  │ Snapshot │    │ AgentAdapter │    │ ToolkitAdapter │    │
│  │  Track   │───►│ reply_stream │───►│    execute     │    │
│  └──────────┘    └──────────────┘    └────────────────┘    │
│        │                │                    │              │
│        │                ▼                    ▼              │
│        │         ┌─────────────┐      ┌───────────┐        │
│        │         │ StreamEvent │      │ToolResult │        │
│        │         │  (yield)    │      │           │        │
│        │         └─────────────┘      └───────────┘        │
│        │                                     │              │
│        ▼                                     ▼              │
│  ┌──────────┐                         ┌───────────┐        │
│  │  Patch   │                         │  Session  │        │
│  │ (diff)   │                         │  (add)    │        │
│  └──────────┘                         └───────────┘        │
│                                                             │
└─────────────────────────────────────────────────────────────┘
    │
    ▼
StreamEvent Iterator (yield to caller)
```

### 6.2 工具执行流程

```
ToolCall
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│                    ToolkitAdapter.execute                   │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Middleware Chain (洋葱模型)              │   │
│  │                                                     │   │
│  │   permission_middleware ──► logging_middleware ──►  │   │
│  │                          │                          │   │
│  │                          ▼                          │   │
│  │                    ┌─────────┐                      │   │
│  │                    │  Tool   │                      │   │
│  │                    │  Func   │                      │   │
│  │                    └─────────┘                      │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
    │
    ▼
ToolResult
```

---

## 7. 待实现功能

### 7.1 Provider 实现（高优先级）

| Provider | 状态 | 说明 |
|----------|------|------|
| Anthropic | Stub | 需要实现 Claude API 调用 |
| OpenAI | Stub | 需要实现 GPT API 调用 |
| Gemini | Stub | 需要实现 Gemini API 调用 |
| Ollama | Stub | 需要实现本地模型调用 |

### 7.2 工具增强（中优先级）

| 工具 | 状态 | 说明 |
|------|------|------|
| `web_fetch` | Stub | 需要实现 HTTP 请求 |
| `web_search` | Stub | 需要集成搜索 API |
| `list_definitions` | Stub | 需要实现代码解析 |

### 7.3 MCP 集成（中优先级）

`mcp_adapter.py` 当前仅为占位符：

```python
class MCPClientBase:
    """Placeholder for MCP integration."""
    pass
```

`toolkit.py` 中的 `register_mcp` 方法：

```python
def register_mcp(self, client: object, group: str = "mcp") -> None:
    raise NotImplementedError("MCP integration is not implemented yet")
```

### 7.4 功能增强（低优先级）

1. **流式响应优化**
   - DashScope Provider 目前是非真正流式（一次性返回）
   - 应改为真正的 SSE 流式处理

2. **Token 计费**
   - AgentScope adapter 未做 token 统计
   - 应在各 Provider 中实现准确的 token 计数

3. **会话持久化**
   - 当前 Session 仅在内存中
   - 应实现完整的会话存储/恢复

4. **并发控制**
   - 多工具并发执行的调度
   - 资源限制和优先级管理

5. **错误恢复**
   - 更细粒度的错误分类
   - 自动恢复策略

6. **日志与观测**
   - 结构化日志
   - OpenTelemetry 集成
   - 性能指标收集

### 7.5 架构改进

1. **类型安全**
   - 使用 pydantic 替代 dataclass（可选）
   - 更严格的类型检查

2. **配置管理**
   - 支持配置文件
   - 环境变量规范

3. **测试覆盖**
   - 更多单元测试
   - 集成测试
   - 性能测试

---

## 8. 使用示例

### 8.1 最小示例（Mock 模型）

```python
import asyncio
from pathlib import Path
from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig

class ScriptedModel:
    async def stream(self, *, system, messages, tools, **kwargs):
        yield {"type": "tool-call", "call_id": "c1", "name": "write",
               "input": {"file_path": "hello.txt", "content": "hello"}}
        yield {"type": "finish", "finish_reason": "tool_call", "usage": {}}

async def main():
    agent = UniversalAgent(
        config=AgentConfig(name="universal", permission="FULL", max_steps=5),
        model=ScriptedModel(),
        system_prompt="",
    )
    loop = AgentLoop(
        agent=agent,
        session=Session(directory=Path("workdir")),
        permission_manager=PermissionManager()
    )
    async for event in loop.run("create a file"):
        print(event)

asyncio.run(main())
```

### 8.2 DashScope 示例

```python
import os
os.environ["DASHSCOPE_API_KEY"] = "your-api-key"

from openagent.core.provider.dashscope import DashScopeProvider, DashScopeLanguageModel
from openagent.core.types import Model

provider = DashScopeProvider()
model = Model(id="qwen-plus", provider_id="dashscope", name="Qwen Plus",
              context_window=32768, max_output=4096)
lm = await provider.get_language_model(model)

async for ev in lm.stream(system="You are helpful.", messages=[...], tools=[]):
    print(ev)
```

---

## 9. 设计原则总结

1. **最小依赖**：核心功能仅使用 Python 标准库
2. **Protocol 优先**：使用 Protocol/ABC 定义接口，便于替换实现
3. **流式处理**：所有 I/O 操作采用异步流式设计
4. **中间件模式**：工具执行支持可插拔的中间件链
5. **事件驱动**：通过 StreamEvent 统一输出格式
6. **权限可控**：细粒度的权限规则系统

---

## 10. 文件索引

| 文件路径 | 核心职责 |
|----------|----------|
| `core/types.py` | 核心类型定义 |
| `core/loop/processor.py` | AgentLoop 主循环 |
| `core/loop/doom_loop.py` | 循环检测 |
| `core/loop/snapshot.py` | 文件快照 |
| `core/loop/retry.py` | 重试管理 |
| `core/agent/base.py` | Agent 基类 |
| `core/agent/universal.py` | 通用 Agent |
| `core/provider/base.py` | Provider Protocol |
| `core/provider/dashscope.py` | DashScope 实现 |
| `core/tool/toolkit.py` | 工具注册执行 |
| `core/tool/middleware.py` | 工具中间件 |
| `core/permission/manager.py` | 权限管理 |
| `core/permission/ruleset.py` | 权限规则集 |
| `core/session/session.py` | 会话管理 |
| `adapter/agent_adapter.py` | Agent 适配器 |
