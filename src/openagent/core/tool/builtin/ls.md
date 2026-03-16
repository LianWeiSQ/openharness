# ls

列出指定目录下的文件与子目录（目录优先）。

## 参数
- `path`（可选，string）：目录路径（默认 `session_root`）
- `ignore`（可选，array[string]）：要忽略的文件名 glob 列表（例如 `["node_modules", "*.pyc"]`）

## 输出
- 每行格式：`d| -  <size>  <name>`
  - `d` 表示目录，`-` 表示文件
  - `size` 仅对文件统计（目录为 0）

## 约束
- 只能列出 `session_root` 内的目录（越界会报错）

