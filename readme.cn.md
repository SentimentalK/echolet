# Echolet

<p align="center">
  <strong>快速、私密、纯本地运行的桌面端语音输入法。</strong><br>
  按下 <kbd>F10</kbd>，开口说话，文字即可实时上屏至当前激活的任何应用。
</p>

<p align="center">
  <a href="https://github.com/SentimentalK/echolet/releases"><img src="https://img.shields.io/github/v/release/SentimentalK/echolet?include_prereleases&style=flat-square" alt="Release"></a>
  <a href="https://github.com/SentimentalK/echolet/actions"><img src="https://img.shields.io/github/actions/workflow/status/SentimentalK/echolet/ci.yml?branch=master&style=flat-square" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square" alt="License"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform">
</p>

<p align="center">
  <a href="readme.md">English</a> • <a href="readme.cn.md">中文说明</a>
</p>

---

## 核心特性

- **100% 本地离线 & 隐私安全**：无需云端连接、无订阅制、零音频上传。模型完全在本地 CPU 上运行。
- **原生文本无缝上屏**：直接向当前聚焦的代码编辑器、浏览器、终端或任意文本框模拟输入，无需手动复制粘贴。
- **中英混读 & 开发者友好**：搭载 [X-ASR](https://github.com/GilgameshWind) Zipformer 流式模型，流畅识别中英文混合语句及技术开发词汇（例如：“Check 这个 Docker 容器的 logs”）。
- **实时流式响应 & 智能 Diff 回改**：边说边出字，识别过程中根据语义上下文自动退格修正早期预测，文字自然流畅。
- **全平台支持**：支持 Windows、macOS 与 Linux，轻量常驻系统托盘。

---

## 快速上手

### 1. 下载安装
前往 [GitHub Releases](https://github.com/SentimentalK/echolet/releases) 页面下载适合您系统的预编译压缩包。

| 操作系统 | 软件包 | 说明 |
| :--- | :--- | :--- |
| **Windows** | `echolet-windows-x86_64.zip` | 解压后直接运行 `echolet.exe` |
| **macOS (Apple Silicon)** | `echolet-macos-aarch64.tar.gz` | 首次运行需授权辅助功能与麦克风权限 |
| **Linux (x86_64)** | `echolet-linux-x86_64.tar.gz` | 支持 GNOME、KDE、X11 及 Wayland 环境 |

### 2. 启动与使用
1. **启动程序**：直接运行 `echolet`，程序将自动最小化并在系统托盘常驻。
2. **定位光标**：点击任意需要输入的文本区域（如 VS Code、浏览器、终端、聊天窗口等）。
3. **语音打字**：
   - 按下 <kbd>F10</kbd>（或点击托盘菜单）**开始录音**。
   - 自然开口说话。
   - 再次按下 <kbd>F10</kbd>（或点击托盘菜单）**结束录音**。

---

## 平台设置与权限配置

### Linux
Echolet 使用 `uinput` 实现 X11 / Wayland 环境下的低延迟虚拟按键模拟。
- **一键权限配置**：首次运行前执行配置命令：
  ```bash
  echolet setup-uinput
  ```
  *(根据终端提示将当前用户加入 `input` 用户组并重载 udev 规则，必要时注销重新登录)*。
- **前台调试模式**：
  ```bash
  echolet -f
  ```

### macOS
- 首次启动时，请在【系统设置 → 隐私与安全性】中授予 **麦克风** 与 **辅助功能 (Accessibility)** 权限。
- Echolet 将常驻在顶部菜单栏。

### Windows
- 早期测试版本如遇到 SmartScreen 提示，请点击 *更多信息 → 仍要运行*。
- Echolet 启动后默认在系统右下角托盘静默运行。

---

## 命令行与快捷控制

Echolet 支持通过系统托盘或命令行子命令进行控制：

```bash
# 切换录音状态（开始 / 停止）
echolet toggle

# 查看当前运行状态
echolet status

# 优雅退出 Echolet 后台服务
echolet stop

# 在前台运行并输出详细日志 (Linux)
echolet -f
```

---

## 架构与技术原理

```
[ 麦克风音频采集 ] ──> [ sherpa-onnx / X-ASR 离线推理 ] ──> [ Diff 差异对比引擎 ] ──> [ 系统原生按键/剪贴板模拟 ]
```

- **核心语言**：基于 [Rust](https://www.rust-lang.org/) 构建，具备极低的内存占用、秒级即时响应与出色的系统稳定性。
- **推理引擎**：基于 [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 与 [ONNX Runtime](https://onnxruntime.ai/) 进行高效率 CPU 离线计算。
- **声学与语言模型**：采用 [X-ASR](https://github.com/GilgameshWind) 双语 Zipformer 流式模型（480ms 分块），针对中英混杂与技术开发场景专门优化。
- **Diff 修正引擎**：实时计算前后识别文本的最长公共前缀与差异，自动发送退格键与新增字符，实现流式回退修正。

---

## 隐私与配置

- **音频隐私**：所有语音数据仅在内存中处理，识别完成后立即释放，绝不会上传云端或落盘保存音频。
- **配置文件**：路径位于 `~/.echolet/config.json`。
- **转录历史**：历史记录默认关闭。如需开启可在托盘菜单或配置文件中设置。

---

## 源码编译构建

### 环境要求
- [Rust 工具链](https://www.rust-lang.org/tools/install) (建议 1.75+)
- CMake, Git 及系统编译工具链

### 编译步骤
```bash
# 克隆仓库
git clone https://github.com/SentimentalK/echolet.git
cd echolet

# 获取官方基础模型并编译发布版本
./scripts/acquire-base-model.sh
cargo build --release
```

---

## 开源许可证与致谢

Echolet 采用 [Apache License 2.0](LICENSE) 开源许可证。

衷心感谢以下开源项目与贡献者：
- [k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) - 新一代端到端语音识别框架。
- [GilgameshWind / X-ASR](https://github.com/GilgameshWind) - 优秀的开源中英双语流式语音识别模型。
- [Microsoft ONNX Runtime](https://onnxruntime.ai/) - 高性能跨平台机器学习推理引擎。
