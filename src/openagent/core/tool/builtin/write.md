# write

写入 UTF-8 文本到文件（覆盖写）。

## 参数
- `file_path`（必填，string）：文件路径（相对 `session_root` 或绝对路径）
- `content`（必填，string）：要写入的内容

## 注意
- 如果文件存在，会覆盖原内容
- 如果父目录不存在，会自动创建

## 约束
- 只能写入 `session_root` 内的路径（越界会报错）

