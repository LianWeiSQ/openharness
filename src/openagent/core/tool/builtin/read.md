# read

读取一个 UTF-8 文本文件的内容（按行编号，可分页）。

## 参数
- `file_path`（必填，string）：文件路径（相对 `session_root` 或绝对路径）
- `offset`（可选，integer，默认 `0`）：起始行号（从 0 开始）
- `limit`（可选，integer，默认 `2000`）：最多读取行数

## 输出
- 使用 `<file>...</file>` 包裹
- 每行带行号前缀，例如：`00001| hello`

## 约束
- 只能读取 `session_root` 内的路径（越界会报错）

