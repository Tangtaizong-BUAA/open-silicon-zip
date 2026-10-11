# open-silicon-zip

为 Apple 芯片 Mac 开发的原生压缩与解压工具。SwiftUI 界面、Rust 引擎，默认简体中文，可在设置中切换 English 或繁體中文。

Native macOS archiver for Apple silicon, with a SwiftUI interface and a Rust engine. Chinese by default; English and Traditional Chinese are selectable in Settings → Language. Restart after changing the interface language.

## 功能 / Features

- Finder 一级右键菜单：直接压缩为 ZIP / 7z，智能解压、解压到当前目录或指定文件夹。
- ZIP/ZIPX、7z、TAR 及其压缩流；RAR 和多种旧格式通过读取后端支持。
- 压缩包浏览与预览、ZIP 编辑与修复、任务队列、校验和、加密密码管理。
- ARM64 原生构建、性能核心感知线程池、按内存预算限制的 7z 并行分块、随应用打包的 liblzma、较小的大文件缓冲区。
- 密码经匿名管道传递；拒绝目录穿越与符号链接重定向；ZIP 覆盖文件在校验通过后原子提交。

安装后请在系统设置中启用 **Open Silicon Zip Finder** 扩展；设置 → 语言可以切换中英文，重启生效。7z 暂不存储符号链接：可选用 ZIP/TAR 保留链接，或在压缩设置中明确选择读取链接目标。

## Build

Sources target macOS 13+ on Apple silicon. Building requires Xcode, Rust 1.94+, Homebrew `xz` and [XcodeGen](https://github.com/yonaskolb/XcodeGen). The default binary also links the system libarchive. Internal Swift targets retain their original `FastZip` names; the product and bundle identifiers are Open Silicon Zip / `org.opensiliconzip.mac`.

```sh
cargo build --locked --release --manifest-path engine/fz-core/Cargo.toml
swift test --package-path app
bash app/package-app.sh
```

The packaged minimum macOS version is raised when the bundled liblzma requires a newer OS (the locally validated build requires macOS 15+).

The app is written to `app/dist-stage/Open Silicon Zip.app`. The packaging script builds from source and verifies nested code signatures. It does not install the app or restart Finder. It defaults to ad-hoc signing; set `SIGN_IDENTITY` to your own valid signing identity for a locally signed Finder extension:

```sh
SIGN_IDENTITY='Apple Development: YOUR IDENTITY' bash app/package-app.sh
```

Copy the app to Applications and launch it. Enable **Open Silicon Zip Finder** in System Settings → General → Login Items & Extensions → Finder Extensions. If Finder cached an older extension, relaunch Finder after enabling it. Settings → Finder Menu controls the menu layout; direct actions are the default. Cloud-provider-managed folders may have separate Finder behavior.

Local development signatures are **not notarized distribution**. This repository does not claim Developer ID / Gatekeeper acceptance for downloads.

## Validation

```sh
RUST_TEST_THREADS=1 cargo test --locked --release --manifest-path engine/fz-core/Cargo.toml
swift test --package-path app
python3 scripts/security-regression.py
python3 scripts/check-localization.py
# With cargo-audit installed:
cargo audit --file engine/fz-core/Cargo.lock
# Same corpus, alternating binary order, three repetitions:
python3 bench/compare.py /path/to/baseline/fz engine/fz-core/target/release/fz
python3 bench/large-store.py /path/to/baseline/fz engine/fz-core/target/release/fz
```

See [validation](docs/VALIDATION.md), [security boundaries](SECURITY.md), and [dependency licenses](THIRD_PARTY_NOTICES.md). Optional external-format fixtures are skipped when their tools are unavailable; those skips are not compatibility passes.

## License

Original code is MIT licensed. Vendored `sevenz-rust2` retains its Apache-2.0 license. RAR support includes UnRAR under its own license, including restrictions on re-creating the RAR compression algorithm; the MIT license does not override dependency terms. See `THIRD_PARTY_NOTICES.md`.
