# HLS 是可选 crate，分片由后端内部管理

## Context

HLS（HTTP Live Streaming）把一个媒体文件切成播放列表和多个分片。它不是
"一个 URL 对应一个文件"的直链语义：

- 分片不是最终输出，不能作为 `FileItem` 暴露给用户；
- 分片数量、URL 和大小只有读取 media playlist 后才知道；
- 播放列表可能变化，续传必须识别"这是同一个点播列表"；
- AES-128、DASH、直播流和 fMP4 初始化段会显著扩大范围。

shaul 的 `Resource` / `FileItem` 模型把 `FileItem` 定义为落盘与校验单位，
`TaskResume` 以源 URL 为键。直接把每个分片塞进 `FileItem` 会污染用户可见的
文件列表、统计和清理语义。因此需要一个独立后端在内部管理分片。

## Decision

- 新增可选 crate `shaul-hls`，facade feature 为 `hls`，默认关闭；CLI 启用。
- `HlsParser` 只接受 HTTP(S) 且路径以 `.m3u8` 结尾的链接，抓取播放列表：
  - master playlist：按 `BANDWIDTH` 选择最高码率；`--include` / `--exclude`
    的 glob 会匹配 `NAME`、`RESOLUTION`、`BANDWIDTH`、`CODECS` 和 URI，
    用于手动选择变体；
  - media playlist：直接使用该 URL。
- 解析结果是一个 `ResourceKind::Custom { protocol: "hls" }` 资源，包含一个
  最终的 `ResourceFile`。`file.url` 是 media playlist URL，也是续传键；
  `file.path` 只是默认文件名，后端会根据分片类型改成 `.ts` 或 `.mp4`。
- `HlsBackend` 在运行时分片：
  - 重新抓取 media playlist（列表可能在 add 之后变化）；
  - 只支持点播：要求 `#EXT-X-ENDLIST`，拒绝直播列表；
  - 首期支持 TS 和带单个 `#EXT-X-MAP` 的 fMP4；首期拒绝 SAMPLE-AES、DASH、
    交替音轨和多个不同的 init map（AES-128 见
    [ADR 0014](0014-hls-aes128.md)，BYTERANGE 见
    [ADR 0015](0015-hls-byterange.md)）；
  - 每个分片通过共享的 `HttpBackend` 实例下载（连接数固定为 1），复用代理、
    认证头、超时、重试、全局/单主机连接预算和限速；
  - 分片文件放在 `<output>.shaul.hls.parts/`，全部完成后按顺序拼接成
    `<output>.shaul.part`，fsync 后原子 rename 到最终路径；
  - 输出扩展名：有 init map 时用 `.mp4`，否则 `.ts`；`--output` 优先。
- 续传：
  - 用 media playlist 文本的 SHA-256 作为指纹，加上分片数量，序列化到
    `FileResume.metadata["hls.state"]`；
  - 已完成的单个分片以临时目录中的文件是否存在为准（`HttpBackend` 只在分片
    完整后才 rename 成最终分片名），下次运行直接复用；
  - 指纹变化或分片数量变化时删除临时目录、从零开始；
  - 分片内部的字节级续传不做，未完成的分片从头下载；
  - 拼接阶段总是从空的 `.part` 文件开始。
- 安全：playlist 和分片 URL 可能来自与原始链接不同的源。原始 `.m3u8` 链接的
  origin 记录在 `metadata["hls.origin"]`；跨源请求默认剥离敏感头和 `Auth`，
  除非开启 `allow_cross_origin_auth`。playlist 重定向或相对 URI 解析使用最终
  响应 URL 作为基准。
- 校验、解压等继续由引擎的 finalizer 负责。HLS 后端返回最终文件的
  `FileOutcome`；`--verify` 需要用户提供最终文件的 checksum。最终输出暂不写入
  内容寻址缓存；`--mirror` 对 HLS 任务不生效。
- `FileResume.part_path` 指向拼接用的 `.part` 文件；分片临时目录记录在
  `metadata["hls.temp_dir"]`。成功完成时后端会自己删除该目录；取消/失败时
  保留用于续传。`clean --orphans` 会为已完成或已取消的任务删除该目录和拼接
  `.part`。

## Phase 1 scope

- 支持：HTTP(S) `.m3u8`、master/media playlist、最高码率自动选择、
  `--include`/`--exclude` 变体选择、VOD、TS、单 init map 的 fMP4、
  分片并发、分片级续传、`--output`。
- 不支持（后续增量）：直播、多音轨/交替 rendition、ffmpeg 转封装、分片内
  字节级续传。DASH 由独立的 `shaul-dash` crate 处理，见
  [ADR 0016](0016-dash-backend.md)。
- AES-128 已由二期加入；密钥、IV 和解密细节见
  [ADR 0014](0014-hls-aes128.md)。BYTERANGE 已由三期加入，见
  [ADR 0015](0015-hls-byterange.md)。

## Alternatives

- **把分片建模成 `FileItem`**：会污染任务文件列表、统计、清理和校验语义；
  分片不是用户要的文件。
- **直接使用 reqwest 下载分片**：需要重新实现代理、认证头、重定向、限速和
  连接预算；共享 `HttpBackend` 已经有这些能力。
- **内置 ffmpeg**：依赖和许可证成本高，而且首期只需要字节拼接。TS 可以直接
  保存为 `.ts`；fMP4 的 init + media 分片可以拼成可播放的 fragmented MP4。
  需要标准 MP4 时由用户自行转封装。
- **新增 core 协议类型**：`ResourceKind::Custom { protocol: "hls" }` 已经
  足够，并保持 core 协议无关。

## Consequences

- HLS 任务只产生一个输出文件，不暴露分片。
- 视频/音频分离的 master playlist 首期只会下载选中的视频变体，可能没有音轨；
  需要用户选择含音轨的变体或等待交替 rendition 支持。
- 临时磁盘占用约为最终文件的一倍；拼接前会做一次剩余空间检查。
- 手动选择变体复用 `--include`/`--exclude` 的 glob 语法，不新增 CLI 参数。
- `shaul-hls` 默认不启用；库用户需要 `features = ["hls"]`，CLI 直接可用。
