# Validation — 2026-10-11

Validation is performed against a native ARM64 build on macOS 15.7.1, Apple M4 Max (12 performance + 4 efficiency cores), 48 GiB memory; Rust 1.94.1 and Swift 6.2.3. These are local measured results, not guarantees for every archive, OS release or Apple chip.

## Finder and language acceptance

Finder primary-menu actions were exercised on synthetic Chinese-named files. The generic compression action displayed a compact ZIP/7z selector with the source parent as destination; browsing to another folder produced a valid 7z there. Direct compression used a compact progress panel. The latest build also suppresses automatic dashboard/Jobs restoration and hides these windows on incoming Finder compression/extraction requests.

After the real Sparkle update to build 3, a warm-launch generic compression click displayed the compact selector and created a valid ZIP. Direct 7z produced a valid archive after the app had been quit. The primary extraction action created a fresh `进度验证.txt (3)` directory without merging into existing output; its extracted 32 MiB file matched SHA-256 `4ac87b5b0e6c5b3526afc67d635f55e7d864308313eec081f2b801c219387653`. Explicit “Extract Here” remains available for intentionally extracting without a wrapper. Reopening the app itself still opens its dashboard; app-selection UI automation can trigger this reopen, so that observation is distinct from the Finder action.

Settings initially selected Simplified Chinese. Selecting English and restarting visibly changed the dashboard, menu bar and Settings to English. The language setting was then restored to Simplified Chinese. Dynamic counts, job state and duration strings use explicit translated format keys. Developer diagnostics and raw codec messages may retain their original language.

## Downloadable preview artifacts

`v0.2.0-preview.2` provides ARM64 DMG and ZIP packages for macOS 15+. The ZIP was extracted and the DMG mounted read-only; every application file hash, symlink and permission mode matched the staging bundle. Both nested ad-hoc signatures verified, and the engine extracted from the final ZIP passed all 24 synthetic security checks. The packaging pipeline removes builder home paths and does not include provisioning profiles.

These preview artifacts are **not notarized** and have no Developer ID distribution signature. Local `spctl --assess --type execute` rejects the ad-hoc package. This is separate from the development-signed installed-app Finder acceptance above; Finder extension activation on other Macs using the preview signature has not been verified.

## Automatic update acceptance

The home-server HTTPS feed and immutable ZIP were downloaded back and independently verified using the embedded Ed25519 public key. Deliberately modified feed/archive bytes were rejected. Publishing uses Sparkle's official `generate_appcast` with a local mode-0600 key file and does not access Keychain; the key is outside the repository and is not deployed to the server.

An installed build 2 actually checked the public feed, downloaded build 3, verified it, installed it and relaunched. Its app executable, engine and Info.plist matched the release staging bundle byte-for-byte, and nested code-signature verification passed. Background notification delivery depends on macOS notification permission; a four-hour polling interval and busy-job deferral are implemented, but a full four-hour idle-cycle and a busy-install deferral were not separately timed in this acceptance run. Notifications are polling-based, not APNs server push.

The isolated update container is healthy, has no host port, and the pre-existing home-server containers retained their IDs/start times across deployment. This proves the checked deployment isolation, not immunity to compromise.

## Automated checks

- Swift: **71 tests passed**.
- Rust: **188 reported passed**, with **1 explicitly ignored** large-file test. Counts include the fixture early returns described below.
- Synthetic security regressions: **24 passed**, including a real SIGTERM cancellation that preserved the existing archive and removed temporary output.
- Independent compatibility matrix: **36 passed**, **0 failed**, **1 skipped** (macOS system unzip lacks support for the BZIP2 method; 7-Zip verifies that archive).
- Localization: **492 source keys**, matching app/kit tables for English, Simplified Chinese and Traditional Chinese; placeholder validation passed.

See the exact commands in the root README. The Rust suite includes fixtures that return early when external tools are absent; those are reported separately, not treated as format-compatibility evidence. Unavailable fixtures: gcab-generated CAB, dpkg-deb-generated Debian packages and lzip-generated streams/TAR. External tar checks for LZ4, Brotli and LZMA are not implemented in that test. Native engine round trips for those TAR codecs still run.

The optional 4.5 GB ZIP64 test is not run because local free disk space is limited. The 70,000-entry ZIP64 test is included.

## Security scope

Local synthetic attacks exercise traversal, symlink redirection, corrupt overwrite preservation, duplicate normalized ZIP paths, untrusted allocation limits, temporary-file redirection and password stdin transport. Source inspection and regression fixes also cover queue cancellation/retry races, subprocess completion, automatic deletion ordering and password-store input bounds.

`cargo-audit` 0.22.2 checked 140 locked dependencies against RustSec commit `7eebec69c352c7191b1f13eb95dd510eeca5d1de` (database updated 2026-10-09): **0 known advisories and 0 warnings**. This cannot audit macOS system libarchive or every behavior of vendored codecs. See [SECURITY.md](../SECURITY.md) for the remaining local directory race, resource-limit and transaction boundaries.

The public branch begins with a sanitized source snapshot. Local baseline history, personal signing settings, local logs, generated apps and real user files are not published. The main app is not sandboxed; the Finder extension is. Local development signing passes nested signature verification, but is not Developer ID notarization or Gatekeeper distribution acceptance.

## Performance

The preview.1 → preview.2 paired run uses five timed repetitions after one warmup per binary/case, with alternating order. All **70 timed round trips plus 14 warmups** passed full relative-path, length and SHA-256 validation. All seven cases preserve archive size; six also preserve archive bytes. Small 7z changes its default dictionary metadata. The [paired report](BENCHMARKS.md#preview2-优化前后) reports all cases and memory tradeoffs, including the effectively unchanged mixed-7z compression time. These are process-level latencies, not isolated codec throughput measurements.

The final performance engine passed the Rust, 24-check security and 36-pass compatibility suites above. The compatibility runner now discovers either `7zz` or `7z`; an initial run without the latter executable failed its opener invocations, then passed using the installed official `7zz`.

### Earlier baseline comparison

For the additional same-machine comparison with Bandizip 365, 7-Zip, Info-ZIP and libarchive, see [Benchmark](BENCHMARKS.md). It includes default and high-speed-off modes, archive size, compression/extraction time, memory and all 30 validated round trips.

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
