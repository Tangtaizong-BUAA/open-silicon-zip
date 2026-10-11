# Changelog

## 0.2.0-preview.1 — 2026-10-11

首次公开的 Apple silicon 预览版。提供 ARM64 DMG、ZIP 和 SHA-256 校验文件；应用内版本号为 0.2.0。

### 功能与修复

- Finder 一级压缩 / 解压动作、选项传递及主应用启动流程修复。
- 默认简体中文；支持 English / 繁體中文，补齐动态数量、任务状态和时间显示。
- 任务队列取消、重试、进程退出及自动删除源文件的时序修复。
- ZIP、TAR、7z 路径及符号链接检查；损坏 ZIP 的覆盖保护；独占随机临时文件。
- 密码通过匿名管道传递，限制不可信压缩包元数据的内存申请。
- ARM64 性能核心感知线程池、7z 自适应并行分块，以及更小的 ZIP 大文件缓冲区。
- 公布本机 Bandizip、7-Zip、Info-ZIP、libarchive 对照，附原始数据、复现脚本与高亮图表。

### 下载与验证边界

- 预编译包要求 Apple 芯片和 macOS 15+；包含 liblzma，无需安装 Homebrew。
- 采用 ad-hoc 签名，未完成 Developer ID 签名 / Apple 公证，未通过 Gatekeeper 分发验证。
- Finder 扩展的实机验收使用本机开发签名；预编译 ad-hoc 包在其他 Mac 上的扩展启用未验证。
- 7z 不存储符号链接；使用 ZIP/TAR 或明确选择读取链接目标。
- 代码测试、已知安全边界及性能测量条件详见 [VALIDATION.md](docs/VALIDATION.md)、[SECURITY.md](SECURITY.md) 和 [BENCHMARKS.md](docs/BENCHMARKS.md)。
