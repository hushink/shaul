# Finalizer 流水线：合并 → 校验 → 解压 → 原子落盘

Task 在标记完成前执行固定顺序的 Finalizer 链：合并（仅当后端产生独立分片）→ 校验（存在 Checksum 或被强制时）→ 解压（启用时）→ 原子 rename。校验先于解压，避免把损坏的压缩包展开；原子 rename 放在最后，确保任何一步失败都不会留下"看似完成"的目标文件。

Extraction 支持 zip、tar、tar.gz、tar.zst 与单文件 gz/zst，默认关闭；解压目录默认任务目标目录，压缩包默认保留；拒绝绝对路径、`..` 与符号链接条目，先解到临时目录。Verification 默认 `auto`（有 Checksum 才校验），BT 的 piece 校验始终开启。

## Consequences

- 解压失败时压缩包保留、Task 标记失败，可重试；不会留下半解压目录。
- 分片下载默认写入单个 `.part` 的偏移位置，因此没有常规合并步骤；该步骤只服务产生独立分片的后端。
