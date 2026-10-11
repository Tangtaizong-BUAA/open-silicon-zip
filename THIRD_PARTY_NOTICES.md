# Third-party notices

The MIT license covers original Open Silicon Zip code only. Dependency licenses remain in force.

## Vendored 7z engine

`engine/vendor/sevenz-rust2` is a local fork of sevenz-rust2 0.23.0 (Apache-2.0). The modifications add the optional liblzma LZMA2 encoder and raw encoder integration. Its LICENSE is retained beside the source.

## RAR support

The `unrar` / `unrar_sys` dependencies include the UnRAR license. It permits use of the decompression code but forbids using it to re-create the proprietary RAR compression algorithm. This project exposes RAR reading only. See the source license below.

## Native libraries

The macOS build uses system libarchive and bundles the build-time liblzma when it is installed outside system paths. Retain the upstream liblzma notices when distributing binaries. macOS system libraries are supplied by Apple.

## Cargo dependency inventory

Generated from the locked dependency metadata for aarch64-apple-darwin. License expressions are upstream declarations, not a legal audit.

| Package | Version | Declared license |
|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| aes | 0.9.3 | MIT OR Apache-2.0 |
| aho-corasick | 1.1.5 | Unlicense OR MIT |
| alloc-no-stdlib | 3.0.0 | BSD-3-Clause |
| alloc-stdlib | 0.3.0 | BSD-3-Clause |
| anstream | 1.0.0 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 |
| block-padding | 0.4.2 | MIT OR Apache-2.0 |
| brotli | 9.0.0 | BSD-3-Clause AND MIT |
| brotli-decompressor | 6.0.1 | BSD-3-Clause/MIT |
| bzip2 | 0.6.1 | MIT OR Apache-2.0 |
| cbc | 0.2.1 | MIT OR Apache-2.0 |
| cc | 1.7.0 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 |
| cipher | 0.5.2 | MIT OR Apache-2.0 |
| clap | 4.6.7 | MIT OR Apache-2.0 |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 |
| cmov | 0.5.4 | Apache-2.0 OR MIT |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 |
| const-oid | 0.10.2 | Apache-2.0 OR MIT |
| cpubits | 0.1.1 | MIT OR Apache-2.0 |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 |
| ctr | 0.10.1 | MIT OR Apache-2.0 |
| ctutils | 0.4.3 | Apache-2.0 OR MIT |
| deflate64 | 0.1.12 | MIT |
| digest | 0.11.3 | MIT OR Apache-2.0 |
| either | 1.19.0 | MIT OR Apache-2.0 |
| encoding_rs | 0.8.42 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| fastrand | 2.5.0 | Apache-2.0 OR MIT |
| filetime | 0.2.29 | MIT/Apache-2.0 |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| fz-core | 0.1.0 | MIT |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hmac | 0.13.0 | MIT OR Apache-2.0 |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 |
| inout | 0.2.2 | MIT OR Apache-2.0 |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| jobserver | 0.1.35 | MIT OR Apache-2.0 |
| libbz2-rs-sys | 0.2.5 | bzip2-1.0.6 |
| libc | 0.2.190 | MIT OR Apache-2.0 |
| libdeflate-sys | 1.26.1 | Apache-2.0 |
| libdeflater | 1.26.1 | Apache-2.0 |
| lz4_flex | 0.14.0 | MIT |
| lzma-rust2 | 0.21.0 | Apache-2.0 |
| lzma-sys | 0.1.20 | MIT/Apache-2.0 |
| md-5 | 0.11.0 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| multiversion_no_op | 1.0.0 | Apache-2.0 OR MIT |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| pbkdf2 | 0.13.0 | MIT OR Apache-2.0 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| ppmd-rust | 1.5.0 | CC0-1.0 OR MIT-0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| rand | 0.10.3 | MIT OR Apache-2.0 |
| rand_core | 0.10.1 | MIT OR Apache-2.0 |
| rayon | 1.12.0 | MIT OR Apache-2.0 |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| same-file | 1.0.6 | Unlicense/MIT |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sevenz-rust2 | 0.23.0 | Apache-2.0 |
| sha1 | 0.11.0 | MIT OR Apache-2.0 |
| sha2 | 0.11.0 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| signal-hook | 0.4.5 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| syn | 3.0.7 | MIT OR Apache-2.0 |
| tar | 0.4.46 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT |
| twox-hash | 2.1.5 | MIT |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 |
| unrar | 0.5.8 | MIT OR Apache-2.0 |
| unrar_sys | 0.5.8 | MIT OR Apache-2.0 |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT |
| walkdir | 2.5.0 | Unlicense/MIT |
| widestring | 1.2.1 | MIT OR Apache-2.0 |
| xattr | 1.6.1 | MIT OR Apache-2.0 |
| xz2 | 0.1.7 | MIT/Apache-2.0 |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.23 | MIT |
| zstd | 0.14.1 | BSD-3-Clause |
| zstd-safe | 8.1.0 | BSD-3-Clause |
| zstd-sys | 2.1.1+zstd.1.5.7 | BSD-3-Clause |
