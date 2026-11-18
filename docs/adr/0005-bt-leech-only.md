# BitTorrent 后端只做 leech

BT 后端实现完整下载路径：metainfo/magnet、tracker/DHT、peer wire 与 piece 校验；下载期间按协议响应上传请求以维持 peer 关系，但完成后不常驻做种，不提供分享率、上传配额与做种策略。原因是长期做种需要驻留进程与连接管理，与"下载引擎"的定位冲突；这一显式的"不做"避免用户对 BT 能力产生错误预期。

## Consequences

- CLI 不提供 `--seed`/分享率相关参数，BT 任务完成后连接关闭。
- 若未来要支持做种，需要在 backend 之外引入常驻会话与上传调度，属于新能力而非参数开关。
