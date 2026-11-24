# shaul-hub

[![crates.io](https://img.shields.io/crates/v/shaul-hub.svg)](https://crates.io/crates/shaul-hub)
[![docs.rs](https://docs.rs/shaul-hub/badge.svg)](https://docs.rs/shaul-hub)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的
[Hugging Face](https://huggingface.co) 与
[ModelScope](https://modelscope.cn) 支持。shaul 是一个多协议下载引擎。

## 支持的链接

- `hf://org/repo`
- `hf://datasets/org/repo`
- `ms://org/repo`
- `ms://datasets/org/repo`
- Hugging Face HTTPS 的 `/tree/<rev>`、`/blob/<rev>/<path>` 与
  `/resolve/<rev>/<path>` URL
- ModelScope HTTPS 仓库 URL，可带 `Revision` 查询参数

对于 `hf://` 与 `ms://` 链接，用 `--revision <rev>` 选择版本。HTTPS URL
也接受 `/tree/<rev>`、`/blob/<rev>/<path>`、`/resolve/<rev>/<path>` 或
`?revision=`。

## 特性

- Hugging Face 与 ModelScope 的 tree/file-list API
- 捕获 `X-Linked-Etag`、`X-Linked-Size` 与 commit 元数据
- 对 gated/私有仓库给出清晰错误
- 可选 provider 缓存：
  - Hugging Face：`models--org--repo/{blobs,refs,snapshots}`（兼容 Python）
  - ModelScope：shaul 自己的 `models--org--repo` 布局
- 从缓存 blob 到目标目录的硬链接或复制
- 用于选择文件的 include/exclude glob
- 通过 `xet` feature 可选启用 Hugging Face Xet 下载，token 或 CAS 操作失败
  时回退 HTTP
- 当要求校验但不知道校验值时，跳过 Xet
- 在拼接路径或写入缓存前拒绝不安全的仓库路径与文件名
- Provider 凭据在运行时解析，绝不写入任务 sidecar

## 凭据

- Hugging Face：`HF_TOKEN` 或 `HUGGING_FACE_HUB_TOKEN`
- ModelScope：`MODELSCOPE_API_TOKEN` 或官方 cookie 文件
- 用户提供的 `--bearer-token`、`--basic` 或请求头优先

## 用法

大多数用户依赖 [`shaul`](https://crates.io/crates/shaul) 门面。
`HubBackend`、`HuggingFaceParser`、`ModelScopeParser` 与 `HubClient` 可用于
自定义 router 和工具。

```toml
[dependencies]
shaul-hub = "1.2"

# 可选的 Hugging Face Xet 支持：
# shaul-hub = { version = "1.2", features = ["xet"] }
```

Hub 下载总是写入一个目录（`--dir`）；单文件 `--output` 路径会被拒绝，因为
快照可能包含多个文件。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [API 参考](https://docs.rs/shaul-hub)
