# bash

在 `session_root` 目录下执行一条 shell 命令，并返回输出（stdout + stderr）。

## 参数
- `command`（必填，string）：要执行的命令
- `timeout`（可选，integer，默认 `120000`）：超时毫秒数
- `workdir`（可选，string，默认空）：工作目录（相对 `session_root` 或绝对路径）

## 约束与安全
- `workdir` 必须位于 `session_root` 内（越界会报错）
- `command` 本身不会被解析/改写；请结合 `PermissionManager` 的规则集限制危险命令

## 输出
- 返回命令输出文本（stdout + stderr）
- 输出过长会被截断；完整输出会落盘到 `session_root/.openagent/tool_output/<call_id>.txt`

