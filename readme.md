# Echolet

<p align="center">
  <strong>Local AI Voice Typing for Windows, macOS & Linux</strong><br>
  Real-time, private, on-device speech-to-text. Press <kbd>F10</kbd>, speak, and text flows directly into your active app.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/AI-Local%20AI-blueviolet?style=flat-square" alt="Local AI">
  <img src="https://img.shields.io/badge/Inference-100%25%20On--Device-success?style=flat-square" alt="100% On-Device">
  <a href="https://github.com/SentimentalK/echolet/releases"><img src="https://img.shields.io/github/v/release/SentimentalK/echolet?include_prereleases&style=flat-square" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square" alt="License"></a>
</p>

<p align="center">
  <a href="readme.md">English</a> • <a href="readme.cn.md">中文说明</a>
</p>

---

## Highlights

- **100% On-Device AI**: Speech recognition runs locally on your CPU. No cloud inference, no API key, no account, and zero audio uploads.
- **Native OS Text Injection**: Injects transcribed text directly into any focused editor, browser, terminal, or app without copy-pasting.
- **Powered by Local AI**: Runs the [X-ASR](https://github.com/GilgameshWind) bilingual Zipformer streaming speech model entirely on CPU, seamlessly recognizing mixed Chinese/English and developer terminology (e.g., *"Check 这个 Docker 容器的 logs"*).
- **Real-Time Streaming & Diff Correction**: Words stream onto the screen as you speak, with instant diff-based backtracking and automatic corrections.
- **Cross-Platform**: Lightweight background utility with system tray integration on Windows, macOS, and Linux.

---

## Quick Start

### 1. Download
Download the latest pre-built package for your operating system from [GitHub Releases](https://github.com/SentimentalK/echolet/releases).

| Platform | Package | Architecture |
| :--- | :--- | :--- |
| **Windows** | `echolet-windows-x64.zip` | x86_64 |
| **macOS** | `echolet-macos-arm64.zip` | Apple Silicon (M1/M2/M3/M4) |
| **macOS** | `echolet-macos-x64.zip` | Intel x86_64 |
| **Linux** | `echolet-linux-x64.tar.gz` | x86_64 |
| **Linux** | `echolet-linux-arm64.tar.gz` | ARM64 |

### 2. Launch & Use
1. **Launch**: Run `echolet`. It starts minimized in your system tray / menu bar.
2. **Focus**: Click into any text field (VS Code, browser, terminal, Slack, Notepad, etc.).
3. **Voice Type**:
   - Press <kbd>F10</kbd> (or click the tray menu) to **Start Listening**.
   - Speak naturally.
   - Press <kbd>F10</kbd> again (or click tray) to **Stop Listening**.

---

## Platform Setup & Permissions

### Linux
Echolet uses `uinput` for low-latency keystroke injection across X11 and Wayland sessions.
- **Initial Setup**: Run the setup command once to configure uinput permissions:
  ```bash
  echolet setup-uinput
  ```
  *(Follow the prompt to add your user to the `input` group and reload udev rules, then re-login if required).*
- **Foreground Mode**: To run with live terminal logs:
  ```bash
  echolet -f
  ```

### macOS
- When launched for the first time, grant **Microphone** and **Accessibility** permissions under *System Settings → Privacy & Security*.
- Echolet lives in your macOS Menu Bar.

### Windows
- For early releases, you may need to click *More info → Run anyway* on Windows Defender SmartScreen.
- Echolet runs silently in the Windows system tray notification area.

---

## CLI & Controls

Echolet supports control via system tray or command-line interface:

```bash
# Toggle recording on / off
echolet toggle

# Check running status
echolet status

# Gracefully quit Echolet
echolet stop

# Run in foreground with detailed debug logging (Linux)
echolet -f
```

---

## Under the Hood

```
[ Microphone ] ──> [ sherpa-onnx / X-ASR ] ──> [ Diff Correction Engine ] ──> [ OS Keystroke Injection ]
```

- **Core Engine**: Written in [Rust](https://www.rust-lang.org/) for memory safety, instant startup, and minimal CPU footprint.
- **Inference Runtime**: Powered by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) using [ONNX Runtime](https://onnxruntime.ai/).
- **Acoustic Model**: [X-ASR](https://github.com/GilgameshWind) bilingual Zipformer streaming model (480ms chunk), optimized for mixed Chinese/English speech and coding terminology.
- **Diff Correction**: Live real-time diff tracker that emits dynamic backspaces and character insertions to adjust partial recognitions on the fly.

---

## Privacy & Configuration

- **Audio Privacy**: Audio data is processed strictly in memory and discarded immediately after transcription. No audio is ever stored or uploaded.
- **Configuration**: Stored at `~/.echolet/config.json`.
- **History Storage**: Transcription history is disabled by default. You can opt-in via tray menu settings or configuration.

---

## Building from Source

### Prerequisites
- [Rust](https://www.rust-lang.org/tools/install) (1.75+ recommended)
- CMake, Git, and build tools

### Build
```bash
# Clone the repository
git clone https://github.com/SentimentalK/echolet.git
cd echolet

# Acquire official base model and build release binary
./scripts/acquire-base-model.sh
cargo build --release
```

---

## License & Acknowledgements

Echolet is released under the [Apache License 2.0](LICENSE).

Special thanks to the following open-source projects and communities:
- [k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) - Next-generation speech recognition framework.
- [GilgameshWind / X-ASR](https://github.com/GilgameshWind) - Bilingual Zipformer streaming speech recognition models.
- [Microsoft ONNX Runtime](https://onnxruntime.ai/) - High-performance cross-platform ML engine.