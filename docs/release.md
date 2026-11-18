# 发布流程（release-plz）

发布由 `.github/workflows/release-plz.yml` 驱动，配置在 `release-plz.toml`。

当前状态：1.0.1 已发布；六个 crate 已配置 crates.io trusted publishing，
`CARGO_REGISTRY_TOKEN` secret 已删除。后续发布直接使用 GitHub OIDC 获取短期
token。

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

#### 首次发布：一次性 bootstrap token

crates.io 不允许通过 trusted publishing 创建全新 crate。六个 crate 都是首次
发布，因此 1.0.0 必须先用一次性 API token bootstrap：

1. crates.io → Account Settings → API Tokens → New Token；
2. scope 勾选 `publish-new` 和 `publish-update`；
3. 添加为 GitHub 仓库 secret `CARGO_REGISTRY_TOKEN`；
4. 合并 release PR。workflow 检测到该 secret 后会注入
   `CARGO_REGISTRY_TOKEN`，release-plz 发布六个 crate 并创建 tag/GitHub
   Release；
5. 确认六个 crate 在 crates.io 可见后，立即撤销这个 API token；
6. 删除 GitHub secret `CARGO_REGISTRY_TOKEN`。

#### 本地 bootstrap（不配置 GitHub secret 的替代方案）

本地没有 GitHub OIDC，因此仍然需要一个 crates.io token：

```console
$ cargo login
$ cp release-plz.toml release-plz.local.toml
# 在 release-plz.local.toml 中设置 release_always = true
# 如工作区有未跟踪文件，另设 publish_allow_dirty = true
$ release-plz release --config release-plz.local.toml --dry-run
$ release-plz release --config release-plz.local.toml
```

本地 release 也会创建 git tag / GitHub Release，需要 `GITHUB_TOKEN` 或
`--git-token`。如果只想发布、不创建 tag/Release，在本地配置中设置
`git_tag_enable = false` 与 `git_release_enable = false`，之后手动创建。
完成后删除 `release-plz.local.toml`、执行 `cargo logout` 并撤销 token。

#### 切换到 trusted publishing（首次发布后）

1. 在 crates.io 为每个 crate 配置 trusted publisher：
   - Repository owner：`hushink`
   - Repository name：`shaul`
   - Workflow filename：`release-plz.yml`
   - Environment：留空（workflow 未使用 environment）
2. 删除仓库 secret `CARGO_REGISTRY_TOKEN`。
3. workflow 的 `Select crates.io credentials` 步骤检测不到 token，会保持
   `CARGO_REGISTRY_TOKEN` 未设置。
4. release job 已声明 `id-token: write`；release-plz 会通过 GitHub OIDC
   向 crates.io 换取短期 token，无需长期 secret。

注意：使用 trusted publishing 时不要在 workflow 中设置空字符串的
`CARGO_REGISTRY_TOKEN`，也不要使用 `rust-lang/crates-io-auth-action`；
release-plz 自己实现这套换取流程。

`CARGO_REGISTRY_TOKEN` secret 只存在于 GitHub Secrets；workflow 仅在 release
job 内把它写入 `$GITHUB_ENV`，不会写入文件或日志。

#### RELEASE_PLZ_TOKEN（可选）

fine-grained PAT，授予当前仓库的 **Contents: read/write** 和
**Pull requests: read/write**。

- 使用 `GITHUB_TOKEN` 创建的 release PR 不会自动触发 `pull_request` CI；
  需要手动 close/reopen PR 才能跑检查。
- CLI 二进制发布不依赖该 PAT：release job 会在发布后通过
  `workflow_dispatch` 主动触发 cargo-dist。
- 该 token 只影响 GitHub 操作，与 crates.io 发布凭据无关。

### CLI 二进制分发

`.github/workflows/shaul-cli-release.yml` 由 cargo-dist 生成，支持
`shaul-cli-v*` tag 和手动 `workflow_dispatch`：

- 构建 Linux x86_64/aarch64、macOS x86_64/aarch64、Windows x86_64；
- 附带 `.tar.xz`/`.zip`、SHA-256、shell 安装器和 PowerShell 安装器；
- 由 cargo-dist 创建该 tag 的 GitHub Release，并上传全部产物。

release-plz 对 `shaul-cli` 关闭了 `git_release_enable`，但保留 tag 创建，
避免与 cargo-dist 重复创建 Release。

发布后，release-plz job 会从 `releases` 输出中取出 `shaul-cli-v*` tag，并用
`gh workflow run` 触发 cargo-dist。`workflow_dispatch` 不受
“`GITHUB_TOKEN` 创建的事件不触发新 workflow”这一限制，因此不需要 PAT。

已有 `shaul-cli-v1.0.0` Release 早于该流程，因此没有二进制附件；从下一个
CLI release 开始生效。若要补 1.0.0 二进制，需要按目标平台手动构建后
`gh release upload` 到现有 Release，或重新创建该 Release。

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
