# Engine security remediation — October 2026

This change addresses seven medium-severity Codex Security findings against the Rust engine and its vendored `sevenz-rust2`. The scan examined an older revision; fixes and regression comparisons use main revision `4c42de2d1f459d234d460f1524e2e5c3034a8011` as the baseline.

## Finding coverage

| Finding prefix | Boundary fixed | Regression evidence |
| --- | --- | --- |
| `39cd5e06` | 7z readers retain unsigned 64-bit remaining lengths, reject premature EOF, and verify CRC. Engine extraction independently checks observed length and CRC before publishing output. Address-space conversions are checked. | Crafted declarations at `i64::MAX`, `i64::MAX + 1`, and `u64::MAX` fail without replacing an existing file. |
| `21bb1416` | TAR listing bounds decoded bytes, extension metadata consumed internally by the TAR parser, record count, names, estimated metadata/JSON, and elapsed time. Budget exhaustion is an explicit error. | Compressed payload expansion, oversized GNU long-name metadata, and a 100,001-header flood fail; an ordinary GNU long name still works. |
| `d3e918ca` | ZIP symlink entries apply Skip, Rename, Ask, and Overwrite decisions. A replacement symlink is created in private staging before publication, preserving the original if link creation fails. | All conflict decisions are exercised, including callback decisions and an invalid link target that must leave the original intact. |
| `f71b8dc4` | Public 7z helpers reject traversal and symlink ancestors. On Unix, default extraction walks directories using `openat` with `O_NOFOLLOW`, creates private staging relative to the held directory descriptor, and publishes with `renameat` after validation. | Preexisting ancestor symlinks, ancestor replacement after callback dispatch, and corrupted replacement payloads cannot redirect writes or overwrite the original. |
| `9addcf3d` | Raw/decoded 7z headers, parser cardinalities, and public APIs returning buffered data have independent operational limits. Solid `read_file` charges decoded preceding entries as well as the requested entry. WASM extraction shares a cumulative budget across entries. | An encoded-header expansion bomb and oversized `read_file` fail. A solid 140 MiB prefix plus 140 MiB target fails the cumulative budget; streaming extraction, including a 300 MiB file, succeeds. |
| `1d1e1601` | ZIP editing uses private randomized staging and held output descriptors, validates the rebuilt archive, then atomically replaces the input. Repair, join, and split create temporary files exclusively and publish without replacing an existing destination. Automatic split joining uses a randomized temporary directory. | Planted symlinks at legacy `.edit.part` / `.add.part`, repair, join, and numbered split paths leave their target unchanged. Existing regular destinations are also refused. |
| `2a56c58f` | ZIP central-directory count is checked before allocation and while parsing, including archives with dishonest EOCD counts. Name, path-depth, estimated metadata, and estimated JSON budgets apply before model growth. | 100,000 minimal entries succeed; 100,001 fail even with an underreported EOCD. Oversized paths, excessive depth, and name-driven allocation amplification fail. |

## Operational limits and compatibility

`engine/fz-core/src/limits.rs` centralizes listing limits. These intentionally reject some valid but excessively expensive archives instead of silently truncating their listing.

| Resource | Limit |
| --- | --- |
| ZIP/TAR listing entry count | 100,000 |
| Aggregate names/link bytes | 16 MiB |
| Estimated parsed metadata | 128 MiB |
| Estimated serialized listing JSON | 64 MiB |
| Individual path | 4,096 UTF-8 bytes; 64 components |
| TAR decoded bytes while listing | Smaller of 16 GiB and four times the supplied codec memory budget |
| TAR extension metadata between yielded entries | 64 KiB, separately from the preceding file's payload allowance |
| TAR listing elapsed time | 60 seconds, checked cooperatively between reads |
| Raw and decoded 7z header buffers | 64 MiB each |
| 7z files, blocks, pack streams, total substreams | 100,000 each |
| 7z aggregate name data | 16 MiB |
| 7z public buffered extraction | 256 MiB of cumulative decoded data per operation |

The metadata and JSON limits are conservative accounting bounds, not a promise about peak process RSS. Codec workspaces and raw input buffers have separate limits. The preexisting 256 MiB raw ZIP central-directory limit remains in place. The 7z header decoder's workspace is capped at 64 MiB.

For native `read_file`, the 256 MiB budget covers the requested entry plus any earlier entries in its solid block that must be decoded to reach it. Earlier entries are drained to a sink instead of retained in vectors. Thus a small file late in a large solid block can be refused. The WASM helper charges all returned entries against one 256 MiB budget; conversion to JavaScript arrays can require additional memory. Native streaming callbacks do not have this 256 MiB per-file or aggregate buffered-output cutoff, and the regression suite explicitly accepts a 300 MiB streamed entry. Header and structural limits still apply to streaming readers.

TAR extraction first lists the archive, so the listing limits also gate that operation. A large compressed TAR may require a substantial decoded scan even to list its names; exceeding the decoded-byte or time budget returns `E_RESOURCE_LIMIT` rather than a partial successful result. The time limit cannot forcibly interrupt a single blocking system or codec read.

Repair, join, and split now refuse an existing destination, including a dangling symlink. Editing is the intentional replacement operation. ZIP edit validation checks the rebuilt archive structure and expected entry count; unchanged payloads, including encrypted or unsupported methods, are copied byte-for-byte rather than decrypted and CRC-tested again. Split publication is atomic per volume; if a later volume fails, already published new volumes may remain.

## Filesystem guarantees and scope

Unix default 7z helper extraction uses directory descriptors for ancestor resolution and final writes. It verifies decoded length and CRC before publishing the staged file. This closes the reported ancestor-symlink redirection, including replacement between helper dispatch and default extraction.

Custom extraction callbacks remain responsible for the safety of their own writes. The high-level helper performs path checks before dispatch, but those checks cannot make arbitrary callback code race-safe. Non-Unix default helper code performs component checks and staged publication without the Unix descriptor-relative guarantee. Other engine extraction paths have their existing path checks; this remediation does not claim to eliminate every possible ancestor-renaming race across every format and operation.

The dependency lock change adds `rustix` and `tempfile` to the vendored 7z crate's dependencies. Both packages already existed in the engine lockfile; no package versions or checksums changed.

## Verification

The following Linux command passed **177 tests**: 107 library tests, 6 edit/repair tests, 16 format roundtrips, 13 security finding regressions, and 35 ZIP roundtrips.

```sh
CARGO_PROFILE_RELEASE_LTO=false cargo test --locked --release --no-default-features \
  --manifest-path engine/fz-core/Cargo.toml \
  --test security_findings --test edit_repair --test formats_roundtrip \
  --test zip_roundtrip --lib -- --test-threads=1
```

The identical 13 security regressions were copied into a clean archive of the baseline revision and run against its unmodified engine: **all 13 failed**, establishing that the tests exercise the vulnerable behavior rather than merely mirroring the fixes. The fixed tree passed all 13 again after the final checked-conversion and test-log cleanup.

Default-feature type checking also succeeded:

```sh
cargo check --locked --manifest-path engine/fz-core/Cargo.toml
```

The vendored WASM source compiled successfully with:

```sh
cargo check --manifest-path engine/vendor/sevenz-rust2/Cargo.toml \
  --target wasm32-unknown-unknown --no-default-features --features default_wasm
```

Local validation used Rust 1.99.0. The Linux container has no system libarchive, so linked tests disable the optional `legacy` feature. Its release thin-LTO link also failed in existing zlib/rayon dependencies; the environment override above disabled LTO for local verification without changing repository profiles. The project's macOS CI remains the gate for default-feature linking, Apple-specific behavior, and the Swift app. The WASM check is compilation coverage, not a browser runtime test. Tests invoking an external system `7z` are conditional on that executable being installed.
