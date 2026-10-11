# Validation — 2026-10-11

Validation is performed against a native ARM64 build on macOS 15.7.1, Apple M4 Max (12 performance + 4 efficiency cores), 48 GiB memory; Rust 1.94.1 and Swift 6.2.3. These are local measured results, not guarantees for every archive, OS release or Apple chip.

## Finder and language acceptance

The final 0.2.0 installed app was launched and its Finder extension enabled. A real primary context-menu click on a synthetic Chinese-named text file created a ZIP. A real primary-menu **smart extract here** click then recreated that file. Its SHA-256 matched the original: `7be0d1b97507509a2fe5a94d5c314508160b56704947e56c9fed953132b6609c`. Independent `unzip -t` also succeeded.

Settings initially selected Simplified Chinese. Selecting English and restarting visibly changed the dashboard, menu bar and Settings to English. The language setting was then restored to Simplified Chinese. Dynamic counts, job state and duration strings use explicit translated format keys. Developer diagnostics and raw codec messages may retain their original language.

## Automated checks

- Swift: **67 tests passed**.
- Rust: **187 reported passed**, with **1 explicitly ignored** large-file test. Counts include the fixture early returns described below.
- Synthetic security regressions: **24 passed**, including a real SIGTERM cancellation that preserved the existing archive and removed temporary output.
- Independent compatibility matrix: **36 passed**, **0 failed**, **1 skipped** (macOS system unzip lacks support for the BZIP2 method; 7-Zip verifies that archive).
- Localization: **478 source keys**, matching app/kit tables for English, Simplified Chinese and Traditional Chinese; placeholder validation passed.

See the exact commands in the root README. The Rust suite includes fixtures that return early when external tools are absent; those are reported separately, not treated as format-compatibility evidence. Unavailable fixtures: gcab-generated CAB, dpkg-deb-generated Debian packages and lzip-generated streams/TAR. External tar checks for LZ4, Brotli and LZMA are not implemented in that test. Native engine round trips for those TAR codecs still run.

The optional 4.5 GB ZIP64 test is not run because local free disk space is limited. The 70,000-entry ZIP64 test is included.

## Security scope

Local synthetic attacks exercise traversal, symlink redirection, corrupt overwrite preservation, duplicate normalized ZIP paths, untrusted allocation limits, temporary-file redirection and password stdin transport. Source inspection and regression fixes also cover queue cancellation/retry races, subprocess completion, automatic deletion ordering and password-store input bounds.

`cargo-audit` 0.22.2 checked 140 locked dependencies against RustSec commit `7eebec69c352c7191b1f13eb95dd510eeca5d1de` (database updated 2026-10-09): **0 known advisories and 0 warnings**. This cannot audit macOS system libarchive or every behavior of vendored codecs. See [SECURITY.md](../SECURITY.md) for the remaining local directory race, resource-limit and transaction boundaries.

The public branch begins with a sanitized source snapshot. Local baseline history, personal signing settings, local logs, generated apps and real user files are not published. The main app is not sandboxed; the Finder extension is. Local development signing passes nested signature verification, but is not Developer ID notarization or Gatekeeper distribution acceptance.

## Performance

Compared with the untouched local Git initialization baseline (`66c3202`). Both binaries are native ARM64 release builds. `bench/compare.py` uses an identical seeded 128 MiB corpus (50% text, 25% semi-compressible binary, 25% media-shaped data), alternates baseline/candidate order and reports three-run medians. Every output is tested by the generating engine. The 512 MiB stored-file case uses five measured repetitions after warmup.

| Workload | Baseline MiB/s | Final MiB/s | Baseline peak RSS MiB | Final peak RSS MiB |
|---|---:|---:|---:|---:|
| zip-fast | 767.4 | 779.9 | 181.2 | 163.3 |
| zip-normal | 770.3 | 776.3 | 185.9 | 166.3 |
| 7z-normal | 2.9 | 11.7 | 252.5 | 715.1 |
| 512 MiB ZIP store | 1596.3 | 1615.5 | 341.8 | 153.2 |

7z's automatic mode exposes more independent solid blocks to performance cores, with concurrency limited by estimated codec workspace and the memory budget. Its measured throughput is about 4× baseline on this corpus, at a higher memory peak. Explicit `--solid 64MB` retains the old block-size choice; `--solid on` retains one solid stream. ZIP's small timing differences are modest; the large stored-file result chiefly demonstrates lower memory use (about 55%). Its individual samples are bimodal because of filesystem/cache behavior, so no broad disk-throughput speedup is claimed.

These measurements are workload-specific, not a guarantee for single huge files, different compression methods, other chips or battery/thermal conditions. Raw samples and binary hashes are in [benchmark data](benchmarks/m4-max.json). The baseline history remains local because the original configuration contained personal signing information.

An additional same-corpus size check produced 82,402,802 bytes at baseline and 82,453,203 bytes with automatic block sizing: **0.061% larger**. This extra single repetition is for archive size; the throughput table above retains the original three-run medians.
