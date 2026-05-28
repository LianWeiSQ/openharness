# P0 可观测性与评测设计

本文档描述 OpenAgent 第一版面向生产的可观测性与评测闭环。

## 目标

P0 让 OpenAgent 的每次运行都可追踪、可回放、可度量，同时不引入外部遥测服务。

第一版重点覆盖：

- 结构化的 run / step / model / tool / context 事件
- 安全写入 `Session.metadata["observability"]` 的元数据
- 可选 JSONL trace
- 确定性 eval case
- 本地 eval 报告和 trace 摘要

## 可观测性模型

OpenAgent 会为每次 `AgentLoop.run()` 调用记录一条 trace。

trace 根节点是：

```text
Session.metadata["observability"]
```

它包含：

- `trace`：trace 元数据，包括 `trace_id`、`run_id`、`session_id`、agent、model、provider 和 workspace
- `events`：近期结构化事件，按环形缓冲区保留
- `event_count`：累计发出的事件数量
- `jsonl_path`：启用 JSONL 时的 trace 文件路径

事件结构由 `ObservationEvent` 定义：

```text
event_id
trace_id
run_id
session_id
span_id
parent_span_id
name
kind
timestamp_ms
duration_ms
status
attributes
```

默认配置：

```python
options={
    "observability": {
        "enabled": True,
        "keep_events": True,
        "jsonl": False,
        "jsonl_dir": ".openagent/observability",
        "max_events": 500,
        "input_preview_chars": 2048,
        "include_traceback": False,
    }
}
```

`observability` 是仅运行时使用的配置，不会传给模型 provider。

## 记录事件

第一版实现会记录：

- `run.started`
- `run.finished`
- `run.failed`
- `step.started`
- `step.finished`
- `model.call.started`
- `model.call.finished`
- `model.call.failed`
- `model.usage`
- `tool.call.started`
- `tool.call.finished`
- `tool.call.failed`
- `context.budget_checked`
- `context.compaction.started`
- `context.compaction.finished`
- `patch.detected`
- `question.requested`
- `permission.denied`
- `doom_loop.detected`
- `error`

这些事件会优先使用低基数、便于聚合的字段：

- `agent_name`
- `model_id`
- `provider_id`
- `step_index`
- `attempt_index`
- `tool_name`
- `finish_reason`
- `error_kind`
- `fallback_stage`
- `projection`
- `input_tokens`
- `output_tokens`
- `cost`
- `duration_ms`

## 安全

可观测性不能变成隐藏的数据泄漏通道。

清洗器会脱敏 key 中包含以下词的字段：

```text
token
secret
password
api_key
authorization
cookie
```

`input_tokens` 和 `output_tokens` 这类 token 用量指标会保留，因为它们是运维指标，不是凭证。

工具输入只保存脱敏后的预览。工具输出不会完整保存；事件只记录输出大小、是否截断，以及可用时的输出路径。

## 评测循环

本地评测运行器位于 `openagent.core.eval`。

eval case 可以使用 JSON 或 YAML：

```yaml
id: context_compaction_001
input: "继续完成整改，保持之前不做 LSP 的决定"
workspace: fixtures/context_project
history: fixtures/history/long_context.json
expected:
  must_remember:
    - "短期不做 LSP"
  files_changed: []
scoring:
  require_no_error: true
  require_final_answer_contains:
    - "上下文"
```

第一版 scorer 是确定性的，支持：

- final answer 包含或不包含指定文本
- 文件存在
- 文件包含指定内容
- 变更文件 allowlist
- 没有 error 事件
- 最大 step 数
- 最大 cost
- 必须调用某个工具
- 禁止调用某个工具
- 必须记住某个上下文决策

eval 报告会写入：

```text
.openagent/eval-runs/<run>/report.json
.openagent/eval-runs/<run>/summary.md
```

每条结果包含：

```text
case_id
status
score
duration_ms
steps
tool_calls
input_tokens
output_tokens
cost
error_kind
failure_reasons
trace_path
```

## 回放

P0 的 replay 是离线 trace 检查，不是确定性模型重放。

`openagent.core.eval.replay` 可以：

- 加载 JSONL trace 事件
- 汇总模型调用、工具调用、上下文事件、错误、token 和 cost
- 渲染 Markdown trace 摘要

后续可以通过保存模型流式输出 fixture，在不调用真实模型的情况下重放 `AgentLoop`，从而补齐确定性模型 replay。
