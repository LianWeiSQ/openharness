# OpenAgent 整改清单

> 梳理日期：2026-05-09
> 范围：项目文档、运行入口、依赖声明、测试与已落地代码的一致性

本文档记录本次项目梳理中发现的整改项。优先级含义：

- P0：会导致安装、运行或测试直接失败的问题
- P1：会误导开发或影响核心能力稳定性的问题
- P2：文档、结构或维护体验类优化

## P0：先保证项目能安装、能跑、能测

### 1. 补齐依赖声明

现状：

- `pyproject.toml` 声明了 `beautifulsoup4`、`markdownify`、`PyYAML`、`tiktoken`、`mcp`、`opensandbox`。
- `core/mcp/runtime.py` 直接导入 `httpx`，但 `httpx` 没有写入依赖。
- 当前本机未安装依赖时，`PYTHONPATH=src python src/examples/run_mock.py` 因 `yaml` 缺失失败。

建议：

- 在 `pyproject.toml` 中补充 `httpx`。
- 重新安装：`python -m pip install -e .`。
- 如果 OpenSandbox 不是所有场景都需要，可考虑拆成 extra，例如 `.[sandbox]`，但当前代码路径已默认依赖 `opensandbox` 包名。

验收：

```bash
python -m pip install -e .
PYTHONPATH=src python src/examples/run_query_only.py "你好"
```

### 2. 修复或移除缺失的 `legacy_cli.py`

现状：

- `src/tests/test_legacy_cli.py` 导入 `agent_cli`。
- 仓库内没有 `legacy_cli.py`。
- 完整测试会在收集阶段失败。

建议二选一：

- 恢复 `legacy_cli.py`，包含 CLI、Web Console、MCP demo、问题回复等测试期望的接口。
- 如果 CLI/Web Console 已不属于 core 范围，则删除或迁移 `src/tests/test_legacy_cli.py`，并把相关能力移到独立项目测试。

验收：

```bash
PYTHONPATH=src python -m unittest src/tests/test_legacy_cli.py
```

### 3. 修正示例脚本中的仓库根定位与工作目录

现状：

- `src/examples/run_mock.py`、`run_dashscope_universal.py`、`run_query_only.py` 的 `_find_repo_root()` 仍检查 `(p / "openagent").exists()`。
- 当前仓库没有 `openagent/` 子目录，真实源码是 `src/openagent/`。
- 示例里的工作目录仍写成 `openagent/examples/...`。

建议：

- 仓库根判断改为 `(p / "pyproject.toml").exists() and (p / "src" / "openagent").is_dir()`。
- 示例工作目录改到 `src/examples/workdir_*` 或 `.openagent/workdir_*`。
- README 中只保留当前可执行命令。

验收：

```bash
PYTHONPATH=src python src/examples/run_query_only.py "你好"
PYTHONPATH=src python src/examples/run_mock.py
```

### 4. 修正 OpenSandbox runtime 测试与实现不一致

现状：

- `OpenSandboxWorkspaceRuntime.glob()` 调用 `sandbox.files.search(SearchEntry(...))`。
- `src/tests/test_execution_runtime.py` 的 fake `search(self, entries)` 按可迭代 entries 处理，导致 `FakeSearchEntry object is not iterable`。

建议：

- 如果 SDK 接口真实签名是单个 `SearchEntry`，修测试 fake。
- 如果 SDK 接口真实签名是 list，则修 runtime 调用。
- 同步更新 `doc/remote-sandbox-runtime.md` 中的 SDK 调用说明。

验收：

```bash
PYTHONPATH=src python -m unittest src/tests/test_execution_runtime.py
```

## P1：让核心能力和文档保持一致

### 5. 明确 Python 版本支持范围

现状：

- `pyproject.toml` 写的是 `requires-python = ">=3.10"`。
- 当前本机测试使用 Python 3.14.4，部分第三方依赖或测试行为可能尚未完全验证。

建议：

- 在 README 中声明推荐开发版本，例如 Python 3.10-3.12。
- CI 覆盖项目实际支持版本。
- 如果确认支持 3.14，再纳入测试矩阵。

### 6. 处理根目录 `__init__.py` 的 bootstrap 说明

现状：

- 根目录 `__init__.py` 写着“从 repo root 直接 import openagent”，并引用旧路径 `openagent/src/openagent/`。
- 当前仓库目录名是 `openagent-ai`，这个文件不会让 `import openagent` 自动生效。

建议：

- 删除根目录 `__init__.py`，避免误导。
- 或改成真正可用的开发入口，但更推荐使用 editable install 或 `PYTHONPATH=src`。

### 7. 清理过期文档

现状：

- `CLAUD.md` 仍描述“仅依赖标准库”“OpenAI Provider stub”“Web 工具 stub”“MCP 适配器 stub”等旧状态。
- `Agent.md` 基本对齐当前代码，旧路径已在本次文档整理中修正。
- `doc/remote-sandbox-runtime.md` 有旧 Windows 绝对路径链接。

建议：

- `CLAUD.md` 保留为历史归档说明，避免作为开发依据。
- 修正 `Agent.md` 和专题文档中的旧路径。
- README 作为唯一入口索引。

### 8. 把 Storage 接入主执行链路或降低文档表达

现状：

- `StorageBase`、`InMemoryStorage`、`JsonFileStorage` 存在。
- `AgentLoop` 与 `Session` 主链路未使用它们做持久化。

建议：

- 如果需要会话恢复，设计并实现存取时机。
- 如果短期不用，在文档里明确“预留抽象，未接入主链路”。

### 9. 明确 `PermissionAction.ASK` 的宿主接线责任

现状：

- Ruleset 可以返回 ASK。
- 没有 `ask_user_func` 时会抛 `PermissionAskRequiredError`，Loop 会把它变成工具错误。

建议：

- 在 SDK 或宿主接入文档中补一个最小 `ask_user_func` 示例。
- CLI/Web 恢复后，把危险工具确认路径接起来。

## P2：维护体验和结构优化

### 10. 精简并统一文档职责

建议职责：

- `README.md`：入口、安装、运行、测试、文档索引、已知限制
- `doc/openagent-project-doc.md`：当前代码事实和模块说明
- `doc/remediation-plan.md`：待整改事项
- `Agent.md`：深架构分析
- `doc/remote-sandbox-runtime.md`：OpenSandbox 专题
- `doc/web-research-convergence-design.md`：Web 研究收敛专题
- `CLAUD.md`：历史归档，不作为当前依据

### 11. 给 Provider 与工具能力增加 smoke tests

建议：

- `OpenAIProvider` 和 `DashScopeProvider` 保留 mock HTTP/SSE 测试。
- 示例脚本至少有一个无需外部 key 的 smoke path。
- Web search 对无 API key、配额错误、429、HTML 错误页做稳定断言。

### 12. 明确 MCP 依赖是否属于核心依赖

现状：

- `ToolkitAdapter.register_mcp()` 和 `core/mcp/runtime.py` 已进入 core。
- `mcp` 和 `httpx` 是 MCP runtime 的硬依赖。

建议：

- 如果 MCP 是核心能力，保留硬依赖并补齐 `httpx`。
- 如果希望 core 更轻，改成 extra，并延迟导入 MCP runtime。

### 13. 清理生成物和系统文件

现状：

- git status 显示 `.DS_Store`、`skills/.DS_Store`。
- `src/openagent_core.egg-info/` 已在仓库中。

建议：

- 将 `.DS_Store` 加入 `.gitignore`，不要提交。
- 确认 egg-info 是否需要入库；如不需要，移除并加入 ignore。

## 本次已完成的文档整改

- 重写 `README.md`，改为当前 `src/` 布局、真实安装/运行/测试说明。
- 重写 `doc/openagent-project-doc.md`，按当前代码整理项目总览、架构、模块和扩展方式。
- 新增本整改清单。
