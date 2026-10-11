# Security

Open Silicon Zip processes local archives and files. Normal operation does not send archive contents or passwords to a service. The Finder extension is sandboxed; the main app and its engine run with the user's file permissions. It is not a sandbox for arbitrary untrusted native code.

Implemented defenses include:

- Absolute paths, parent traversal, drive prefixes, NUL bytes, existing symlink descendants and colliding normalized entry names are refused.
- ZIP central-directory size is bounded **before** allocation; stream decoding refuses expansion beyond the declared entry size. Whole-buffer deflate decoding has a per-worker memory threshold.
- Regular extracted files use sibling temporary files; the ZIP original is replaced only after size/CRC/MAC validation. Cancel cleanup does not delete pre-existing overwritten files.
- 7z creation refuses stored symlinks rather than silently reading their targets; select ZIP/TAR to preserve links, or explicitly enable dereferencing.
- Temporary archive creation uses unpredictable exclusive temporary files instead of predictable `.part` paths.
- CLI SIGINT/SIGTERM reaches cooperative cancellation for archive operations; application quit also resumes and terminates its paused/running children.
- GUI passwords travel over an anonymous stdin pipe, not arguments or environment variables. CLI users should use `--password-stdin` too.
- Refused, failed or skipped extraction entries do not qualify for automatic source deletion. Destructive external requests require an in-app confirmation. Compression source deletion is chained after archive testing.
- URL requests require absolute, non-traversing paths. Finder callbacks use integer action tokens and explicitly launch their containing app.
- Malformed password-store KDF parameters and unsupported envelope versions are rejected before key derivation.

## Boundaries

The regression suite is targeted testing, not proof of absence of vulnerabilities or an independent security audit. Directory checks are not descriptor-relative `openat` traversal: a different local process concurrently replacing directories can still race them. Use a fresh private destination for hostile archives. 7z worker memory accounting is an estimate, not a hard per-process allocation cap. Advertised valid large sizes can still exhaust disk space; there is no global extraction quota or full transaction rollback across an entire archive. Codec and system-library vulnerabilities remain dependency risks. ZIP multi-volume publication is not a transaction across all volumes. Legacy text-stream decoding and uncommon archive variants need further corpus coverage.

The release pipeline in this repository builds a local development app. Developer ID notarization and Gatekeeper distribution acceptance are separate and are not claimed.

Do not submit passwords, personal archives or credentials in public issues. Report a minimal synthetic reproducer privately through the repository owner's available GitHub security reporting channel.
