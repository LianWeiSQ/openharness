# Context Persistence P2

## 目标

P2 的目标是把 OpenAgent 的一次运行拆成可恢复的 `session parts`：

- step 什么时候开始、什么时候结束；
- 模型决定调用了哪些工具；
- runtime 实际返回了哪些工具结果；
- 哪一步产生了 patch；
- 哪一步消耗了多少 token/cost；
- 每一步引用了哪个 context pack、context assets、session memory。

P0 解决的是 context pack 快照落盘，P1 解决的是 instruction/file/memory 资产落盘。P2 解决的是：把这些资产和 AgentLoop 的真实执行顺序串成一条可复盘的工程流水账。

## 背景

对标 Claude Code 和 OpenCode，成熟的 Agent runtime 不只保存对话消息。它们还会保存：

- system/context 注入来源；
- tool call / tool result；
- patch / file change；
- usage / cost；
- compaction / memory；
- session resume 所需的结构化状态。

OpenAgent 已经有 `events.jsonl` 和 `state.latest.json`，但 event 更偏审计日志，message 更偏模型上下文。P2 增加 `parts.jsonl`，用于表达“这次运行由哪些可展示、可恢复、可统计的片段组成”。

## 落盘结构

```text
.openagent/sessions/{session_id}/
  state.latest.json
  transcript.jsonl
  session-memory.md
  runs/{run_id}/
    events.jsonl
    parts.jsonl
    summary.json
    context/
      context-pack-step-0001.json
      context-assets-step-0001.json
```

`parts.jsonl` 中每一行是一个独立 part：

```json
{
  "schema_version": "openagent.session_part.v1",
  "part_id": "part_xxx",
  "seq": 1,
  "type": "tool-call",
  "timestamp_ms": 1760000000000,
  "session_id": "sess_xxx",
  "run_id": "run_xxx",
  "step_index": 1,
  "status": "ok",
  "attributes": {}
}
```

## Part 类型

P2 第一阶段落盘这些类型：

- `run-start`：一次 run 的入口信息。
- `step-start`：AgentLoop 一轮 step 的开始。
- `tool-call`：模型决定调用的工具、call id、输入预览。
- `tool-result`：runtime 执行结果、tool source、错误状态、输出大小、耗时。
- `patch`：本 step 产生的工作区文件变化摘要。
- `usage`：本 step 的 input/output token、cost、finish reason。
- `step-finish`：本 step 的收敛状态、工具数量、耗时。
- `context-pack-reference`：本 step 引用的 P0 context pack 快照。
- `context-assets-reference`：本 step 引用的 P1 context assets 快照。
- `memory-reference`：本 step 更新后的 session memory 引用。
- `compaction`：触发上下文压缩时的压缩记录引用。

默认仍然只保存 metadata 和引用，不把完整 prompt、工具输出、文件正文复制到 parts。

## 代码入口

- `FileSessionStore.append_part(...)`：追加 `parts.jsonl`。
- `load_session_parts(session, run_id=None)`：恢复后读取 parts。
- `AgentLoop.run(...)`：在 step/tool/patch/usage/context/memory 节点写入 part。
- `summary.json`：增加 `part_count` 和 `part_type_counts`，让 eval/report 可以快速统计。

## 和 events/messages 的关系

`messages` 是模型上下文的对话投影。

`events` 是运行审计日志，适合机器统计和故障定位。

`parts` 是可展示、可恢复、可复盘的 session 片段，适合：

- Web/CLI 按 step 展示；
- Langfuse trace 对齐；
- eval report 引用；
- session resume 后人工检查；
- 后续做 session/message/part 数据库模型。

## 非目标

P2 不做：

- 不引入数据库；
- 不改变模型实际输入；
- 不默认存储完整敏感内容；
- 不做 Web UI 展示；
- 不做跨 session 长期知识库召回。

这些属于后续 P3/P4。

## 验收标准

运行一个包含 3 step、2 次工具调用、1 次 patch、usage 统计、context snapshot、memory 更新的任务后：

- `parts.jsonl` 存在；
- `parts.jsonl` 包含 `step-start` 和 `step-finish`；
- `parts.jsonl` 包含 `tool-call` 和 `tool-result`；
- `parts.jsonl` 包含 `patch`；
- `parts.jsonl` 包含 `usage`；
- `parts.jsonl` 包含 `context-pack-reference`；
- `parts.jsonl` 包含 `context-assets-reference`；
- `parts.jsonl` 包含 `memory-reference`；
- `summary.json.part_count == len(parts.jsonl)`；
- `summary.json.part_type_counts` 能统计各类 part；
- `resume_session(...)` 后 `load_session_parts(...)` 能读取同一份 parts。

## 后续方向

P3 可以继续推进：

- part schema 版本演进；
- message/part 双向索引；
- tool result 内容按策略分层保存；
- Web/CLI 实时 timeline；
- Langfuse observation 与 session part id 对齐；
- eval baseline regression 按 part/token/cost 做门禁。
