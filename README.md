# Open Silicon Zip

**为 Apple 芯片 Mac 制作的高速开源压缩软件，提供原生中文界面、Finder 右键操作和多核并行压缩。**

[![CI](https://github.com/Tangtaizong-BUAA/open-silicon-zip/actions/workflows/ci.yml/badge.svg)](https://github.com/Tangtaizong-BUAA/open-silicon-zip/actions/workflows/ci.yml)
[![Preview release](https://img.shields.io/badge/release-v0.2.0--preview.1-orange)](https://github.com/Tangtaizong-BUAA/open-silicon-zip/releases/tag/v0.2.0-preview.1)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Apple Silicon](https://img.shields.io/badge/macOS-Apple_Silicon-111827)](#构建与安装)

[下载](#下载) · [性能实测](#性能实测) · [功能](#功能) · [构建与安装](#构建与安装) · [开发与贡献](#开发与贡献) · [安全说明](SECURITY.md)

## 下载

**[下载 macOS ARM64 预览版（DMG）](https://github.com/Tangtaizong-BUAA/open-silicon-zip/releases/download/v0.2.0-preview.1/OpenSiliconZip-0.2.0-preview.1-macos-arm64.dmg)** · [ZIP](https://github.com/Tangtaizong-BUAA/open-silicon-zip/releases/download/v0.2.0-preview.1/OpenSiliconZip-0.2.0-preview.1-macos-arm64.zip) · [SHA-256](https://github.com/Tangtaizong-BUAA/open-silicon-zip/releases/download/v0.2.0-preview.1/SHA256SUMS.txt) · [更新记录](CHANGELOG.md)

适用于 **Apple 芯片 / macOS 15+**。`v0.2.0-preview.1` 是 ad-hoc 签名预览版，尚未完成 Developer ID 签名与 Apple 公证，未通过 Gatekeeper 分发验证。Finder 扩展已在本机开发签名环境中验证；预览包在其他 Mac 上的扩展启用尚未完成验证。完整状态见 [Release 说明](https://github.com/Tangtaizong-BUAA/open-silicon-zip/releases/tag/v0.2.0-preview.1)。

## 性能实测

在 **M4 Max / 48 GiB** 上，与本机 **Bandizip 365、7-Zip、Info-ZIP、libarchive** 使用同一份 128 MiB 合成数据比较；每项三轮，全部 **30 次压缩和 30 次解压**通过逐文件 SHA-256 校验。

**绿色 + 粗体：该列最优。浅蓝色：第二优。↓ 越低越好。** ZIP 与 7z 分开比较；不设置综合分数。

[![ZIP 与 7z 性能对照：每列最优值加粗标绿，第二优标蓝；覆盖压缩时间、解压时间、输出大小及峰值内存。](docs/benchmarks/comparison.svg)](docs/BENCHMARKS.md)

- **速度与内存取舍：** 在此语料上，关闭高速模式的 7z 压缩约 10.0 秒；7-Zip 约 36.8 秒，Bandizip 约 35.7 秒。本项目峰值 RSS 约 1.45 GiB，更高；输出比后两者分别大约 0.58% 和 0.96%。
- **默认策略单独列出：** 高速模式会按扩展名直接存储 `.bin` 和 `.mp4`，这份数据中的 `.bin` 可压缩，因此默认模式的包明显更大。
- **计时边界：** Bandizip 通过官方命令行启动完整应用，包含初始化成本；其余行是 CLI 引擎。各软件保留默认线程、字典与分块策略；解压各自生成的包。结果代表这次工作负载，不是所有场景的排名。

[完整报告与可复制表格](docs/BENCHMARKS.md) · [原始样本 JSON](docs/benchmarks/competitors-m4-max.json) · [复现脚本](bench/competitors.py) · [优化前后对照](docs/VALIDATION.md#performance)

## 功能

| 功能 | 说明 |
|---|---|
| Finder 一级右键菜单 | 直接压缩为 ZIP / 7z，智能解压、解压到当前目录或指定文件夹 |
| 多格式支持 | ZIP / ZIPX、7z、TAR 及其压缩流；RAR 和多种旧格式通过读取后端支持 |
| 原生桌面界面 | SwiftUI，默认简体中文，可切换 English / 繁體中文 |
| 压缩包管理 | 浏览与预览、ZIP 编辑与修复、任务队列、校验和、加密密码管理 |
| Apple 芯片优化 | ARM64 Rust 引擎，性能核心感知线程池，按内存预算限制的 7z 并行分块 |
| 文件安全 | 密码经匿名管道传递，路径穿越与符号链接重定向检查，ZIP 校验后原子覆盖 |

## 构建与安装

可下载上方预览包，或按以下步骤从源码构建。DMG 内可将应用拖入「Applications」；预览包的签名与扩展限制见下载说明。

**环境要求：** Apple 芯片 Mac、Xcode、Rust 1.94+、Homebrew `xz` 和 [XcodeGen](https://github.com/yonaskolb/XcodeGen)。源码目标为 macOS 13+；打包脚本会根据 liblzma 的实际要求提高最低系统版本，本机验证包为 macOS 15+。

```sh
git clone https://github.com/Tangtaizong-BUAA/open-silicon-zip.git
cd open-silicon-zip
brew install xz xcodegen
bash app/package-app.sh
```

生成位置：`app/dist-stage/Open Silicon Zip.app`。复制到「应用程序」并启动，然后在 **系统设置 → 通用 → 登录项与扩展 → Finder 扩展**中启用 **Open Silicon Zip Finder**。

打包默认使用 ad-hoc 签名。需要本地签名的 Finder 扩展时，使用你自己的有效签名身份：

```sh
SIGN_IDENTITY='Apple Development: YOUR IDENTITY' bash app/package-app.sh
```

若 Finder 缓存了旧扩展，可在启用后重新启动 Finder。菜单布局可在应用设置中调整；中英文切换位于「设置 → 语言」，重启生效。云盘托管目录的菜单行为可能不同。内部 Swift target 仍保留 `FastZip` 名称，产品 bundle ID 为 `org.opensiliconzip.mac`。

<details>
<summary>只构建并使用命令行引擎</summary>

```sh
cargo build --locked --release --manifest-path engine/fz-core/Cargo.toml

# 压缩与解压
engine/fz-core/target/release/fz create output.zip /path/to/folder
engine/fz-core/target/release/fz extract output.zip -o /path/to/output

# 充分压缩 .bin 等默认跳过的扩展名
engine/fz-core/target/release/fz create output.7z /path/to/folder --high-speed off
```

7z 暂不存储符号链接；可用 ZIP / TAR 保留链接，或明确选择 `--dereference` 读取目标。

</details>

## 开发与贡献

欢迎提交可复现的问题和 Pull Request。报告问题时请包含 macOS / 芯片型号、压缩格式、复现步骤及经过脱敏的日志；尽量提供可公开的最小合成样本。安全问题请参照 [SECURITY.md](SECURITY.md) 私下报告。

| 目录 | 内容 |
|---|---|
| `app/` | SwiftUI 应用、Finder 扩展与 Swift 测试 |
| `engine/fz-core/` | Rust 压缩引擎和命令行工具 |
| `bench/` | 基准运行、对照和图表生成脚本 |
| `scripts/`、`compat-suite/` | 安全、汉化和格式兼容验证 |
| `docs/` | 验证报告、Benchmark 与原始测量数据 |

提交修改前运行相关验证：

```sh
RUST_TEST_THREADS=1 cargo test --locked --release --manifest-path engine/fz-core/Cargo.toml
swift test --package-path app
cargo build --locked --release --manifest-path engine/fz-core/Cargo.toml
python3 scripts/security-regression.py
python3 scripts/check-localization.py
```

性能修改应附上同一数据集的耗时、输出大小和内存对照，避免只报告单一速度指标。可安装 `sevenzip` 后运行：

```sh
python3 bench/competitors.py --output competitors.json
# 图表生成另需 matplotlib：
python3 bench/render.py
```

验证覆盖范围、外部工具缺失导致的跳过项，以及未运行的大文件测试，均记录在 [VALIDATION.md](docs/VALIDATION.md)。安全边界见 [SECURITY.md](SECURITY.md)，不将有限测试等同于不存在漏洞。

## 许可证

本项目原创代码使用 [MIT License](LICENSE)。Vendored `sevenz-rust2` 保留 Apache-2.0 许可证；RAR 读取包含 UnRAR，其许可证对重建 RAR 压缩算法等另有约束，MIT 不覆盖这些依赖条款。完整说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

---

**Open Silicon Zip is an open-source macOS archiver for Apple silicon, with a native interface, Finder context-menu actions and parallel compression.** English is available in Settings → Language.
