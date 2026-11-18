# 多 crate workspace 与静态扩展

shaul 采用 workspace 多 crate：`shaul-core` 承载领域 trait、引擎与调度，`shaul-http`/`shaul-bt`/`shaul-hub` 实现后端，`shaul-cli` 是命令行应用，`shaul` 是默认组装的 facade。第三方扩展通过新 crate 实现 trait 并注册，不做 dylib 插件——Rust 没有稳定 ABI，动态插件会让错误类型、生命周期与线程模型契约失控。代价是发布时多个 crate 需要同步版本，且 facade 必须谨慎 re-export。

## Considered Options

- 单 crate + feature flags：无法表达"后端可替换"的边界，BT 的重依赖会污染所有用户。
- dylib 插件：Rust ABI 不稳定，错误与生命周期契约不可控。
