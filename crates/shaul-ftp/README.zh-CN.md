# shaul-ftp

[![crates.io](https://img.shields.io/crates/v/shaul-ftp.svg)](https://crates.io/crates/shaul-ftp)
[![docs.rs](https://docs.rs/shaul-ftp/badge.svg)](https://docs.rs/shaul-ftp)

[English](README.md) | **简体中文**

[shaul](https://crates.io/crates/shaul) 的 FTP 后端。

该 crate 提供可选的 `ftp://` parser，以及单流、可续传的 FTP backend。它使用
[`suppaftp`](https://crates.io/crates/suppaftp)，并在服务器支持时使用
`SIZE`、`MDTM` 与 `REST`。FTP 没有 Range 请求，因此下载不分片。

## 用法

大多数应用应依赖带 `ftp` feature 的 `shaul` 门面：

```toml
[dependencies]
shaul = { version = "1.2", features = ["ftp"] }
```

```console
$ shaul download ftp://user:password@example.com/pub/file.bin --dir ./downloads
```

`FtpParser` 与 `FtpBackend` 也可以注册到自定义 router。该 backend 与协议
隔离：`shaul-core` 不依赖 FTP 或 `suppaftp`。

## 说明

- 认证使用 URL userinfo 或 `--basic` 提供的 Basic 凭据。
- 续传使用 `REST` 加 `SIZE`/`MDTM`；若服务器拒绝 `REST`，shaul 会重新连接
  并从零开始。
- 已知校验值和文件大小时，FTP 传输也会参与可选的内容寻址缓存。
- 未启用 FTPS。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [常见问题](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API 参考](https://docs.rs/shaul-ftp)
