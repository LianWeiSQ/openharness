# Context Persistence P0

## 目标

P0 的目标是把 OpenAgent 的上下文管理从“运行时 metadata”推进到“可落盘、可恢复、可审计”的第一阶段。

本阶段不改变模型实际看到的 prompt 语义，只把每一步已经构建出来的 context pack 诊断信息持久化，形成后续 eval、Langfuse、Web UI、resume 能共同引用的稳定证据。

## 背景

OpenAgent 已经具备：

- `ContextPackBuilder`：把 session messages、todo、instruction、file context、sandbox metadata 等统一抽象成 `ContextItem`。
- `FileSessionStore`：写入 run、transcript、events、summary、state.latest。
- `AgentLoop`：每个 step 记录 model/tool/patch/usage/runtime warning。

缺口是：`context.pack_built` 之前主要存在于 session metadata 和 trace 事件里，不够像一个独立上下文快照资产。进程恢复、eval 报告、Langfuse trace、Web inspection 都需要一个稳定文件路径来引用“这一轮模型调用前，上下文是怎么组成的”。

## P0 需求

### 1. Context pack snapshot 落盘

每次 AgentLoop 构建 context pack 诊断后，写入独立 JSON 文件：

```text
.openagent/sessions/{session_id}/runs/{run_id}/context/context-pack-step-0001.json
```

快照包含：

- `session_id`
- `run_id`
- `step_index`
- `fallback_stage`
- `message_count`
- `item_count`
- `included_count`
- `estimated_input_tokens`
- 每个 context item 的 metadata-only trace：
  - `item_id`
  - `kind`
  - `source`
  - `priority`
  - `pinned`
  - `stable_prefix`
  - `token_estimate`
  - `included`
  - `drop_reason`

默认不写入 prompt 正文、工具输出正文、文件正文，避免把敏感内容扩散到额外产物里。

### 2. Session metadata 记录快照引用

`Session.metadata` 增加：

- `context_pack_snapshots`
- `last_context_pack_snapshot`

这些字段只保存 snapshot path、step index、token estimate、included count 等引用信息。

### 3. Session ledger 记录快照事件

每次快照保存后，run ledger 追加：

```text
context.pack_snapshot.saved
```

这样 eval/report/trace 可以通过事件流统计和定位 context snapshot。

### 4. Resume API

新增：

```python
resume_session(session_id, root_dir=...)
load_latest_context_pack_snapshot(session)
```

`resume_session` 负责从 `SessionStore` 恢复：

- `Session.messages`
- `Session.todos`
- `Session.metadata`
- `Session.directory`
- `Session.status`

恢复后的 Session 可以继续交给 `AgentLoop` 创建新的 run。

## 非目标

P0 不做：

- 不改变模型输入投影策略。
- 不引入数据库。
- 不把 message 拆成 OpenCode 风格 part。
- 不做跨 session 长期记忆提炼。
- 不把完整 prompt/tool output 默认写入额外 snapshot。

这些属于 P1/P2。

## 验收标准

运行一个 3-step 任务后应满足：

- `events.jsonl` 包含 3 条 `context.pack_snapshot.saved`。
- run 目录下存在 `context/context-pack-step-0001.json` 等文件。
- `state.latest.json` 能恢复 `last_context_pack_snapshot`。
- `resume_session(...)` 能恢复 `Session.messages`。
- `load_latest_context_pack_snapshot(...)` 能读取最新 context snapshot。

## 对标关系

### 对标 Claude Code

Claude Code 会在会话开始注入稳定的 system/user context，例如 git status、CLAUDE.md、memory files，并缓存为本轮会话上下文快照。

OpenAgent P0 的对应动作是：把每步 context pack 的组成落盘，使上下文构建过程可追踪。

### 对标 OpenCode

OpenCode 把 session、message、part、todo 存进数据库，step-start、tool、patch、step-finish 都是可恢复的结构化 part。

OpenAgent P0 暂时不做完整 part 化，而是先把 context pack 作为 run artifact 落盘，为 P2 的 message part 化提供证据来源。

## 后续阶段

P1：

- instruction snapshot 一等持久化；
- file context store 一等持久化；
- session memory markdown；
- resume 时自动校验 instruction/file context 是否变化。

P2：

- message part schema；
- tool/patch/usage/compaction part 化；
- Web UI 可以按 session/message/part 展示完整运行过程。
