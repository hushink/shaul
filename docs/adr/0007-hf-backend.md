# HF 后端自研传输，缓存对齐 Python，Xet 延后

不包装 `hf-hub` 的下载器——它是单流、无断点续传、无多连接，与 S1 卖点冲突。HF 后端自行处理 resolve 逐跳重定向、`X-Linked-*`/`X-Xet-*` 头、Range 分片与 sidecar；缓存布局对齐 `huggingface_hub`：`<cache>/<type>s--<owner>--<name>/{blobs,refs,snapshots}`，尊重 `HF_HOME`/`HF_HUB_CACHE`，Windows 用硬链接→复制回退。0.2 对 Xet 存储的文件走纯 HTTP（不做块级去重），`hf-xet` 作为 0.3+ 的可选 feature；`hf-hub` 仅用于缓存扫描/对照测试。

## Consequences

- 在可选 feature 接入前，无法获得 Xet 块级去重与加速。
- 缓存兼容性需要持续跟随 HF 的目录布局与 token 位置。
