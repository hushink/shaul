# shaul-hls

[![crates.io](https://img.shields.io/crates/v/shaul-hls.svg)](https://crates.io/crates/shaul-hls)
[![docs.rs](https://docs.rs/shaul-hls/badge.svg)](https://docs.rs/shaul-hls)

[English](README.md) | **简体中文**

[shaul](https://crates.io/crates/shaul) 的 HLS 后端。

这个 crate 增加可选的 HTTP(S) `.m3u8` 解析器与后端：从 master playlist 选择
最高码率变体，并发下载分片，并拼接成一个输出文件。分片是内部细节，任务仍然
只产出一个文件。

## 用法

大多数应用应通过 `shaul` 门面启用 `hls` feature：

```toml
[dependencies]
shaul = { version = "1.2", features = ["hls"] }
```

```console
$ shaul download https://example.com/video/master.m3u8 --connections 8
```

用 `--include` / `--exclude` glob 按 `NAME`、`RESOLUTION`、`BANDWIDTH`、
`CODECS` 或 URI 选择变体：

```console
$ shaul download https://example.com/video/master.m3u8 --include '*1080*'
```

`HlsParser` 与 `HlsBackend` 也可以注册到自定义 router。后端保持协议隔离：
`shaul-core` 不依赖 HLS。

## 支持范围

- 带 `#EXT-X-ENDLIST` 的点播 media playlist
- master/media playlist，默认选择最高码率变体
- TS 输出，以及带单个 `#EXT-X-MAP` 的 fMP4
- AES-128 加密，支持密钥轮换，以及显式 IV 或按 media sequence 推导的 IV
- 通过共享 HTTP 后端并发下载分片，因此代理、认证、超时、重试、限速和连接预算
  都生效
- 分片级续传：暂停或失败后复用已完成的分片文件

暂不支持：

- SAMPLE-AES 加密
- `#EXT-X-BYTERANGE` 分片
- 直播列表
- DASH（`mpd`）
- 交替音轨和多音轨选择
- ffmpeg 转封装
- 最终输出的内容寻址缓存
- `--mirror`（HLS 任务会忽略）

输出扩展名：有 init map 时为 `.mp4`，否则为 `.ts`；`--output` 优先。

## 说明

- 后端在下载时重新抓取 media playlist，并用其 SHA-256 作为续传指纹。
- 已完成分片存放在 `<output>.shaul.hls.parts/`，通过
  `<output>.shaul.part` 拼接；成功后删除临时目录。
- 跨源 playlist/密钥/分片 URL 默认剥离敏感请求头；开启
  `allow_cross_origin_auth` 后保留。
- AES-128 密钥在运行时获取，只驻留内存，不写入任务 sidecar。加密分片先保存为
  `.enc`，解密后的 `.bin` 才用于续传和拼接。
- 如果 master 使用独立的音频 rendition，目前只会下载选中的视频变体，输出可能
  没有音轨。

## 文档

- [项目 README](https://github.com/hushink/shaul#readme)
- [HLS ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0013-hls-backend.md)
- [AES-128 ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0014-hls-aes128.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API 参考](https://docs.rs/shaul-hls)
