# 自研 sidecar 续传清单，默认开启且可关闭

每个 Task 使用一份原子写入、带格式版本号的 sidecar 清单，记录已完成 Segment 区间、Validator（ETag/Last-Modified/总大小）与内容摘要；恢复时严格校验，Validator 不一致默认安全重下，不做 aria2 `.aria2` 兼容。Resume 默认开启，可在引擎与任务级关闭。服务器忽略 Range 返回 200 时切换到单流并丢弃已写区间，同时发出明确事件。

这个磁盘格式一旦发布就难以更改，因此选择自研而非兼容外部工具；代价是无法直接接管 aria2 的未完成任务。
