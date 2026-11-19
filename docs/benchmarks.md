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
| Full, 1 connection | 242.8 ms | 62.9 – 68.8 MiB/s |
| Full, 4 connections | 231.6 ms | 68.4 – 69.7 MiB/s |
| Full, 8 connections | 237.5 ms | 66.2 – 68.5 MiB/s |
| Resume from 50%, 8 connections | 163.4 ms | 47.3 – 51.0 MiB/s |

The resume case pre-writes the first 8 MiB of the `.part` file and a matching
`TaskResume` manifest, so it measures probe + resume-validation + transfer of
the remaining 8 MiB. Throughput is computed over the remaining bytes.

### Observations

- At 16 MiB over loopback, 1/4/8 connections are within roughly 5% of each
  other. The local disk and the single-process test server are the likely
  bottlenecks; this is not a WAN throughput measurement.
- The resume case avoids downloading the first half, but its per-byte cost is
  higher: probe, validator matching and checkpoint writes add roughly 40–50 ms
  over the ideal half-size transfer.
- These numbers are a regression baseline, not a claim about remote-server
  performance. A future benchmark should add a throttled/latency-injected
  server and a larger payload.

## Reproducing

The benchmark is a normal Cargo bench target:

```console
$ cargo bench -p shaul --bench http -- --noplot
```

Criterion writes its report under `target/criterion/`. CI compiles benchmarks
through `cargo check --workspace --all-targets`, but does not run timing
measurements, since shared runners are too noisy for stable numbers.
