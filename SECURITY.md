# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| 1.x | Yes |
| 0.x | No |

Security fixes are released on the latest 1.x line.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories:

https://github.com/hushink/shaul/security/advisories/new

Do **not** open a public issue for a suspected vulnerability.

Include, when possible:

- affected crate and version;
- operating system and architecture;
- a minimal reproduction (link type, command, config);
- impact and any known workaround;
- logs with tokens, cookies and private URLs removed.

We aim to acknowledge reports within three business days and will coordinate a
fix and disclosure timeline with the reporter. There is no bug bounty program.

## Scope

In scope:

- the download engine, parsers, routers and backends;
- the CLI, queue worker, sidecar store and configuration handling;
- credential handling, redaction and cross-origin redirect behavior;
- archive extraction and path traversal protections;
- dependency advisories that affect shaul in its supported configuration.

Out of scope:

- vulnerabilities in remote servers or provider APIs;
- content you are not authorized to download;
- issues that require a compromised local account or already-elevated access.

## Dependency security

`cargo deny check` runs in CI for advisories, licenses, duplicate dependencies
and registry sources. Dependabot tracks Cargo and GitHub Actions updates.
Known advisory exceptions are documented with a rationale in `deny.toml` and
reviewed when their upstream dependency changes.
