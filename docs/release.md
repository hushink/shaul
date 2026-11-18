# 发布流程（release-plz）

发布由 `.github/workflows/release-plz.yml` 驱动，配置在 `release-plz.toml`。

## 流程

1. 向 `main` push 符合 Conventional Commits 的提交。
2. CI 运行；`release-plz release-pr` 创建或更新 release PR，内容包括：
   - workspace 版本号；
   - 各内部依赖的版本要求；
   - `CHANGELOG.md`。
3. 人工审查 release PR。版本不对时，可在本地检出该分支后用
   `release-plz set-version <version>` 修正，再 push 回 PR 分支。
4. 合并 release PR。
5. `release-plz release` 在 `main` 上运行，按依赖顺序发布 crate、创建 tag
   和 GitHub Release。

`release_always = false`：只有合并 release PR 才会真正发布，普通 push 不会
触发 `cargo publish`。

## 必需的仓库配置

### Secrets

- `CARGO_REGISTRY_TOKEN`：crates.io token，scope 至少包含 `publish-new` 和
  `publish-update`。
  - 首次发布必须使用 token：crates.io 的 trusted publishing 不能创建新 crate。
  - 六个 crate 都发布过之后，可迁移到 trusted publishing。
- `RELEASE_PLZ_TOKEN`（可选）：fine-grained PAT，授予当前仓库的
  **Contents: read/write** 和 **Pull requests: read/write**。
  - 不配置时 workflow 回退到 `GITHUB_TOKEN`。
  - 使用 `GITHUB_TOKEN` 创建的 release PR 不会触发 `pull_request` CI；
    需要手动 close/reopen PR，或配置该 PAT。

### GitHub Actions 设置

仓库 Settings → Actions → General：

- 允许 GitHub Actions 创建和批准 pull request（release PR 需要）。
- Workflow 内已按 job 声明最小权限：release job 需要 `contents: write`，
  PR job 需要 `contents: write` 与 `pull-requests: write`。

## 版本规则

- 常规提交：`fix:` → patch，`feat:` → minor，`feat!:`/`BREAKING CHANGE:`
  → major。
- 首次 1.0 发布前，workspace 版本已手动设为 `1.0.0`；release-plz 对未发布
  crate 的首次 release PR 会沿用当前版本，不会再次自增。
- `semver_check = false`，因为 1.0 tag 尚不存在、没有基线；1.0 发布后应打开
  并接入 `cargo-semver-checks`。

## 发布顺序

release-plz 根据 workspace 依赖自动确定顺序：

1. `shaul-core`
2. `shaul-http`、`shaul-hub`、`shaul-bt`
3. `shaul`
4. `shaul-cli`

`shaul-cli` 安装后的二进制名仍是 `shaul`：

```console
$ cargo install shaul-cli
```

## 本地检查

```console
$ cargo package -p shaul-core --list
$ cargo publish --dry-run -p shaul-core
```

对依赖尚未发布的 crate，`cargo publish --dry-run` 在校验阶段会查找 registry
版本；首次发布时以 release-plz 的依赖顺序为准。
