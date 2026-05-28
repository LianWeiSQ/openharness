# 终端优先的 Agent 评测系统架构设计

定位：产品与工程架构设计稿  
读者：Agent 平台、模型评测、Sandbox 基础设施、企业接入与评测运营团队

## 1. 执行摘要

本平台的核心目标是建设一套 **Terminal-first 的 Agent 生产级评测系统**。它以 Terminal-Bench 2.0 的评测范式为主参考：每个任务运行在独立终端环境中，agent 需要通过命令行和文件系统完成真实工作，最后由隐藏测试、状态检查或产物验证给出结果。

与传统 LLM benchmark 不同，本平台评估的是：

```text
Agent Harness + LLM + Tool/Terminal Execution + Recovery Strategy
```

而不是裸模型的单次回答能力。

设计结论：

- **主评测形态**：独立 Docker / Sandbox 终端任务。
- **主能力指标**：terminal task completion / resolved rate。
- **核心闭环**：任务下发 -> 沙箱执行 -> agent 循环操作 -> 隐藏验证 -> 诊断报告。
- **OpenAgent 接入方式**：优先容器/本地 adapter；同时支持 API action loop。
- **生产标准**：任务必须可复现、可验证、有区分度、防污染、可审计。
- **能力面板**：Terminal 为主轴，SWE、MLE、Tool、Office、Memory 等作为辅助维度。

平台应服务两类场景：

| 场景 | 目标 |
| --- | --- |
| 内部研发评测 | 对 OpenAgent 或其他 agent 版本做回归、对比和失败分析 |
| 外部用户接入 | 让企业或开发者接入自有 agent，获得非公开报告或可选榜单分数 |

## 2. 设计原则

### 2.1 终端优先

平台以终端任务为第一等公民。代码修复、环境配置、数据处理、ML 实验、工具工作流和文件交付，都尽量落到可执行环境中完成，而不是只做文本题。

原因：

- 终端任务天然覆盖文件、进程、依赖、日志、测试和系统状态。
- 终端任务可以用程序化 verifier 判分，结果更稳定。
- 终端任务更接近真实 agent 在工程场景里的工作方式。

### 2.2 默认使用 Sandbox

正式评测默认每个任务一个独立沙箱。沙箱可以是 Docker 容器，也可以后续扩展为 E2B、Runloop、Kubernetes Job 或内部执行平台。

沙箱是可信评测的基础：

- 隔离任务间状态。
- 保护宿主机和用户密钥。
- 固定依赖和资源。
- 支持运行后销毁。
- 支持隐藏测试和防污染。

### 2.3 Agent 系统级评测

分数评价的是完整 agent 系统，不只评价 LLM。

同一个 LLM，如果换不同 harness、工具策略、上下文压缩、命令执行策略，结果会不同。因此报告必须记录：

- agent 名称与版本。
- LLM/provider 配置。
- 工具和权限配置。
- 执行环境版本。
- 任务集版本。

### 2.4 验证器优先

生产级任务必须优先使用程序化 verifier。LLM Judge 可以用于开放质量评价，但不能单独决定关键任务是否成功。

优先级：

1. 隐藏测试、状态检查、文件校验。
2. 结构化产物检查。
3. LLM Judge。
4. 人工抽检。

### 2.5 可观测、可回放

评测不是只产出一个分数。每次任务运行都要能回放关键过程：

- agent 收到什么任务。
- 执行了哪些命令。
- 命令返回了什么。
- 修改了哪些文件。
- 为什么失败或成功。
- verifier 输出了什么。

## 3. 调研结论与平台定位

### 3.1 Terminal-Bench 2.0 的启发

Terminal-Bench 2.0 的核心价值不在于题目数量，而在于它定义了一类真实 agent 评测范式：

| 特征 | 说明 |
| --- | --- |
| 任务环境 | 每个任务有独立终端环境 |
| 任务形态 | 长程、多步骤、状态化任务 |
| 执行方式 | agent 通过命令行完成任务 |
| 验证方式 | 测试脚本或环境状态判定 |
| 评价对象 | model-agent combination |

本平台采用这一范式，但不局限于 Terminal-Bench 原始任务集。我们需要支持更多生产任务：工程修复、ML 实验、数据处理、工具工作流、Office 产物、长期记忆与自我改进。

### 3.2 Benchmark 调研结论

前期调研显示，当前 agent 评测正在从静态问答转向“真实环境中的长程任务”。不同 benchmark 虽然侧重点不同，但可以归纳为四类能力。

| 类别 | 代表评测 | 评测核心 | 对平台的启发 |
| --- | --- | --- | --- |
| 终端/系统执行 | Terminal-Bench 2.0、LongCLI-Bench、WildClawBench | 在真实 CLI / Docker 环境中完成长程任务 | 评测底座必须支持独立终端环境、命令轨迹、隐藏 verifier |
| 软件工程 | SWE-Bench Pro、Multi-SWE Bench、SWE-Bench Live | repo 级 bug 修复、多语言代码修改、测试通过率 | 软件工程任务应作为 terminal sandbox 中的一类高置信任务 |
| 工具与工作流 | GAIA、Toolathlon、AutomationBench、ClawBench、MM-ClawBench | 多工具调用、跨系统状态变更、业务流程完成度 | 需要 mock service、状态快照和 policy checker，而不是只看文本回答 |
| 专业生产力 | GDPval-AA、OdysseyBench、MLE-Bench Lite | ML 实验、Excel/PPT/Word、研究报告、专业工作产物 | 需要产物检查、文件渲染、专家 rubric 和部分 LLM Judge |
| 记忆与自我改进 | EverMemBench、EvoAgentBench、MEME | 长期记忆、历史轨迹复用、train/test 后能力提升 | 需要把 trajectory、memory store、skill extraction 纳入评测数据链路 |

这些 benchmark 的共同趋势是：

- 任务越来越接近真实工作，而不是 isolated QA。
- 执行环境越来越重要，尤其是 CLI、文件系统、工具/API 和状态化服务。
- 评分越来越依赖隐藏测试、端状态验证和产物检查。
- 只给总分不够，需要失败类型、成本、轨迹和可复现信息。

因此，本平台不应把每个 benchmark 做成一套孤立系统，而应抽象成一个统一底座：

```text
Terminal / Sandbox Execution Substrate
+ Multi-domain Task Families
+ Verifier-first Scoring
+ Trajectory-based Diagnostics
```

### 3.3 Benchmark 到平台能力的映射

| 外部参考 | 平台吸收的能力 | 是否进入主闭环 | 说明 |
| --- | --- | --- | --- |
| Terminal-Bench 2.0 | 终端任务、Docker 环境、隐藏测试、agent harness 评价 | 是 | 作为平台主架构参考 |
| LongCLI-Bench | 长程 CLI 编程、步骤级失败分析 | 是 | 用于补充 terminal 任务难度和过程指标 |
| WildClawBench | Docker 原生运行、真实工具、语义验证 | 是 | 强化真实环境和多模态/长程任务方向 |
| SWE-Bench Pro | 真实 repo 修复和隐藏测试 | 是 | 落到 Code Repair 任务族 |
| Multi-SWE Bench | 多语言 repo 修复 | 是 | 落到 Multi-language Engineering 任务族 |
| MLE-Bench Lite | 数据处理、训练、实验迭代、提交评分 | 是 | 落到 ML Engineering 任务族，可能需要更长预算 |
| Toolathlon | 大量工具/API 选择和调用 | 部分 | 通过 mock tools 和 API state verifier 接入 |
| AutomationBench | REST API 编排、业务终态验证、policy adherence | 部分 | 通过 mock service + state assertion 接入 |
| GDPval-AA | 专业岗位产物质量 | 部分 | 通过 artifact validator + rubric judge 接入 |
| OdysseyBench | Office 应用长程工作流 | 部分 | 通过文件渲染和办公产物检查接入 |
| MM-ClawBench / ClawBench | 日常/办公多步骤任务 | 部分 | 作为长程任务和工具工作流样例来源 |
| EverMemBench | Add -> Search -> Answer -> Evaluate 记忆管线 | 扩展 | 作为 Memory QA 任务族 |
| EvoAgentBench | train/extract/evaluate 自进化协议 | 扩展 | 作为 Evolution Gain 任务族 |

整体定位是：

```text
Terminal-first execution substrate
+ benchmark-derived task families
+ unified verifier/scoring/reporting
```

### 3.4 平台不直接照搬的部分

调研里的 benchmark 有些设计不适合直接照搬：

| 做法 | 不直接照搬的原因 | 平台处理 |
| --- | --- | --- |
| 只发布数据集和脚本 | 难保证外部结果可复现 | 平台托管运行环境和任务版本 |
| 只展示榜单分 | 无法指导 agent 改进 | 必须输出失败画像和轨迹回放 |
| 大量依赖 LLM Judge | 开放任务评分容易争议 | verifier 优先，judge 只做质量补充 |
| 每类任务独立 harness | 成本高，难统一接入 | 收敛到 terminal/sandbox substrate |
| 默认公开所有结果 | 企业用户顾虑大 | 默认非公开报告，可选公开 |

## 4. 总体架构

平台由六个核心层组成。

```text
+--------------------------------------------------------------+
| User / Customer / Researcher                                  |
+-----------------------------+--------------------------------+
                              |
                              v
+--------------------------------------------------------------+
| Benchmark Portal / API                                        |
| - 注册 agent                                                   |
| - 选择任务集                                                   |
| - 查看非公开报告 / 发布榜单                                      |
+-----------------------------+--------------------------------+
                              |
                              v
+--------------------------------------------------------------+
| Benchmark Orchestrator                                        |
| - 任务调度                                                     |
| - 沙箱生命周期管理                                             |
| - agent 接入适配                                               |
| - 运行预算控制                                                 |
+-----------------------------+--------------------------------+
                              |
          +-------------------+-------------------+
          v                                       v
+-----------------------------+      +---------------------------+
| Sandbox Runtime             |      | Agent Runtime              |
| - Docker / Sandbox           |      | - OpenAgent Adapter         |
| - 文件/进程/网络隔离         |      | - API Action Loop Adapter    |
| - 命令执行                   |      | - 第三方 Agent Adapter       |
+-----------------------------+      +---------------------------+
          |                                       |
          +-------------------+-------------------+
                              v
+--------------------------------------------------------------+
| Evaluation Engine                                             |
| - 隐藏测试                                                     |
| - 状态检查                                                     |
| - 产物检查                                                     |
| - LLM Judge / 人工抽检                                         |
+-----------------------------+--------------------------------+
                              |
                              v
+--------------------------------------------------------------+
| Report & Leaderboard                                          |
| - 能力分                                                       |
| - 失败聚类                                                     |
| - 轨迹回放                                                     |
| - 可选公开榜单                                                 |
+--------------------------------------------------------------+
```

### 4.1 评测门户 / API

对用户提供：

- agent 注册。
- 接入方式选择。
- 任务集选择。
- run 状态查看。
- 非公开报告查看。
- 公开结果确认。

这里不承载具体评测逻辑，只负责用户和项目管理。

### 4.2 评测编排器

平台的调度核心，负责：

- 读取任务集。
- 为任务创建 sandbox。
- 将任务交给 agent adapter。
- 控制超时、资源和并发。
- 收集执行轨迹。
- 调用 verifier。

这是平台最关键的控制面。

### 4.3 Sandbox 运行时

负责提供隔离执行环境。

第一版建议用 Docker：

- 成本低。
- 本机可运行。
- 易于打包任务镜像。
- 易于后续迁移到云端 worker。

正式生产可以升级到 Kubernetes Job 或专用 sandbox 服务。

### 4.4 Agent 运行时

负责把不同 agent 接到统一评测循环中。

支持两类主接入：

| 接入方式 | 用途 |
| --- | --- |
| 容器 / 本地 Adapter | 正式评测、可复现运行、接近 Terminal-Bench 官方口径 |
| API Action Loop | 企业远程 agent 接入、平台化服务接入 |

不支持把普通 chat API 直接当作 terminal agent。普通 chat API 只能用于问答类评测，不能完成 Terminal-Bench 类任务。

### 4.5 评测引擎

负责评分。

输入：

- sandbox 最终状态。
- agent 执行轨迹。
- 文件 diff。
- 产物。
- verifier 输出。

输出：

- task score。
- pass/fail。
- failure tags。
- cost/latency。
- report summary。

### 4.6 报告与排行榜

默认生成非公开报告。公开榜单需要用户显式确认，并且只展示聚合信息。

## 5. 终端任务执行闭环

一个任务的完整生命周期如下：

```text
Task Selected
  -> Sandbox Created
  -> Assets Mounted
  -> Agent Started
  -> Task Instruction Delivered
  -> Agent/LLM Generates Action
  -> Action Executed in Sandbox
  -> Observation Returned
  -> Agent Continues
  -> Agent Finishes or Times Out
  -> Verifier Runs
  -> Score Aggregated
  -> Report Generated
  -> Sandbox Destroyed
```

### 5.1 角色交互

```text
Benchmark Orchestrator
    |
    | task instruction
    v
Agent Adapter
    |
    | prompt + observation
    v
LLM
    |
    | next action
    v
Agent Adapter
    |
    | execute command
    v
Sandbox Terminal
    |
    | stdout / stderr / exit code
    v
Agent Adapter
    |
    | continue until finish / timeout
    v
Evaluation Engine
```

### 5.2 终端动作

Terminal Action 是 agent 对执行环境的下一步操作。

最小 action 集：

| Action | 说明 |
| --- | --- |
| run_command | 在沙箱中执行 shell 命令 |
| finish | agent 声明任务完成 |

扩展 action 集：

| Action | 说明 |
| --- | --- |
| read_file | 读取文件 |
| write_file | 写文件 |
| edit_file | 局部修改文件 |
| list_files | 列目录 |
| submit_artifact | 提交最终产物 |

为了贴近 Terminal-Bench 2.0，最小可行实现可以只支持 `run_command` 和 `finish`。文件操作可以通过 shell 命令完成。

### 5.3 观察结果

每次 action 后，sandbox 返回 observation。

至少包括：

- command。
- stdout。
- stderr。
- exit code。
- working directory。
- duration。
- timeout status。
- output truncation status。

Observation 是下一轮 LLM 决策的主要输入，也是报告回放的基础。

## 6. 任务模型与生产级标准

平台任务不应是 demo prompt，而应是可验证的生产级工作单元。

### 6.1 任务组成

一个任务至少包含：

| 组成 | 说明 |
| --- | --- |
| instruction | 给 agent 的任务说明 |
| environment | sandbox 镜像或环境定义 |
| visible assets | agent 可见的代码、数据、文档、配置 |
| hidden verifier | agent 不可见的测试或检查脚本 |
| budget | 时间、步骤、成本、资源限制 |
| scoring rule | 任务如何计分 |
| metadata | 难度、能力维度、来源、版本 |

### 6.2 生产级任务标准

正式任务应满足：

- 有真实文件或系统状态。
- 需要多步骤推进。
- 需要根据反馈调整策略。
- 最终结果可验证。
- 强弱 agent 有区分度。
- 重复运行结果稳定。
- 不暴露隐藏答案。
- 任务来源和许可证可审计。

### 6.3 任务族设计

Terminal-first 并不等于只评 Linux 命令。平台应把调研中的 benchmark 统一成若干任务族，每个任务族共享一套执行和评分模式。

| 任务族 | 参考 benchmark | 例子 | 验证方式 |
| --- | --- | --- | --- |
| Terminal Operations | Terminal-Bench 2.0、LongCLI-Bench | 修复损坏环境、恢复服务、处理日志并生成结果 | hidden script、service health、file check |
| Code Repair | SWE-Bench Pro、SWE-Bench Live | 修复 repo bug，让测试通过 | hidden tests、patch diff、regression tests |
| Multi-language Engineering | Multi-SWE Bench | 跨 Python/TS/Go/Rust/Java 项目修复问题 | language-specific test suite |
| ML Engineering | MLE-Bench Lite | 处理数据、训练模型、提交预测文件 | hidden score、submission validator |
| Tool Workflow | Toolathlon、AutomationBench、GAIA | 调用 mock CRM/Calendar/DB/API 完成业务终态 | state assertion、policy checker |
| Office Delivery | GDPval-AA、OdysseyBench | 生成 xlsx/pptx/docx/研究报告 | artifact validator、render check、rubric judge |
| Long-horizon Work | MM-ClawBench、ClawBench、WildClawBench | 多轮、多约束、跨工具完成真实工作流 | milestone checks、final state、judge |
| Memory & Evolution | EverMemBench、EvoAgentBench、MEME | 记忆问答、历史轨迹复用、skill gain | memory accuracy、retrieval recall、train/test delta |

这些任务族共享同一个执行骨架：

```text
environment + instruction + visible assets + budget
-> agent actions in sandbox
-> verifier / artifact check / state check
-> task score + failure tags
```

区别只在于任务资产、工具服务和 verifier 类型。

### 6.4 任务准入流程

```text
Candidate Task
  -> Environment Build
  -> Verifier Build
  -> Baseline Run
  -> Stability Check
  -> Leakage Check
  -> Suite Inclusion
```

准入要求：

- 至少一个 baseline 能完成。
- 至少一个弱 baseline 失败。
- verifier 连续运行稳定。
- 任务说明不泄漏答案。
- 运行成本可控。

## 7. 沙箱与执行环境

### 7.1 为什么需要沙箱

不用沙箱也能做 POC，但不适合作为正式评测。

| 运行方式 | 适用 | 风险 |
| --- | --- | --- |
| 本机终端 | 快速调试 | 不安全、不可复现、污染本地环境 |
| 固定 VM | 内部灰度 | 状态清理和并发隔离较弱 |
| Docker / Sandbox | 正式评测 | 推荐 |

沙箱带来的能力：

- 任务隔离。
- 环境复现。
- 资源控制。
- 隐藏验证。
- 运行后清理。
- 安全边界。

### 7.2 Docker 执行单元

第一版每个任务一个 Docker 容器即可满足基本闭环。

容器中应包含：

- 操作系统。
- 任务工作目录。
- 项目代码和数据。
- 必要运行时和工具。
- agent 可见说明。

容器外部保留：

- 隐藏 verifier。
- 任务调度。
- 报告生成。
- 用户密钥。

任务结束后：

- 收集轨迹、日志、产物和 verifier 输出。
- 销毁容器。
- 保留报告数据。

### 7.3 本机可行性

当前机器 Docker 可用，适合做第一阶段 POC 和小规模并发。

建议策略：

- 并发 1-2 起步。
- 先跑 3-5 个 smoke task。
- 再扩展到 10-20 个任务。
- 全量或多次重复评测迁移到云端 worker。

## 8. Agent 接入模式

### 8.1 容器 / 本地适配器

这是正式评测推荐方式。

OpenAgent 或其他 agent 以 adapter 形式运行在评测环境中，由 orchestrator 调用。

特点：

- 接近 Terminal-Bench 官方 custom agent 方式。
- 轨迹完整。
- 可复现性强。
- 更适合榜单和严肃评测。

OpenAgent 需要提供：

- 一个可被 benchmark harness import 或启动的 adapter。
- 能接收 task instruction。
- 能通过 AgentLoop 调用 LLM。
- 能执行 terminal action。
- 能把轨迹写入指定目录。

### 8.2 API 动作循环

API 接入可以支持，但必须是 action loop，不是普通 chat API。

流程：

```text
Orchestrator
  -> POST task + observation
  -> OpenAgent API
  <- next action
  -> execute action in sandbox
  -> POST observation
  <- next action
  ...
  <- finish
```

API 返回的应该是：

- 下一条命令。
- 工作目录。
- 超时设置。
- 是否完成。

而不是一段自然语言答案。

API 接入适合：

- 企业已有远程 OpenAgent 服务。
- 不方便把 agent 镜像交给平台。
- 平台化产品接入。

限制：

- 网络延迟会影响结果。
- 远程 API 不能直接拿到隐藏文件。
- 需要严格权限和输出脱敏。
- 分数评价的是“远程 agent + 平台 sandbox executor”的组合。

### 8.3 普通 Chat API

普通 Chat API 不适合 Terminal-first 评测。

原因：

- 不能真实执行命令。
- 不能根据 stdout/stderr 迭代。
- 不能修改文件或系统状态。
- 不能通过隐藏测试验证真实完成度。

它只能用于问答类 benchmark，不能代表 agent 的终端工作能力。

## 9. OpenAgent 接入设计

OpenAgent 当前已有：

- AgentLoop。
- UniversalAgent。
- bash / file / edit / grep / glob / ls 工具。
- 权限系统。
- 本地 workspace runtime。
- 工具轨迹和观测能力。

需要补齐：

### 9.1 终端评测适配器

提供一个稳定入口，让外部 benchmark 可以把 OpenAgent 当作 agent 调用。

职责：

- 创建 session。
- 初始化模型/provider。
- 配置工具和权限。
- 接收 task instruction。
- 驱动 AgentLoop。
- 将 stream event 转为 benchmark 日志。
- 处理 finish/timeout。

### 9.2 评测权限配置

当前产品权限策略偏安全保守。Benchmark 模式需要在一次性沙箱内允许更多操作。

建议 profile：

- 允许 bash、read、write、edit、grep、glob、ls。
- 禁止访问沙箱外部。
- 禁止读取隐藏 verifier。
- 记录危险命令。
- 网络默认关闭或 allowlist。

### 9.3 终端运行时适配器

短期：

- OpenAgent 在任务容器内部本地执行 bash。
- 用 Docker 隔离环境。

长期：

- 将 OpenAgent 命令执行转发到 benchmark 提供的 terminal session。
- 支持更完整的 session state、tmux 交互和日志捕获。

### 9.4 长程任务支持

需要调整：

- 更高 max steps。
- 更长命令 timeout。
- 大输出裁剪和日志落盘。
- 更强 loop detection。
- 任务级成本和时间预算。

## 10. 评分体系

### 10.1 主指标

主指标是任务完成率：

```text
completion_rate = passed_tasks / total_tasks
```

不同任务可以按难度、时长、能力维度加权，但公开口径要固定。

### 10.2 辅助指标

| 指标 | 说明 |
| --- | --- |
| command_count | 命令数 |
| invalid_command_rate | 无效命令比例 |
| retry_rate | 重复重试比例 |
| time_to_first_valid_signal | 首次有效验证信号耗时 |
| verification_behavior | 是否主动运行测试或检查 |
| recovery_rate | 出错后的恢复能力 |
| safety_violation_rate | 越权、泄密、危险操作 |
| cost | token 与运行成本 |

### 10.3 失败分类

| Failure Tag | 说明 |
| --- | --- |
| task_understanding_error | 误解任务目标 |
| environment_misread | 误读目录、依赖、系统状态 |
| command_error | 命令选择或参数错误 |
| no_verification | 修改后未验证 |
| timeout | 超时或陷入无效循环 |
| artifact_invalid | 产物缺失或格式错误 |
| verifier_failed | 最终隐藏验证失败 |
| safety_violation | 越权或泄漏 |

## 11. 报告形态

报告分为四层。

### 11.1 总览

展示：

- 总分。
- Terminal 完成率。
- 任务数。
- 平均耗时。
- 平均命令数。
- 安全违规数。

### 11.2 能力面板

Terminal 是主轴，其他维度作为辅助。

| 能力维度 | 主/辅 | 参考 | 解释 |
| --- | --- | --- | --- |
| Terminal Operations | 主 | Terminal-Bench 2.0、LongCLI-Bench | 在终端环境中完成长程系统任务 |
| Code Repair | 主 | SWE-Bench Pro、SWE-Bench Live | 修复真实 repo 问题 |
| Multi-language Engineering | 辅 | Multi-SWE Bench | 跨技术栈工程修复 |
| ML Engineering | 辅 | MLE-Bench Lite | 数据、训练、实验、提交 |
| Tool Workflow | 辅 | Toolathlon、AutomationBench、GAIA | 工具/API 编排和业务终态 |
| Office Delivery | 辅 | GDPval-AA、OdysseyBench | 文件、报告和专业产物质量 |
| Long-horizon Work | 辅 | MM-ClawBench、ClawBench、WildClawBench | 多轮、多约束真实工作流 |
| Memory / Evolution | 辅 | EverMemBench、EvoAgentBench、MEME | 长期记忆和经验复用 |

对外展示时不需要暴露每道任务，但需要保留 benchmark lineage。也就是说，用户看到的是能力维度分，平台内部知道这些分数来自哪些任务族、哪些参考 benchmark、哪些 verifier。

### 11.3 失败画像

按 failure tag 聚合，而不是只列原始日志。

示例：

| 失败类型 | 占比 | 解释 |
| --- | ---: | --- |
| environment_misread | 23% | 目录或依赖理解错误 |
| no_verification | 18% | 修改后没有跑测试 |
| timeout | 14% | 重复尝试或等待过久 |
| verifier_failed | 31% | 最终隐藏测试未通过 |

### 11.4 轨迹回放

每个任务保留：

- 命令序列。
- stdout/stderr 摘要。
- 文件变更。
- verifier 输出。
- agent final answer。

用于开发者定位 agent 失败原因。

## 12. 安全与合规

生产级评测默认按不可信 agent 处理。

必须保证：

- 每个任务独立容器。
- 容器默认无宿主机敏感挂载。
- hidden verifier 不暴露。
- secret 不进入容器或日志。
- 网络默认关闭，按任务 allowlist。
- 输出日志脱敏。
- 任务结束销毁容器。

API agent 额外要求：

- 只回传必要 observation。
- 命令执行前做策略检查。
- stdout/stderr 截断和脱敏。
- 不把隐藏文件内容发送给远程 agent。

## 13. 实施阶段

### 13.1 阶段 1：本机 Docker POC

目标：跑通最小闭环。

范围：

- 每个任务一个 Docker 容器。
- 3-5 个 smoke task。
- OpenAgent 本地 adapter。
- bash/file 工具。
- 简单 verifier。
- 生成基础报告。

验收：

- OpenAgent 能在容器内执行任务。
- 能记录命令和输出。
- 能跑验证脚本。
- 容器结束后可清理。

### 13.2 阶段 2：Terminal-Bench 兼容

目标：接近 Terminal-Bench 2.0 运行方式。

范围：

- custom agent adapter。
- benchmark permission profile。
- 长 timeout / max steps。
- 完整日志目录。
- 小子集任务运行。

验收：

- 能用 Terminal-Bench/Harbor 类方式调用 OpenAgent。
- 能跑 10-20 个任务并产出稳定报告。

### 13.3 阶段 3：平台化接入

目标：支持外部 agent 接入。

范围：

- API action loop。
- 容器接入。
- 任务队列。
- 非公开报告。
- 多能力维度汇总。

验收：

- 用户能注册 agent。
- 能选择任务集。
- 能获得非公开报告。

### 13.4 阶段 4：生产级评测

目标：支持更正式的 benchmark 运行。

范围：

- 云端 worker。
- 并发控制。
- 任务版本管理。
- 防污染策略。
- 可选榜单。

验收：

- 任务可复现。
- 分数可解释。
- 结果可审计。

## 14. 本机可行性与运行成本

当前机器 Docker 可用，适合第一阶段 POC。

建议：

- smoke：3-5 个任务，10-30 分钟。
- 小集：10-20 个任务，1-3 小时。
- 中集：30-50 个任务，4-10 小时。
- 全量 89 个 Terminal-Bench 2.0 风格任务，本机可能需要 12-30 小时或更久。

当前本机更适合：

- 验证架构闭环。
- 观察失败类型。
- 调整 OpenAgent adapter。
- 小规模回归。

正式全量评测建议迁移到云端 worker。

## 15. 风险与决策

| 风险 | 影响 | 决策 |
| --- | --- | --- |
| 不使用沙箱 | 不可复现且不安全 | 正式评测必须使用沙箱 |
| 只接普通 API | 无法真实执行任务 | API 必须是 action loop |
| 任务过简单 | 分数没有区分度 | 任务必须有 baseline 和弱 baseline 检查 |
| verifier 不稳定 | 结果争议 | verifier 需多次稳定性测试 |
| agent 读隐藏测试 | 分数失真 | 隐藏验证隔离 |
| 本机资源有限 | 无法全量高并发 | POC 本机，生产云端 worker |

## 16. 结论

本设计的核心不是给 agent 出问答题，而是构建一个生产级终端任务评测系统。

最终目标：

```text
让 agent 在受控沙箱中完成真实任务，
用隐藏验证给出可信分数，
用轨迹和失败画像解释能力边界。
```

OpenAgent 接入时，应优先实现容器/本地 adapter 跑通 Terminal-first 闭环，再扩展 API action loop 和平台化接入。

## 17. 参考来源

- [Terminal-Bench 2.0 leaderboard](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- [Terminal-Bench agent interface](https://www.tbench.ai/docs/agent-introduction)
- [Terminal-Bench GitHub repository](https://github.com/harbor-framework/terminal-bench)
- [Epoch AI: Terminal-Bench](https://epoch.ai/benchmarks/terminal-bench)
- [MiniMax M2.7 benchmark disclosure](https://www.minimax.io/news/minimax-m27-en)
- [GAIA dataset](https://huggingface.co/datasets/gaia-benchmark/GAIA)
- [WildClawBench](https://hf.co/papers/2605.10912)
- [ClawBench](https://hf.co/papers/2604.08523)
- [AutomationBench](https://hf.co/papers/2604.18934)
- [LongCLI-Bench](https://hf.co/papers/2602.14337)
- [OdysseyBench](https://hf.co/papers/2508.09124)
- [EverOS](https://github.com/EverMind-AI/EverOS)
- [EvoAgentBench](https://huggingface.co/datasets/EverMind-AI/EvoAgentBench)
- [EverMemBench-Dynamic](https://huggingface.co/datasets/EverMind-AI/EverMemBench-Dynamic)
