# 解析器产出 Resource，路由器按能力选择后端

下载入口固定为 `Link → LinkParser → Resource → Router → DownloadBackend`。LinkParser 允许发起网络请求补全元数据（例如 Hub 文件清单），其输出是与协议无关的 Resource；Router 依据 Resource 类别与后端自声明 capability 选择后端，并支持优先级与失败回退。拒绝"每个后端各自 `supports(uri)` 嗅探"的方案，因为协议知识会散落到后端、路由顺序不可预测且难以单测。

Parser 的失败回退由 [ADR-0010](0010-first-claiming-parser.md) 收紧为"首个认领者
权威"；后端优先级与回退仍保留在这一层。

## Considered Options

- 后端自报 `supports(uri)`、Router 轮询：疑似灵活，但路由顺序与冲突处理不可预测，测试需要构造全部后端。
- 解析与执行合一：实现简单，但 Hub 类链接需要先查 API 才能知道文件清单，无法在不发请求的情况下路由。
