# OpenAgent 文档目录

本文档按功能整理 `openagent/doc` 下的设计文档。这里的显示标题统一使用中文，不在标题中附带创建时间或更新时间。`SKILL.md` 属于 skill 定义文件，不纳入本目录重命名和中文化规则。

## 项目总览与维护

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 项目技术文档](openagent-project-doc.md) | 项目整体架构、主执行链路、工具系统、权限、上下文预算、MCP、Skill 和 SDK 入口 | 快速了解 OpenAgent 全貌 |
| [OpenAgent 整改清单](remediation-plan.md) | 安装、运行、测试、文档一致性和维护体验的整改项 | 做项目健康度梳理或补工程债 |

## 上下文工程

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 上下文优化专项](context-optimization-initiative.md) | 结构化 compaction、上下文渲染、预算降级、兼容策略和测试验证 | 准备讲 Context Engineering |
| [结构化工作状态压缩设计](structured-work-state-compaction-design.md) | 把自由文本摘要升级成可恢复、可诊断、可投影的结构化工作状态 | 解释长任务恢复和上下文压缩 |
| [OpenAgent 对标 opencode 的上下文优化路线](opencode-context-gap-roadmap.md) | 对标 opencode 的 session、rules、skills、subagent 和 compaction 能力 | 做竞品对标或后续路线规划 |
| [OpenAgent 对标 Agent Runtime 的上下文工程加固路线](agent-runtime-context-engineering-hardening-roadmap.md) | 梳理 Agent Runtime 长上下文工程，抽象 Context Engineering 概念，并规划 OpenAgent 一个月加固项 | 做 Harness review专项和 OpenAgent 上下文工程排期 |

## 执行运行时与 Sandbox

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [Remote Sandbox 执行后端设计](remote-sandbox-runtime.md) | OpenAgent core 如何把 workspace 工具从本地执行切到 OpenSandbox 执行 | 解释远端工具执行链路 |
| [Remote Sandbox 生命周期设计](remote-sandbox-lifecycle.md) | Session 与 Sandbox 实例的一对一绑定、创建、查询、更新、删除和renewal | 设计沙箱生命周期管理服务 |

## 工具与 Web Research

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 网页研究收敛设计](web-research-convergence-design.md) | 减少 `web_search` 与 `web_fetch` 来回摆动，让搜索链路在单轮内收敛 | 优化研究型问题的工具调用质量 |

## 可观测性、日志与评测

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [P0 可观测性与评测设计](observability-eval-design.md) | trace、事件、JSONL、eval case、本地报告和 replay 摘要 | 建立生产级可观测和评测闭环 |
| [生产运行时日志设计](production-runtime-logging-design.md) | 面向运维排障的结构化日志，与 trace/eval 事件区分职责 | 排查线上任务、工具失败和权限问题 |
| [终端优先的 Agent 评测系统架构设计](agent-benchmark-evaluation-platform-design.zh.md) | Terminal-first 任务、Sandbox 运行、Verifier、排行榜和 OpenAgent 接入 | 设计 Agent 评测系统 |

## review与能力补强

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [Agent 架构师能力准备路线图](agent-architect-readiness-roadmap.md) | 面向 Agent 架构师岗位的优势、差距和 3-6 个月建设计划 | 准备高阶 Agent/Harness review |
| [KV Cache 与推理底层补强路线图](kv-cache-inference-learning-roadmap.md) | KV Cache、PagedAttention、Continuous Batching、Prefix Cache 与 Harness 的结合 | 补齐推理底层短板 |

## 推荐阅读顺序

1. 先读 [OpenAgent 项目技术文档](openagent-project-doc.md)，建立整体地图。
2. 再读 [OpenAgent 上下文优化专项](context-optimization-initiative.md) 和 [结构化工作状态压缩设计](structured-work-state-compaction-design.md)，抓住 OpenAgent 的上下文主线。
3. 接着读 [Remote Sandbox 执行后端设计](remote-sandbox-runtime.md) 和 [Remote Sandbox 生命周期设计](remote-sandbox-lifecycle.md)，把工具执行和远端沙箱讲顺。
4. 然后读 [P0 可观测性与评测设计](observability-eval-design.md)、[生产运行时日志设计](production-runtime-logging-design.md) 和 [终端优先的 Agent 评测系统架构设计](agent-benchmark-evaluation-platform-design.zh.md)，形成生产化与评测闭环。
5. 最后读 [Agent 架构师能力准备路线图](agent-architect-readiness-roadmap.md) 和 [KV Cache 与推理底层补强路线图](kv-cache-inference-learning-roadmap.md)，整理成review表达。
