# OpenAgent (core)

本目录是 `Agent.md` 设计文档对应的 Python 核心实现（Provider / Permission / Tool / Session / AgentLoop）。

## 目录结构

源码位于 `openagent/src/openagent/`，与 `Agent.md` 的推荐结构一致：

- `core/loop/`：`AgentLoop` + snapshot / retry / doom-loop
- `core/permission/`：`PermissionManager` + ruleset
- `core/tool/`：`ToolkitAdapter` + builtin tools（file/shell/web/search/memory/todo）
- `core/session/`：`Session`（messages/todos + fork/revert）
- `core/provider/`：Provider 接口（内置 provider 为 stub）
- `adapter/`：对接层（AgentAdapter / MemoryAdapter 等）

## 最小运行示例

`AgentLoop` 依赖一个实现了 `LanguageModel.stream()` 的模型适配器（见 `openagent/core/provider/base.py` 的 `LanguageModel` 协议）。

你可以参考 `openagent/tests/_mock_model.py` 的 `ScriptedLanguageModel` 来对接真实 LLM（OpenAI/Anthropic/Gemini 等）。

### 方式 A：临时用 `PYTHONPATH`

```powershell
$env:PYTHONPATH="openagent/src"
python openagent/src/examples/run_mock.py
```

### 方式 A2：仓库内直接运行（不安装）

直接运行示例脚本（脚本会自动把仓库根目录加入 `sys.path`，并通过 `openagent/__init__.py` bootstrap 补齐 `src` 路径）：

```powershell
python openagent/src/examples/run_mock.py
```

### 方式 B：安装为包（推荐）

```powershell
pip install -e openagent
python openagent/src/examples/run_mock.py
```

## 与 Web 的边界

`web` 作为 HTTP runtime 服务层，应该通过已安装的 `openagent-core` 包来消费 core。

约束如下：

- `web` 及其他正式消费方不得依赖仓库根的 repo-local bootstrap 壳
- `web` 应统一从 `openagent.sdk.http_runtime` 导入其运行时所需能力
- 仓库根的兼容壳只用于 core 自己的示例和仓库内直跑

## 运行测试

```powershell
$env:PYTHONPATH="openagent/src"
python -m unittest discover -s openagent/src/tests -p "test_*.py"
```

