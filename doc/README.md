# OpenAgent 文档目录

本文档按功能整理 `openagent/doc` 下适合公开展示的设计文档。这里不收录个人准备材料、未脱敏方案、真实环境信息或非公开调研记录。

## 项目总览与维护

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 项目技术文档](openagent-project-doc.md) | 项目整体架构、主执行链路、工具系统、权限、上下文预算、MCP、Skill 和 SDK 入口 | 快速了解 OpenAgent 全貌 |
| [OpenAgent 整改清单](remediation-plan.md) | 安装、运行、测试、文档一致性和维护体验的整改项 | 做项目健康度梳理或补工程债 |

## 上下文工程

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 上下文优化专项](context-optimization-initiative.md) | 结构化 compaction、上下文渲染、预算降级、兼容策略和测试验证 | 理解长任务上下文恢复 |
| [结构化工作状态压缩设计](structured-work-state-compaction-design.md) | 把自由文本摘要升级成可恢复、可诊断、可投影的结构化工作状态 | 解释 compaction schema 与 fallback 策略 |
| [ContextPackBuilder、InstructionContextLoader 与 FileContextState 设计](context-pack-builder-instructions-file-context-design.md) | 将 runtime、消息、todo、指令文件、文件状态和 sandbox metadata 收敛成可诊断 context item | 设计统一上下文组装管线 |

## 工具与 Web Research

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [OpenAgent 网页研究收敛设计](web-research-convergence-design.md) | 减少 `web_search` 与 `web_fetch` 来回摆动，让搜索链路在单轮内收敛 | 优化研究型问题的工具调用质量 |

## 可观测性、日志与评测

| 文档 | 功能定位 | 适合什么时候看 |
| --- | --- | --- |
| [可观测性与评测设计](observability-eval-design.md) | trace、事件、JSONL、eval case、本地报告和 replay 摘要 | 建立可观测和评测闭环 |
| [生产运行时日志设计](production-runtime-logging-design.md) | 面向运维排障的结构化日志，与 trace/eval 事件区分职责 | 排查运行时任务、工具失败和权限问题 |
| [终端优先的 Agent 评测系统架构设计](agent-benchmark-evaluation-platform-design.zh.md) | Terminal-first 任务、Sandbox 运行、Verifier、报告和 OpenAgent 接入 | 设计 Agent 评测系统 |

## 推荐阅读顺序

1. 先读 [OpenAgent 项目技术文档](openagent-project-doc.md)，建立整体地图。
2. 再读 [OpenAgent 上下文优化专项](context-optimization-initiative.md)、[结构化工作状态压缩设计](structured-work-state-compaction-design.md) 和 [ContextPackBuilder 设计](context-pack-builder-instructions-file-context-design.md)，抓住 OpenAgent 的上下文主线。
3. 接着读 [可观测性与评测设计](observability-eval-design.md)、[生产运行时日志设计](production-runtime-logging-design.md) 和 [终端优先的 Agent 评测系统架构设计](agent-benchmark-evaluation-platform-design.zh.md)，形成运行、诊断和评测闭环。
