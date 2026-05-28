# 生产运行时日志设计

## 目标

OpenAgent 现在把生产诊断拆成两个界面：

- **可观测性事件**：面向机器消费的 trace/span 事件，用于指标、回放、评测和回归分析。
- **运行时日志**：面向运维和排障的结构化日志，用于问题定位、审计和生产日志管道。

两者通过 `session_id`、`run_id`、`trace_id` 和当前 `span_id` 关联，但职责不同。这样既能避免 trace 流被日志噪声污染，也能给生产运维保留足够有用的生命周期日志。

## 运行时日志模型

运行时日志模块位于：

```text
src/openagent/core/runtime_logging.py
```

它定义了：

- `RuntimeLoggingConfig`：从 `AgentConfig.options["logging"]` 解析出的运行时日志配置。
- `RuntimeLogRecord`：结构化日志记录。
- `RuntimeLogger`：元数据环形缓冲区、可选 JSONL sink、可选 Python logging bridge。

每条日志记录包含：

```text
log_id
timestamp_ms
level
message
category
session_id
run_id
trace_id
span_id
attributes
```

记录存储在：

```text
Session.metadata["runtime_logging"]["records"]
```

启用 JSONL 时，记录也会写入：

```text
.openagent/logs/<session_id>/<run_id>.jsonl
```

## 配置

默认行为适合生产环境：

```python
options={
    "logging": {
        "enabled": True,
        "keep_records": True,
        "jsonl": False,
        "jsonl_dir": ".openagent/logs",
        "max_records": 500,
        "input_preview_chars": 2048,
        "level": "INFO",
        "python_logging": False,
        "logger_name": "openagent.runtime",
        "include_context": True,
    }
}
```

生产部署可以启用 JSONL 和 Python logging：

```python
options={
    "logging": {
        "level": "INFO",
        "jsonl": True,
        "python_logging": True,
    }
}
```

`logging` 是仅运行时使用的配置。它会在 provider 调用前被过滤，不会发送给 OpenAI-compatible provider。

## 日志等级

OpenAgent 的默认日志保持有用但克制：

- `INFO`：run 开始/结束、step 开始/结束、compaction 开始/结束、patch detected、question requested、context prune。
- `WARNING`：compaction 空输出、模型重试、policy reminder 重试、permission denied。
- `ERROR`：run failure、tool failure、doom loop、不可恢复的模型/工具错误。
- `DEBUG`：工具调用开始/成功详情，以及跳过 compaction 的细节。

这能让生产日志保持可读，同时允许通过 `level="DEBUG"` 打开更深细节。

## 安全

运行时日志复用可观测性清洗器：

- key 中包含 `api_key`、`apikey`、`authorization`、`cookie`、`password`、`secret` 或 `token` 的字段会被脱敏。
- `input_tokens` 和 `output_tokens` 这类 token 指标会保留。
- 任意值都会安全渲染，过长字符串会被截断。
- 默认不记录完整 prompt、完整工具输出和完整文件内容。

工具日志记录输入预览和输出统计，不记录完整输出正文：

```text
tool_name
call_id
input_preview
output_bytes
output_lines
output_truncated
output_path
error_kind
```

## AgentLoop 集成

`AgentLoop` 在 run 开始时同时创建 `ObservationRecorder` 和 `RuntimeLogger`。运行时日志绑定 recorder 的 `run_id` 和 `trace_id`，并在可用时使用 recorder 的当前 span 作为 `span_id`。

主要生命周期日志：

```text
Agent run started
Agent step started
Agent step finished
Agent run finished
Model call failed; retrying
Context compaction started
Context compaction finished
Workspace patch detected
Question requested
Doom loop detected
```

工具 middleware 也会为权限拒绝、工具失败和 debug 级工具执行详情发出结构化日志。

## 生产效果

通过这套设计，生产运维可以回答：

- 哪个 run 失败了？
- 哪个 step 失败了？
- 失败是模型、工具、权限、上下文还是策略导致的？
- 应该打开哪条 trace 做更深回放？
- 消耗了多少 token 和 cost？
- 哪个 patch 修改了 workspace？
- 失败前是否发生过 context pruning 或 compaction？

运行时日志提供第一线生产视角；可观测性事件提供更深入的 trace/eval/replay 视角。
