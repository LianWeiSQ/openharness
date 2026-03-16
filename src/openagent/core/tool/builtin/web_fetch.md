# web_fetch

抓取一个 URL 的内容。

## 说明
- 当前仓库默认运行环境可能禁网，因此该工具默认未启用（调用会报错）
- 需要启用时，请在代码里实现真实请求并配置权限策略

## 参数
- `url`（必填，string）：要抓取的 URL
- `method`（可选，string，默认 `GET`）：HTTP 方法
- `headers`（可选，object）：请求头

