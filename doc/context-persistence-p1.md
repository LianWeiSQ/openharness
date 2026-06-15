# Context Persistence P1

## 目标

P1 的目标是把 OpenAgent 的关键上下文来源升级成可恢复的上下文资产：

- instruction snapshot；
- file context snapshot；
- session memory markdown；
- resume 时的上下文资产校验。

P0 已经把每个 step 的 `ContextPack` 组成落盘。P1 进一步回答两个问题：

1. 这些 context item 背后的规则文件和已读文件当时是什么状态？
2. 进程重启或跨会话恢复时，这些上下文资产有没有发生变化？

## P1 需求

### 1. Instruction Snapshot

每个 step 生成 instruction asset snapshot，记录：

- 加载了多少 instruction 文件；
- 来源 scope：workspace / user；
- source；
- display path；
- bytes read；
- truncated；
- content hash；
- loader issues。

快照不默认复制完整 instruction 正文，只保存 hash 和读取元数据，减少敏感内容扩散。

### 2. File Context Snapshot

每个 step 生成 file context asset snapshot，记录：

- 已读文件路径；
- absolute path；
- size；
- mtime；
- content hash；
- source tool；
- read timestamp；
- preview；
- 是否已变化、缺失或不可读。

这让 OpenAgent 能回答：“模型当前依赖哪些文件视图，这些文件现在还可靠吗？”

### 3. Session Memory Markdown

每个 step 结束后更新：

```text
.openagent/sessions/{session_id}/session-memory.md
```

内容包括：

- session id；
- workspace；
- last step；
- structured work state；
- todos；
- recent messages；
- file context；
- latest context pack reference。

它不是替代 transcript，而是给长任务恢复、人工审阅、后续 compaction 的轻量 continuation packet。

### 4. Resume Context Asset Check

`resume_session(...)` 会自动读取最新 context assets，并写入：

```python
Session.metadata["session_resume"]["context_asset_check"]
```

校验内容：

- instruction 文件是否 unchanged / changed / missing / unreadable；
- file context 文件是否 unchanged / changed / missing / unreadable；
- remote/virtual path 标记为 `remote_unchecked`。

## 落盘结构

```text
.openagent/sessions/{session_id}/
  state.latest.json
  transcript.jsonl
  session-memory.md
  runs/{run_id}/
    events.jsonl
    summary.json
    context/
      context-pack-step-0001.json
      context-assets-step-0001.json
```

## Ledger Events

P1 新增：

```text
context.assets_snapshot.saved
session.memory.updated
```

这些事件和 P0 的 `context.pack_snapshot.saved` 一起构成 context persistence 的审计链。

## 非目标

P1 不做：

- 不把 message 拆成 part；
- 不做数据库迁移；
- 不改变模型输入；
- 不自动把 session memory 注入模型；
- 不做复杂长期知识库召回。

这些属于 P2 或后续 memory/RAG 专项。

## 验收标准

运行一个包含 instruction 文件和 read 工具的 3-step 任务后：

- `events.jsonl` 包含 `context.assets_snapshot.saved`；
- `events.jsonl` 包含 `session.memory.updated`；
- run context 目录存在 `context-assets-step-0003.json`；
- asset snapshot 里 instruction count 大于 0；
- asset snapshot 里 file record count 大于 0；
- `session-memory.md` 存在并包含最近任务状态；
- `resume_session(...)` 恢复后 `context_asset_check.status == "unchanged"`；
- 修改 instruction 文件后，`validate_resume_context_assets(...)` 能返回 `changed`。

## 对标关系

### Claude Code

Claude Code 的强项是把 CLAUDE.md、memory files、git status、session memory 等作为上下文资产管理。P1 对应的是把 OpenAgent 的 instruction/file/session memory 变成可落盘资产，而不是只存在于临时 metadata。

### OpenCode

OpenCode 的强项是 session/message/part 数据库模型。P1 暂时不做 part 化，但通过 `context-assets-step-*.json` 和 `session-memory.md` 为 P2 的 part 化提供稳定来源。

## 后续 P2

P2 应该把当前 event/message 混合模型升级为 message part schema：

- step-start part；
- tool part；
- patch part；
- usage part；
- compaction part；
- memory part；
- context asset reference part。
