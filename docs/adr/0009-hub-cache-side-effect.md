# Hub 下载以目标目录为主，缓存是可选的副产物

Hub Task 始终把文件写到 `<destination>/<repo-name>/<repo-relative-path>`，
与既定的输出布局一致。`hub.use_cache` / `--cache` 开启时，后端在文件
校验通过后再把目标文件硬链接（跨卷时复制）到 provider 缓存：

- HF：`<hf_cache>/<models|datasets>--org--repo/{blobs,refs,snapshots}`；
  LFS 文件以 SHA-256 命名 blob，非 LFS 文件使用树 API 的 git oid；snapshot 与
  refs 按 `x-repo-commit` 写入。
- ModelScope：shaul 自己的 `<modelscope_cache>/<models|datasets>--org--repo`
  布局，blob 以 API 提供的 SHA-256 命名。

缓存复用前校验大小与可用校验值；命名冲突时按任务冲突策略处理。Provider token
在运行时从环境变量/官方凭据文件解析，不写入 sidecar。

## Consequences

- 缓存默认关闭，避免不同卷时产生额外复制；开启后目标目录与缓存共享 inode
  （同卷）或保留两份（跨卷）。
- Snapshot 使用硬链接而不是符号链接，Windows 无需管理员权限；读取工具仍能看到
  正常文件。
- 0.2 不实现 Xet 块级去重；Xet 存储的文件走普通 HTTP。
