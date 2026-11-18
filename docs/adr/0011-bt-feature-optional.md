# BitTorrent 是库的可选 feature，CLI 默认启用

`shaul` 的默认 features 不包含 `shaul-bt`；`shaul-cli` 依赖 `shaul` 时启用
`bt`。原因是 `librqbit` 会引入数十个依赖（DHT、tracker、peer protocol、
UPnP 等），纯 HTTP/Hub 的库用户不应为未使用的 BT 栈付出编译与体积成本；
CLI 作为"开箱即用"的入口默认包含全部后端。

## Consequences

- 库用户需要 `shaul = { ..., features = ["bt"] }` 才能解析磁力链/torrent 并
  使用 BT 后端。
- 默认构建的 `default_router` 不注册 `TorrentParser`/`BtBackend`；CLI 构建的
  会注册。
- BT 任务只做 leech：下载期间按协议响应上传，完成后立即暂停；不提供做种或
  分享率参数。
