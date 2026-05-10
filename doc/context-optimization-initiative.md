# OpenAgent 上下文优化专项

> 面向：Agent 架构设计复盘、技术review表达、后续 Roadmap 对齐
> 范围：结构化 compaction、上下文渲染、预算降级、兼容策略、测试验证

## 1. 专项背景

OpenAgent 已经具备基础的上下文预算能力：会在模型调用前估算输入 token，必要时裁剪旧工具输出、压缩历史消息，并在溢出时做降级。但原先的 compaction 结果只是自由文本 summary，例如“Goal: continue implementing”。这种方式能省 token，但在长任务中存在几个问题：

- 摘要不稳定：不同模型会写出不同风格，重要信息可能被混在自然语言里。
- 不可机器消费：后续无法单独读取“文件、决策、阻塞、下一步”等字段。
- 恢复能力弱：模型恢复会话时要从一段 prose 里重新推断工作状态。
- 预算不可控：摘要如果写长了，仍可能导致 compact 后继续超预算。
- 无法支撑后续 ContextPackBuilder：没有结构化字段，就很难做上下文选择、排序和诊断。

这次专项的目标不是做 LSP，也不是上向量库，而是先把 OpenAgent 的上下文治理升级成一个稳定的 runtime 能力：**把历史对话压缩成结构化工作状态，并根据预算动态投影给模型。**

## 2. 已完成修改

### 2.1 落盘设计文档

新增：

- `doc/structured-work-state-compaction-design.md`

这份文档定义了结构化工作状态的目标、非目标、schema、兼容策略、失败策略和生产级不变量。

核心设计点：

- compaction record 仍然存储在 `Session.metadata["context_compaction"]`
- 保留旧字段 `summary`，避免破坏旧会话和旧调用方
- 新增 `schema_version`、`format`、`state`、`source`、`parse_error`
- `state` 里明确拆分 `task/progress/decisions/files/tool_findings/todos/open_questions/blockers/next_steps/risks`

### 2.2 新增结构化工作状态模块

新增：

- `src/openagent/core/context_state.py`

它负责：

- 解析模型返回的 compaction 文本
- 支持 raw JSON、fenced JSON、嵌入式 JSON、旧文本 fallback
- 归一化 work state schema
- 限制字段长度和列表长度，避免把 overflow 从历史消息转移到 metadata
- 渲染完整结构化状态
- 渲染 brief 状态，用于预算紧张时的降级投影

关键能力：

```text
parse_work_state_output(raw_text)
  -> ParsedWorkState(state, summary, source, parse_error)

build_compaction_record(raw_text, compacted_until, updated_at)
  -> backward-compatible context_compaction record
```

### 2.3 升级上下文消息渲染

修改：

- `src/openagent/core/context_messages.py`

新增能力：

- `get_context_compaction()` 同时识别旧 summary 和新 structured work state
- `build_messages_for_model()` 能注入 `[Structured work state]`
- `build_brief_messages_for_model()` 提供极简状态投影
- `build_brief_trimmed_messages_for_model()` 在极端预算压力下只保留结构化状态和最近用户回合

兼容策略：

- 老格式仍然显示为 `[Compacted context summary]`
- 新格式显示为 `[Structured work state]`
- 即使 metadata 中的 `summary` 过期，只要 `state` 存在，就以 `state` 重新渲染为准

### 2.4 接入 AgentLoop 主链路

修改：

- `src/openagent/core/loop/processor.py`

主要变化：

- compaction prompt 从“写摘要”变成“返回结构化 JSON”
- `_run_compaction_summary()` 重命名为 `_run_compaction_model()`
- `_compact_context()` 不再直接保存自由文本，而是调用 `build_compaction_record()`
- compact 后新增多级预算降级：

```text
after_compact
  完整 structured work state + compacted_until 后的新消息

after_compact_brief
  brief structured work state + compacted_until 后的新消息

after_compact_minimal
  brief structured work state + 最近 1 个用户回合
```

这解决了一个真实集成问题：结构化状态比旧摘要更有信息量，但也可能更长。如果模型上下文窗口极小，完整结构化状态仍可能超预算。因此 runtime 需要“保留完整状态、按预算投影”的能力。

### 2.5 补充测试

新增：

- `src/tests/test_context_state.py`

修改：

- `src/tests/test_context_messages.py`
- `src/tests/test_loop.py`

覆盖点：

- JSON work state 解析
- fenced JSON 解析
- legacy text fallback
- compaction record 兼容字段
- structured work state 注入模型消息
- brief/minimal 投影
- AgentLoop compact 策略集成
- compact 后仍能继续执行后续模型调用

验证命令：

```bash
PYTHONPATH=src:src/tests python -m unittest \
  src/tests/test_context_state.py \
  src/tests/test_context_messages.py \
  src/tests/test_context_budget.py \
  src/tests/test_loop.py
```

结果：53 个测试通过。

全量测试仍有两个既有失败：

- `src/tests/test_legacy_cli.py` 引用仓库内缺失的 `legacy_cli.py`
- `src/tests/test_execution_runtime.py` 的 OpenSandbox fake search 与当前 runtime 调用形态不一致

这两个问题不是本专项引入。

## 3. 解决了什么问题

### 3.1 长任务上下文丢失

原来压缩历史后，模型只看到一段自然语言摘要。现在它看到的是结构化状态：

```text
[Structured work state]
Task:
...

Progress:
- ...

Decisions:
- ...

Files:
- ...

Next steps:
- ...
```

这使模型更容易恢复任务，而不是重新理解一整段聊天历史。

### 3.2 摘要不可控、不可解析

自由文本 summary 对人可读，但对系统不可操作。结构化 `state` 可以被后续模块直接消费：

- UI 可以展示当前任务、下一步、阻塞
- ContextPackBuilder 可以按字段选择上下文
- 诊断系统可以检查 compaction 是否丢失文件信息
- 后续持久化 session 时可以单独保存工作状态

### 3.3 Provider 输出不稳定

不同模型可能返回：

- 纯 JSON
- markdown fenced JSON
- JSON 外面包解释文本
- 完全自由文本

现在解析器是 best-effort 的，不依赖 JSON mode。只要模型输出有可用内容，系统就能归一化成 work state；如果解析失败，也会把文本放入 legacy fallback，并记录 `parse_error`。

### 3.4 compact 后仍然超预算

结构化状态信息更完整，也可能更长。专项新增了预算感知投影：

- 预算足够：注入完整结构化状态
- 预算紧张：注入 brief 状态
- 预算极紧：注入 brief 状态 + 最近用户回合

这体现了一个关键原则：**存储可以完整，投影必须按预算自适应。**

### 3.5 为未来上下文工程打基础

这次没有直接做 ContextPackBuilder，但已经把最重要的底座补上了：

- 状态 schema
- 兼容 record
- 渲染层
- budget fallback
- parser diagnostics
- 单测覆盖

后续可以自然扩展成：

```text
Session.messages
Session.metadata.context_compaction.state
Tool output manifest
Repo context manifest
Todo state
Patch state
  -> ContextPackBuilder
  -> ranked context items
  -> model-ready messages
```

## 4. 达到的效果

### 4.1 Agent 恢复能力更强

模型不再依赖“读懂摘要”来恢复上下文，而是能直接看到当前目标、已完成工作、关键文件、阻塞和下一步。

### 4.2 上下文更可解释

以前只能问“为什么模型忘了？”，现在可以直接检查：

```json
Session.metadata["context_compaction"]["state"]
```

看里面是否有：

- `task`
- `files`
- `tool_findings`
- `next_steps`
- `risks`

如果丢了，就知道是 compaction prompt、parser、还是上游工具信息不足。

### 4.3 上下文更可控

新增完整、brief、minimal 三档投影，避免“压缩后还是爆上下文”。这比单纯缩短 summary 更稳，因为它保留了完整 metadata，只是在模型调用时选择更小的表达。

### 4.4 兼容旧会话

旧格式：

```json
{"summary": "Goal: continue", "compacted_until": 2}
```

仍然可用。新格式只是增强，不是破坏性迁移。

### 4.5 生产可维护性提高

本次不是只改 prompt，而是把能力拆成了独立模块：

- `context_state.py` 管 schema 和 parser
- `context_messages.py` 管模型消息投影
- `processor.py` 管 loop 编排

职责边界更清晰，后续扩展不会把所有上下文逻辑继续堆在 AgentLoop 里。

## 5. 最终体验

对用户来说，体验上的变化不是一个新的按钮，而是 Agent 在长任务中更稳定：

- 长对话后不容易忘记当前目标
- 压缩历史后仍然知道改了哪些文件
- 继续任务时更少重复探索
- 预算紧张时不会直接报错，而是尽量用最小工作状态继续
- 如果模型输出不规范，系统仍然能 fallback，而不是整次 compaction 失败

对上层产品来说，`context_compaction` 变成了一个可展示、可诊断、可持久化的工作状态对象。

## 6. 架构取舍

### 6.1 为什么不先做 LSP

LSP 解决的是“代码符号理解”和“代码导航”。但 OpenAgent 当前更缺的是长任务连续性。没有稳定上下文，即使有 LSP，模型也可能忘记任务状态、用户约束和历史决策。

所以优先级是：

```text
上下文连续性 > 上下文结构化 > 上下文选择 > 代码智能增强
```

### 6.2 为什么不直接上向量数据库

向量库解决的是检索问题，不解决“当前任务状态是什么”的问题。Agent runtime 里更基础的是 work state：

- 当前目标
- 已完成动作
- 决策和约束
- 下一步

这些信息通常不是靠 embedding 检索出来的，而是应该在执行过程中持续维护。

### 6.3 为什么保留 summary 字段

这是兼容性设计。旧调用方可能只知道 `summary`，旧会话也只有 `summary`。保留它可以避免一次 schema 升级破坏历史数据。

### 6.4 为什么做多档投影

结构化状态有完整性，但模型输入有预算。完整存储和模型投影是两个问题：

- metadata 里保存完整 state
- 模型调用时根据预算选择 full / brief / minimal

这是面向生产的上下文设计，不是简单“把摘要写短一点”。

## 7. review Agent 架构师时可以怎么讲

### 7.1 一句话描述专项

我把 OpenAgent 的上下文压缩从自由文本 summary 升级成结构化 work state，并加入预算感知投影，让长任务在压缩、恢复和低上下文预算下都能稳定继续。

### 7.2 架构表达

我把上下文看成一个 runtime pipeline，而不是聊天历史数组：

```text
conversation/tool results/todos
  -> compaction model
  -> structured work state parser
  -> normalized state record
  -> full/brief/minimal projection
  -> context budget check
  -> model call
```

这条链路里，每一步都有明确职责：

- compaction model 负责提炼
- parser 负责容错和归一化
- metadata record 负责持久状态
- renderer 负责模型可读性
- budget layer 负责是否降级

### 7.3 可以强调的技术判断

- 没有把问题简化成“更短的 prompt”，而是把上下文状态结构化。
- 没有依赖 JSON mode，因为多 provider runtime 不能假设所有模型能力一致。
- 没有牺牲兼容性，旧 summary 格式仍然能跑。
- 没有把完整状态直接塞给模型，而是做预算感知投影。
- 没有先做向量库，因为工作状态和长期检索是两个问题。

### 7.4 可以举的例子

原来压缩后：

```text
Goal: continue implementing
```

现在压缩后：

```text
[Structured work state]
Task:
Implement structured compaction

Progress:
- Added design doc
- Wired AgentLoop compaction generation

Files:
- src/openagent/core/context_state.py (created) - parses provider output
- src/openagent/core/context_messages.py (modified) - renders structured state

Next steps:
- Run tests
- Address remaining full-suite failures
```

这就是从“摘要”变成了“可恢复工作现场”。

### 7.5 review可用回答模板

如果review官问“你怎么做 Agent 长上下文管理”，可以回答：

> 我会把上下文管理拆成三层：历史消息治理、工作状态治理、检索上下文治理。历史消息可以裁剪和压缩，但真正支撑长任务的是工作状态。我们把 compaction 结果从自由文本升级成结构化 work state，包含 task、progress、decisions、files、tool findings、todos、blockers、next steps、risks。然后在模型调用前按预算选择 full、brief 或 minimal 投影。这样既保留完整状态，又不会因为状态太长导致二次 overflow。

如果review官问“为什么不用向量库解决”，可以回答：

> 向量库适合找相关材料，但不适合维护当前任务状态。当前目标、用户约束、已完成工作和下一步应该是执行链路的一等状态，而不是每次靠检索碰运气。向量检索可以作为后续补充，但首先要把 work state 做稳定。

如果review官问“生产里模型输出 JSON 不稳定怎么办”，可以回答：

> 我不会强依赖 JSON mode。我们做了 best-effort parser，支持纯 JSON、markdown fenced JSON、嵌入式 JSON和旧文本 fallback。解析成功就生成结构化 state；解析失败但有文本，就降级成 legacy_text_fallback，并记录 parse_error 便于诊断。

如果review官问“上下文预算不够怎么办”，可以回答：

> 我们区分存储态和投影态。存储态保留完整 structured state；投影态按预算降级：先 full work state，再 brief work state，最后 minimal work state 加最近用户回合。这样不会因为结构化状态本身变长而让 compaction 失效。

## 8. 待改进点

### 8.1 接入 session 持久化

目前 `context_compaction` 已经是结构化 record，但 session storage 主链路还没完全接入。下一步应该把它持久化到 `.openagent/sessions/<session_id>.json`，支持真正的跨进程 resume。

### 8.2 做 ContextPackBuilder

现在 structured state 仍作为一个 synthetic message 注入。后续应该升级成 ContextPackBuilder，把 state、todo、diff、tool findings、repo map 都作为 `ContextItem` 排序和选择。

### 8.3 增加上下文诊断事件

可以输出：

```json
{
  "context_stage": "after_compact_brief",
  "included_items": 12,
  "dropped_items": 5,
  "estimated_tokens": 12000,
  "limit": 16000
}
```

这样 UI 和日志能解释“为什么模型这次只看到了 brief 状态”。

### 8.4 引入 repo context manifest

短期不做 LSP，也可以做轻量 repo map：

- 文件树
- 重要文件摘要
- 最近 read/edit/grep 命中文件
- git diff 文件

然后把它作为 context item 进入 pack。

### 8.5 让 compaction schema 可配置

不同 agent 可能需要不同 schema：

- coding agent 需要 files、patch、tests
- research agent 需要 sources、claims、citations
- planning agent 需要 decisions、risks、milestones

后续可以按 agent profile 选择 compaction schema。

### 8.6 增加质量评估

可以构造长会话 benchmark，评估：

- 压缩后能否继续完成任务
- 是否保留关键文件和决策
- 是否减少重复探索
- 是否降低 context overflow 错误率
- full/brief/minimal 分别的成功率

## 9. 专项价值总结

这次优化的核心价值不是“多了一个摘要格式”，而是把 OpenAgent 的上下文系统从被动裁剪升级成主动维护工作状态。

它让 OpenAgent 在架构上具备了三个能力：

- **Continuity**：长任务压缩后还能延续
- **Observability**：上下文状态可检查、可诊断
- **Adaptivity**：根据预算自动选择合适投影

这正是 coding agent runtime 和普通聊天机器人最大的区别之一：普通聊天机器人保存对话，Agent runtime 需要保存“工作现场”。
