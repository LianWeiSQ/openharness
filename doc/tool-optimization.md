# Tool Optimization

这份文档维护 OpenAgent 的工具优化专项，目标是对标 Claude Code 的成熟工具运行时：工具语义清晰、可并发调度、可观测、可回放，并且能在复杂任务中减少无效等待。

## Why

OpenAgent 之前只有模型可见的 tool 参数 schema，以及权限语义上的 `dangerous` 标记。这还不够，因为：

- `dangerous` 只能说明权限风险，不能说明是否可以并发；
- `read/grep/ls` 这类只读工具适合并发；
- `write/edit/bash/question/todowrite` 这类工具会改 workspace、session 或等待用户，需要串行；
- Langfuse/trace 里需要看清楚一个 step 内的工具是如何被调度的。

## P0 Current Slice

已落地：

- 新增 runtime-only `ToolExecutionSchema`。
- 给内置工具完成第一轮分类。
- 新增 `ToolBatchPlanner`，按模型返回顺序把 tool calls 切成 batch。
- `ToolkitAdapter.plan_tool_batches(...)` 暴露 planner 入口。
- MCP 动态工具默认标记为 `unknown`，先串行。

本轮暂未改 AgentLoop 的实际执行器。也就是说，当前代码已经能“规划 batch”，但 loop 仍沿用原来的串行执行路径。这样做是为了先把分类和 planner 做成可测试底座，避免一次性改动 question、permission、session store 和 trace 写入。

## Execution Schema

字段含义：

| Field | Meaning |
| --- | --- |
| `read_only` | 工具主体是否只读 |
| `mutates_workspace` | 是否修改 workspace 文件或命令环境 |
| `mutates_session` | 是否修改 session/todo/memory/read-cache |
| `mutates_external` | 是否可能修改外部系统 |
| `external_io` | 是否访问网络或外部服务 |
| `requires_user_interaction` | 是否会等待用户输入 |
| `concurrency` | `safe` / `exclusive` / `keyed` / `unknown` |
| `batch_group` | 调度分组，例如 `workspace-read`、`web`、`shell` |
| `conflict_key_template` | 未来 keyed concurrency 的冲突键模板 |
| `max_parallelism` | 同一 batch 的最大并发度 |

## Builtin Tool Classification

| Tool | Group | Concurrency | Notes |
| --- | --- | --- | --- |
| `read` | `workspace-read` | `safe` | 读文件，但会更新 file read cache |
| `glob` | `workspace-read` | `safe` | workspace 只读检索 |
| `grep` | `workspace-read` | `safe` | workspace 只读检索 |
| `ls` | `workspace-read` | `safe` | workspace 只读目录遍历 |
| `code_search` | `workspace-read` | `safe` | host-only 代码搜索 |
| `skill` | `skill` | `safe` | skill discovery / skill read |
| `memory_read` | `memory` | `safe` | 读取 memory adapter |
| `memory_write` | `memory` | `exclusive` | 写 memory，未来可按 key 做 keyed concurrency |
| `todoread` | `todo` | `safe` | 读取 session todo |
| `todowrite` | `todo` | `exclusive` | 覆盖 session todo |
| `web_fetch` | `web` | `safe` | 外部 I/O，当前 max parallelism = 4 |
| `web_search` | `web` | `safe` | 外部 I/O，当前 max parallelism = 3 |
| `web_scrape` | `web` | `safe` | 较重外部 I/O，当前 max parallelism = 2 |
| `write` | `workspace-write` | `exclusive` | 写文件 |
| `edit` | `workspace-write` | `exclusive` | 改文件 |
| `bash` | `shell` | `exclusive` | 可能读写 workspace、网络和外部状态 |
| `question` | `interactive` | `exclusive` | 等待用户输入 |
| MCP tools | `mcp` | `unknown` | 动态工具，除非 descriptor 后续提供语义 |

## Planner Rule

当前 planner 规则：

```text
model tool calls, preserving order
        ↓
safe tools: group consecutive calls into one batch
        ↓
exclusive / keyed / unknown tools: singleton serial batch
        ↓
safe batch respects the lowest max_parallelism among its items
```

示例：

```text
read, grep, bash, ls
        ↓
[read, grep] concurrent
[bash] serial
[ls] serial
```

## Next Slices

P1：把 `ToolBatchPlanner` 接入 AgentLoop trace，但仍串行执行。

- 增加 `tool.batch.started` / `tool.batch.finished`。
- 在 Langfuse 中展示 batch、tool call、step 的层级关系。
- 验证不改变现有行为。

P2：实现 read-only batch 的真实并发执行。

- 只对 `concurrency=safe` 的 batch 并发。
- tool result 写回 session 时仍保持模型原始顺序。
- question request、permission、doom-loop、tool failure 语义保持兼容。

P3：keyed concurrency。

- `write/edit` 按文件路径冲突键做排他。
- `memory_write` 按 key 做排他。
- `todowrite` 仍保持 session 级排他。

P4：eval 和 runtime 指标。

- 统计 tool batch wall time、并发节省时间、失败率。
- 增加 eval budget gate：复杂任务中 tool wait time 不能退化。
- Langfuse dashboard 展示 batch 效果。

## Acceptance

每个切片都需要满足：

- 有针对性单测；
- trace 中能解释工具调度行为；
- 不改变模型可见 tool 参数 schema；
- 对未知工具默认保守串行；
- 对 session / workspace 写入保持安全。
