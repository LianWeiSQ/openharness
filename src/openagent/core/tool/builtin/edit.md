# edit

在文件中把 `old_string` **第一次出现**替换为 `new_string`。

## 参数
- `file_path`（必填，string）：文件路径（相对 `session_root` 或绝对路径）
- `old_string`（必填，string）：要替换的字符串
- `new_string`（必填，string）：替换后的字符串

## 建议
- 为了避免误替换，建议先用 `read` 查看上下文，再提供足够长且唯一的 `old_string`

## 约束
- 只能编辑 `session_root` 内的路径（越界会报错）

