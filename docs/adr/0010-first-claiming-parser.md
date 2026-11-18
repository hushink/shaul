# 第一个认领链接的 parser 具有权威性

Router 按注册顺序选择第一个 `supports(link)` 为 true 的 `LinkParser`，并直接
使用它的解析结果；解析错误向上传播，不再尝试后续 parser。Hub parser 注册在
直链 parser 之前。这样 HF/ModelScope 的 401/404 会变成明确错误，而不是被直链
后端当成 HTML 页面下载。

## Considered Options

- 继续在当前 parser 失败后尝试下一个：看似更健壮，但会让 Hub 鉴权/网络错误
  静默降级成"下载一个网页"，且难以诊断。
- 由 parser 自行声明 `exclusive`：增加接口复杂度，收益与固定注册顺序相同。

## Consequences

- 新增 parser 必须谨慎设计 `supports`，避免误认领普通 URL。
- 多后端优先级与失败回退仍可在 `Router::backend_for` 层面扩展，但与 parser
  的认领语义分离。
