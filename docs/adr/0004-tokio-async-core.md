# tokio async 内核 + blocking 门面

`DownloadEngine` 的公开 API 为 async（tokio），HTTP 后端使用 reqwest async，CLI 与同步调用者通过轻量 blocking 封装使用。多任务调度、取消、事件流与超时共享同一套 async 原语；另写一套同步实现会产生行为漂移。BT 后端内部线程模型不受此约束，由后端自行决定。
