# BitTorrent 复用 librqbit

BT 后端包装 `librqbit` 9.x 的 `Session`：用 `StorageFactory` 定制输出布局，`only_files` 做文件选择，复用 pause/unpause、DHT 与限速；进度通过 100ms stats 轮询映射到统一事件流。BT 的 piece 位图与未完成状态由 librqbit 持久化拥有，sidecar 只存 Task 元数据与选择，不强行映射成 Segment 位图。`bt` feature 默认关闭，且不做种（见 ADR-0005）。

## Consequences

- 启用 BT 时引入约 60 个依赖；需要锁定 librqbit 版本并跟进其 API 变化。
- BT 任务的事件流是轮询适配，不是原生回调，进度粒度受 stats 刷新频率限制。
