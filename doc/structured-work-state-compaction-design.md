# 结构化工作状态压缩设计

> 状态：已在 OpenAgent core runtime 中实现。
> 范围：`src/openagent/core/context_state.py`、`context_messages.py` 和 `loop/processor.py`。

## 1. 目标

OpenAgent 之前会把旧对话历史压缩成一段自由文本 summary。这能节省 token，但会让长任务会话变脆弱，因为模型必须从自然语言里重新推断真实工作状态。

结构化工作状态压缩会把 summary 升级成确定性的 continuation packet。每次 compaction 都应该保留：

- 当前任务和用户意图
- 已完成工作和重要决策
- 对下一步有影响的文件和产物
- 工具发现、失败和证据
- 活跃 todo、阻塞点、开放问题和可能的下一步
- 明确的风险和验证缺口

模型应该收到一份紧凑的工作状态：易扫读、跨 provider 稳定，并且能在上下文压力后安全恢复。

## 2. 非目标

- 不引入 LSP、embedding 或语义代码检索。
- 不替代 session 中存储的完整消息历史。
- 不引入新的 memory 系统。
- 不要求所有 provider 支持 JSON mode。

## 3. 运行时契约

compaction 记录仍然存储在：

```text
Session.metadata["context_compaction"]
```

记录与旧格式保持向后兼容：

```json
{
  "summary": "rendered continuation state",
  "compacted_until": 12,
  "updated_at": 1760000000000
}
```

结构化 compaction 增加以下字段：

```json
{
  "schema_version": 1,
  "format": "structured_work_state",
  "state": {
    "task": "...",
    "progress": ["..."],
    "decisions": ["..."],
    "files": [
      {"path": "src/app.py", "status": "read", "note": "contains the failing route"}
    ],
    "tool_findings": ["..."],
    "todos": ["..."],
    "open_questions": ["..."],
    "blockers": ["..."],
    "next_steps": ["..."],
    "risks": ["..."]
  },
  "summary": "[Structured work state]\n...",
  "compacted_until": 12,
  "updated_at": 1760000000000,
  "source": "model_json",
  "parse_error": null
}
```

`summary` 仍然是旧调用方使用的 canonical rendered string。`state` 是用于诊断、未来 ContextPackBuilder 和产品 UI 的结构化 payload。

## 4. 工作状态 Schema

### 必填字段

- `task`: string
- `progress`: list of strings
- `decisions`: list of strings
- `files`: list of file objects
- `tool_findings`: list of strings
- `todos`: list of strings
- `open_questions`: list of strings
- `blockers`: list of strings
- `next_steps`: list of strings
- `risks`: list of strings

### 文件对象

```json
{
  "path": "relative/or/display/path",
  "status": "read|modified|created|deleted|mentioned|unknown",
  "note": "why this file matters"
}
```

parser 会把任意 provider 输出归一化成这个形状：

- 非字符串列表项会被转成紧凑 JSON 字符串
- 空字符串会被丢弃
- 文件条目可以是对象或字符串
- 未知文件状态会归一化成 `unknown`
- 每个列表和列表项都有长度上限

## 5. Prompt 策略

compaction 模型调用使用专用 system prompt，只要求输出 JSON object。它与 provider 无关，也不依赖 JSON mode。

user prompt 会在可用时把当前 todo list 作为 JSON 放进去。模型会被要求只保留可行动状态，不保留寒暄话术或过期尝试。

## 6. 渲染策略

注入回模型的 rendered message 以以下内容开头：

```text
[Structured work state]
```

然后渲染稳定 section：

```text
Task:
...

Progress:
- ...

Decisions:
- ...
```

空 section 会被省略，只有 `Task` 例外；它会 fallback 到 `(unspecified)`。

这样即使下游模型忽略 metadata，压缩后的上下文仍然可读。

## 7. 失败处理

compaction 不应该因为 provider 返回 markdown fence、解释性文字或轻微格式错误就失败。

parser 支持：

- 原始 JSON object
- fenced JSON block
- 嵌在自然语言里的 JSON
- 旧版自由文本 summary

如果没有任何可用内容，compaction 会失败，并交给现有预算 fallback 路径处理。如果返回了非空自由文本 summary，它会被包装成结构化 fallback state，并写入 `source = "legacy_text_fallback"` 和 `parse_error` metadata。

## 8. 兼容性

`get_context_compaction()` 同时接受旧记录和新记录，并都会返回 rendered summary。

旧 metadata：

```json
{"summary": "Goal: continue", "compacted_until": 2}
```

仍然会渲染成 compacted context message。新 metadata 额外暴露 `state`、`format`、`schema_version` 和 parser diagnostics。

## 9. 生产不变量

- 如果 `compacted_until` 超出 session message 范围，compaction record 无效。
- rendered content 是确定性的。
- 结构化字段有边界，避免把 history 的溢出问题转移到 metadata。
- prompt 输出解析采用 best-effort，但绝不静默保存空状态。
- 旧 `summary` 字段保留，以维持兼容。
- 测试覆盖 JSON 解析、fenced JSON 解析、legacy fallback、message injection 和 loop integration。

## 10. 后续扩展

这个结构刻意贴近未来 `ContextPackBuilder` 的形态。后续可以独立排序 `state.files`、`state.tool_findings` 和 `state.todos`，而不是把整个 compaction 当成一条单独消息。
