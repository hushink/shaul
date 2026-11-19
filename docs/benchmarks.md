# Benchmarks

## 2026-11-18 baseline

Environment:

- Windows 11 Pro for Workstations
- Intel Core i5-12400F (6C/12T)
- rustc 1.99.1 / cargo 1.99.1
- benchmark built with `cargo bench` (optimized `bench` profile)

Command:

```console
$ cargo bench -p shaul --bench http -- --noplot
```

Workload: a local range-capable HTTP/1.1 server, 16 MiB payload, 1 MiB
segments, verification disabled, retries disabled, fresh temporary
destination per iteration. Only `DownloadEngine::run_task` is measured; task
creation and engine construction happen during Criterion setup.

### Results

| Case | Criterion estimate | Throughput |
| --- | ---: | ---: |
| Full, 1 connection | 220.0 ms | 68.6 – 77.1 MiB/s |
| Full, 4 connections | 205.0 ms | 76.8 – 79.3 MiB/s |
| Full, 8 connections | 202.2 ms | 78.3 – 80.1 MiB/s |
| Resume from 50%, 8 connections | 151.9 ms | 52.0 – 53.5 MiB/s |

These numbers are from a second, warmed run. The first run on the same machine
was 10–15% slower (for example 242.8 ms / 65.9 MiB/s for one connection), so
treat small differences as run-to-run noise rather than regressions.

The resume case pre-writes the first 8 MiB of the `.part` file and a matching
`TaskResume` manifest, so it measures probe + resume-validation + transfer of
the remaining 8 MiB. Throughput is computed over the remaining bytes.

### Observations

- At 16 MiB over loopback, 4 and 8 connections are within ~1.5% of each other
  and only ~9% faster than one connection. The local disk and the
  single-process test server are the likely bottlenecks; this is not a WAN
  throughput measurement.
- The resume case avoids downloading the first half, but its per-byte cost is
  higher: probe, validator matching and checkpoint writes add roughly 50 ms
  over the ideal half-size transfer.
- These numbers are a regression baseline, not a claim about remote-server
  performance. A future benchmark should add a throttled/latency-injected
  server and a larger payload.

## 2026-11-19 pre-release check (1.1 network controls)

The same command was run after adding proxy/rate-limit plumbing. No proxy or
rate limit was configured, so the transfer path should be unchanged. A quick
run produced:

| Case | Time | Throughput |
| --- | ---: | ---: |
| Full, 1 connection | 249 ms | ~64 MiB/s |
| Full, 4 connections | 227 ms | ~70 MiB/s |
| Full, 8 connections | 238 ms | ~67 MiB/s |
| Resume from 50%, 8 connections | 155 ms | ~52 MiB/s |

Criterion reported no change against the immediately preceding run in this
environment, but the 8-connection estimate is above the 2026-11-18 warmed
baseline. This was a noisy run; re-measure on a quiet machine before treating
it as a regression. The rate-limit path is disabled when both limits are unset.

## Reproducing

The benchmark is a normal Cargo bench target:

```console
$ cargo bench -p shaul --bench http -- --noplot
```

Criterion writes its report under `target/criterion/`. CI compiles benchmarks
through `cargo check --workspace --all-targets`, but does not run timing
measurements, since shared runners are too noisy for stable numbers.
