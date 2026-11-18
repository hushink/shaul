# Contributing

Thanks for helping improve shaul.

## Prerequisites

- Rust **1.92** or newer
- A C toolchain is not required for normal builds; the workspace uses rustls

## Build and test

```console
$ cargo fmt --all --check
$ cargo clippy --workspace --all-targets --all-features -- -D warnings
$ cargo test --workspace --all-features
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
$ cargo deny check
```

All public items must be documented; every crate enables
`#![warn(missing_docs)]`.

The test suite is self-contained: it spins up local HTTP servers and a local
BitTorrent peer. Do not add tests that require external network access.

Feature checks:

```console
$ cargo check -p shaul --no-default-features
$ cargo check -p shaul --features bt
$ cargo check --workspace --all-targets --all-features
```

## Code style

- Keep changes focused and avoid unrelated refactors.
- Use `thiserror` for public errors and preserve the `Error` variants' context.
- Public API additions must respect the 1.x semver policy in
  [docs/api-audit.md](docs/api-audit.md).
- New protocol behavior should be documented in an ADR when it is hard to
  reverse and involves a real trade-off.

## Commits and releases

- Use Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
  `chore:`; use `feat!:` or a `BREAKING CHANGE:` footer for intentional breaks.
- `release-plz` turns commits on `main` into a release PR. Do not edit versions
  or `CHANGELOG.md` by hand unless a maintainer asks you to.
- The release process is documented in [docs/release.md](docs/release.md).

## Pull requests

- Describe the problem and the approach.
- Include tests for behavior changes and update docs when user-facing behavior
  changes.
- Confirm the PR checklist locally before requesting review.
- Report security issues privately per [SECURITY.md](SECURITY.md); do not put
  tokens, cookies or private URLs in issues, tests or commit messages.

## License

By contributing, you agree that your contributions are licensed under the
Apache-2.0 license used by this repository.
