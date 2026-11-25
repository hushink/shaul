# shaul-cli

[![crates.io](https://img.shields.io/crates/v/shaul-cli.svg)](https://crates.io/crates/shaul-cli)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的命令行界面。shaul 是统一的下载
引擎，支持 HTTP、HLS、DASH、Hugging Face、ModelScope、BitTorrent、FTP 与
可选 Xet。

该 crate 安装名为 `shaul` 的可执行文件。

## 安装

```console
$ cargo install shaul-cli
```

Linux（x86_64/aarch64）、macOS（x86_64/aarch64）与 Windows（x86_64）的预编译
压缩包和安装器附在
[`shaul-cli` releases](https://github.com/hushink/shaul/releases)。

## 用法

```console
# HTTP 直链
$ shaul download https://example.com/file.tar.gz
$ shaul download https://example.com/file.bin --connections 8 \
      --checksum sha256:<hex>
$ shaul download https://example.com/api/export --method POST \
      --data '{"id":42}' -H 'Content-Type: application/json'

# Hugging Face 与 ModelScope
$ shaul download hf://org/repo --revision main --include '*.safetensors' --cache
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent 与 FTP
$ shaul download 'magnet:?xt=urn:btih:<info-hash>'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media
$ shaul download ftp://user:password@example.com/pub/file.bin

# HLS：最高码率变体，拼接成一个文件
$ shaul download https://example.com/video/master.m3u8 --connections 8
$ shaul download https://example.com/video/master.m3u8 --include '*1080*'

# DASH：从静态点播清单选择最高码率视频 Representation
$ shaul download https://example.com/video/manifest.mpd --connections 8
$ shaul download https://example.com/video/manifest.mpd --include '*1080*'

# 代理与带宽限制
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 \
      --limit 10MiB --per-host-limit 5MiB

# 持久队列
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list --status pending
$ shaul status <id-or-suffix>
$ shaul pause <id> <id>     # 一个或多个 id
$ shaul pause --all         # pending、running、paused
$ shaul resume --all        # paused、failed
$ shaul cancel --all        # pending、running、paused
$ shaul resume <id>         # 运行中的 worker 约 500ms 内感知

# 诊断与维护
$ shaul info <link>
$ shaul doctor
$ shaul stats
$ shaul clean --finished --cache
$ shaul config init|show|get|set|edit|path
$ shaul completions bash
```

常用参数：`--dir`、`--output`、`--checksum`、`--verify/--no-verify`、
`--extract/--no-extract`、`--extract-dir`、`--delete-archive`、
`--no-delete-archive`、`--header`、
`--header-file`、`--bearer-token`、`--basic`、`--method`、`--data`、
`--data-file`、`--revision`、`--include`、
`--exclude`、`--cache/--no-cache`、`--content-cache/--no-content-cache`、
`--file-jobs`、`--mirror`、`--priority`、`--global-connections`、
`--per-host-active`、`--per-host-limit`、`--json`、`--force`、`--overwrite`
与 `--rename`。

`pause` / `resume` / `cancel` 支持多个 id 或 `--all`。`--all` 对 pause 和
cancel 默认选择 pending/running/paused，对 resume 默认选择 paused/failed；
`--status` 可覆盖默认过滤。`list --status` 可重复指定状态。

`--include` / `--exclude` 也可用于按 `NAME`、`RESOLUTION`、`BANDWIDTH`、
`CODECS` 或 URI 选择 HLS 变体，或按 `id`、`mimeType`、`codecs`、`lang`、
分辨率和 `bandwidth` 选择 DASH Representation。HLS 支持点播 TS、单 init map
的 fMP4、AES-128 加密分片、`#EXT-X-BYTERANGE` 和分片级续传。DASH 支持静态
单 Period 清单、SegmentTemplate/SegmentList/SegmentBase 寻址和分片级续传；
暂不支持 SAMPLE-AES、直播列表、DRM 和多 Period DASH。

`--method`、`--data`、`--data-file` 只适用于直接 HTTP(S) 链接。未指定
`--method` 时，`--data` 隐含 `POST`。这类请求是单请求语义：不分片、不续传、
不使用镜像，`POST`/`PATCH` 不会自动重试。请求体支持 `${env:NAME}` 占位符，
并且不会出现在 `--json` 输出中。`--data-file` 在命令执行时读取，内容会像
`--data` 一样写入任务 sidecar。

`shaul run --jobs N` 会并发处理任务。`shaul download --limit` 与
`shaul add --limit` 限制单个任务；`shaul run --limit` 限制 worker 的全局
速率。

## 配置

优先级为 **命令行参数 > 配置文件 > 环境变量 > 默认值**。

`shaul config path` 打印解析后的配置路径。`--config PATH` 或 `SHAUL_CONFIG`
可覆盖它。任务状态保存在每用户的数据目录中；同一时间只有一个
`shaul run` worker 持有某个状态目录。

在 Unix 上，任务 sidecar 以 0600 权限写入。从环境变量或官方凭据文件读取的
provider token 永不持久化。`--json` 输出会对敏感请求头做脱敏，并从任务视图
中移除查询字符串。

配置文件会拒绝未知键，因此拼写错误会直接报错而不是被忽略。
`shaul config set` 原地编辑 TOML 文档，保留已有注释与格式。`shaul doctor`
还会检查 Hugging Face 与 ModelScope 的官方凭据文件。校验失败的文件会被重命名
为 `<name>.shaul.failed`，`clean --orphans` 会忽略 sidecar 中记录的不安全路径。
它还会清理已完成/已取消任务遗留的 `.shaul.part` 文件，以及 HLS
`.shaul.hls.parts/` 和 DASH `.shaul.dash.parts/` 目录；统计目录大小时不跟随
符号链接，并且永远不会删除目标文件。

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [常见问题](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
