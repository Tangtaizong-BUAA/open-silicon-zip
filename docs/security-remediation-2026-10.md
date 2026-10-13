# October 2026 security remediation

This change addresses the 1 high and 12 medium Codex Security repository-scan findings reviewed against main commit `4c42de2d1f459d234d460f1524e2e5c3034a8011`. The scan was based on `6b703c9b8f30673c964cb9b56ff36bec67deb85d`; newer main changes were retained.

## Finding coverage

| Finding | Severity | Remediation and regression evidence |
| --- | --- | --- |
| `csf_05b715a609ef26c4f3f150eb` | High | URL, inbox and Services requests require an explicit in-app review of all source paths, output paths and deletion effects before file operations. Every source is checked against canonical configured roots. Mixed-directory input must use a user-selected destination. `ExternalRequestSecurityTests`, `SecurityBoundaryTests`. |
| `csf_9138ebcb7ee1d88a57efecee` | Medium | Settings import preserves the locally configured engine executable; changing it requires the local Advanced settings control. Import regression covers fresh and existing settings. |
| `csf_dabbda3e08ae130e8a851782` | Medium | Effective deletion is set from the external request after presets have been merged; false cannot be overridden by a destructive preset. Regression checks both directions. |
| `csf_473d543f1f4ceae431672684` | Medium | Admission before collecting events and building indexes limits entries, path depth, metadata bytes, expanded directory count and prefix bytes. Deep-path regression rejects a 30,000-component input before recursive tree work. |
| `csf_4301d336057a6ca79f4cb629` | Medium | Checked UInt64 arithmetic for archive and folder totals turns forged ZIP64 aggregate overflow into an error. Tests exercise both logical and compressed sizes. |
| `csf_186ec9418bf51b72bff721ce` | Medium | Plaintext extraction uses private 0700 leased session directories. Preview replacement, window closure, errors/cancellation and application exit release files; canceled work waits for engine termination before deletion. Startup reclaims abandoned leased sessions while preserving active instances. Existing legacy temporary directories from older releases are not removed automatically. |
| `csf_39cd5e06b90d6ac6e666170d` | Medium | 7z lengths remain unsigned; exact-length/CRC validation also covers skipped solid members and publication. Huge-length tests preserve an existing destination on failure. |
| `csf_21bb1416561c45988ceff565` | Medium | TAR listing has decoded-byte, time, cancellation and metadata admission limits, including GNU/PAX metadata that the tar library buffers before yielding entries. |
| `csf_d3e918ca0a343531a2285028` | Medium | ZIP symlinks obey overwrite/skip/rename/ask just like regular files. Invalid targets never remove the original leaf; publication uses a temporary link. |
| `csf_f71b8dc4c59f7f3b84fe96d3` | Medium | The vendored 7z default extraction helper checks components and, on Unix, binds ancestors with directory descriptors and NOFOLLOW, writes in private staging, and publishes only after integrity validation. |
| `csf_9addcf3d65c854788eb75df3` | Medium | 7z raw/decoded header limits and cumulative read-file/WASM buffering limits protect in-memory APIs. Large ordinary streaming extraction remains supported. |
| `csf_1d1e1601f16cea22dc28b902` | Medium | ZIP editing uses a random sibling staging file and safe replacement. Repair/join/split use exclusive creation and refuse existing destinations, including symlinks. |
| `csf_2a56c58f3641f4ad83bf2aa3` | Medium | ZIP admission counts actual parsed records, independent of the advertised EOCD count, and bounds expanded metadata/JSON work before allocation grows without limit. |

The Rust details, limits and reproducible commands are in [the engine report](security-engine-remediation-2026-10.md). The attack cases are synthetic and do not use user data.

## Observable changes

- External automation now displays a confirmation dialog. Cancel is the default. This is also required for Finder/Services handoffs because the transport cannot authenticate the originating application.
- Importing preferences no longer changes the executable path. A local user can still configure it in Advanced preferences.
- Archive lists accept at most 100,000 entries, 4,096 UTF-8 bytes per path, 64 components and 16 MiB of strings. Index construction also caps directory nodes at 100,000 and expanded prefix strings at 32 MiB. Refusal is explicit (`E_RESOURCE_LIMIT`), never a truncated listing.
- `ArchiveIndex(entries:)` is now throwing; the empty `ArchiveIndex()` initializer remains available.
- Temporary opened files are intended for viewing. Their lifetime ends when the archive browser closes or the app exits; save a separate copy if an external editor needs to keep changes.
- Existing outputs passed to repair/join/split must be removed or renamed explicitly; a failed operation preserves prior data.
- Unix default extraction has descriptor-based defenses. Callers supplying their own vendored 7z extraction callback remain responsible for that callback's filesystem writes.

## Validation and release status

Local Linux validation includes the Rust attack/roundtrip suite, a WASM build, and all three localization tables. macOS GitHub Actions runs the repository's unmodified release-feature Rust tests, Swift tests, security/localization regressions, compatibility suite and ARM64 app packaging. Consult PR checks for the final commit's results.

This is a source fix. Existing installed apps remain unchanged until a fixed release is built and installed. The original repository-scan findings remain in Codex Security until the changes are merged and the scanner verifies the updated default branch.
