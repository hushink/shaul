# shaul-cli

[![crates.io](https://img.shields.io/crates/v/shaul-cli.svg)](https://crates.io/crates/shaul-cli)

[English](README.md) | **简体中文**

[shaul](https://github.com/hushink/shaul) 的命令行界面。shaul 是统一的下载
引擎，支持 HTTP、Hugging Face、ModelScope、BitTorrent、FTP 与可选 Xet。

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

# Hugging Face 与 ModelScope
$ shaul download hf://org/repo --revision main --include '*.safetensors' --cache
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent 与 FTP
$ shaul download 'magnet:?xt=urn:btih:<info-hash>'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media
$ shaul download ftp://user:password@example.com/pub/file.bin

# 代理与带宽限制
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 \
      --limit 10MiB --per-host-limit 5MiB

# 持久队列
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id-or-suffix>
$ shaul pause <id>     # 运行中的 worker 约 500ms 内感知
$ shaul resume <id>
$ shaul cancel <id>

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
`--header-file`、`--bearer-token`、`--basic`、`--revision`、`--include`、
`--exclude`、`--cache/--no-cache`、`--content-cache/--no-content-cache`、
`--file-jobs`、`--mirror`、`--priority`、`--global-connections`、
`--per-host-active`、`--per-host-limit`、`--json`、`--force`、`--overwrite`
与 `--rename`。

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

## 文档

- [项目 README](https://github.com/hushink/shaul/blob/main/README.zh-CN.md)
- [常见问题](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [库使用指南](https://github.com/hushink/shaul/blob/main/docs/library.md)
