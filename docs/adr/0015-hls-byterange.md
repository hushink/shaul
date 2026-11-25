# HLS #EXT-X-BYTERANGE

## Context

HLS 允许分片引用同一个资源中的字节区间，常见于 fMP4/CMAF 的单个媒体文件。
一期拒绝 `#EXT-X-BYTERANGE`，因为现有 HTTP 后端只有"下载整个文件"的入口；
为每个分片重复下载完整文件不可接受。`#EXT-X-MAP` 也可能带 `BYTERANGE`。

## Decision

- 播放列表解析：
  - `#EXT-X-BYTERANGE:<n>[@<o>]` 解析为长度和可选偏移；长度为 0 或溢出时报错。
  - 缺少偏移时，按 RFC 从前一个同 URI 分片的区间末尾继续；前一个分片没有
    区间或 URI 不同时从 0 开始。
  - 分片在解析结束时保存已解析的绝对偏移；`ByteRange` 的 `offset` 在解析后
    总是 `Some`。
  - `#EXT-X-MAP` 的 `BYTERANGE` 同样解析，缺少偏移时按 0 处理；它参与
    `init_changed` 判断（URI、密钥或区间任一变化都视为变化）。
  - `validate_media` 不再拒绝 BYTERANGE；仍拒绝直播、SAMPLE-AES 和其他不支持
    的加密方式。
- HTTP 层：
  - `HttpBackend` 新增公开方法
    `download_range(request, file, range, destination) -> Result<u64>`。
  - 只允许 `GET` 且没有请求体；复用任务的 headers、认证、代理、超时、重试、
    限速和连接预算。
  - 发送 `Range: bytes=<start>-<end>`，校验 `206` 的 `Content-Range` 必须精确
    匹配请求区间；`416` 和 4xx 直接失败，5xx/408/429 按重试策略处理。
  - 服务器忽略 `Range` 返回 `200` 时，流式读取并丢弃前缀，写满请求长度后停止；
    这是兼容性回退，代价是多读前缀。
  - 写入 `<destination>.shaul.part`，fsync 后原子 rename；不发送进度事件，
    由调用方汇总进度。
- HLS 后端：
  - 每个分片/init map 的 `ByteRange` 转成 core 的 `ByteRange`，传给
    `download_range`；普通分片继续使用 `download_file`。
  - 加密的 BYTERANGE 分片仍然下载为 `.enc`，再解密为 `.bin`；续传和拼接只看
    `.bin`。
  - 计数器按解密后的长度累加，和最终输出一致。

## Consequences

- 同一文件上的多个区间会各发一次 Range 请求；不会把整个文件缓存到磁盘。
- 不支持 Range 的服务器仍可工作，但会读取并丢弃前缀；大文件上的回退成本较高。
- SAMPLE-AES、直播、DASH、交替音轨和 ffmpeg 转封装仍不在范围内。
- `HttpBackend::download_range` 是新增公开方法，属于 minor 变更。
