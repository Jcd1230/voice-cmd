# voice-cmd

`voice-cmd` is a Linux-first, local voice-to-text CLI daemon for Wayland desktops (tested with KDE Plasma).

It captures microphone audio, segments speech with VAD, transcribes with a local Parakeet model, and sends text to a configurable output hook (default: `ydotool type`).

## Features

- Local transcription (Parakeet V2 default)
- PipeWire/CPAL audio capture
- VAD-based chunking (Silero) with configurable thresholds/timings
- Background daemon with UNIX socket IPC
- `toggle` auto-starts daemon if it is not running
- Runtime config reload for hook/history settings
- In-memory transcription history (`voice-cmd history`)
- Optional Wayland overlay indicator (`voice-cmd-overlay`)
- Configurable output command and sound hook
- Model prefetch command: `voice-cmd model fetch`
- Built-in diagnostics (`voice-cmd doctor`)

## Requirements

- Linux (Wayland)
- Rust toolchain (if building from source)
- GTK4 + layer-shell runtime/build deps for overlay
- `ydotool` (if you use default output command)

Ubuntu/Debian packages typically needed for overlay builds:

```bash
sudo apt update
sudo apt install -y libgtk-4-dev libgtk4-layer-shell-dev
```

## Installation

### Option 1: `mise` GitHub backend (release assets)

If this repo publishes prebuilt GitHub release assets:

```bash
mise use -g github:Jcd1230/voice-cmd
```

### Option 2: `mise` cargo backend (build from source)

```bash
mise use -g rust
mise use -g "cargo:https://github.com/Jcd1230/voice-cmd@branch:main"
```

### Option 3: Manual build/install

```bash
git clone https://github.com/Jcd1230/voice-cmd.git
cd voice-cmd
cargo build --release --bin voice-cmd --bin voice-cmd-overlay
install -Dm755 target/release/voice-cmd ~/.local/bin/voice-cmd
install -Dm755 target/release/voice-cmd-overlay ~/.local/bin/voice-cmd-overlay
```

### Local dev install via repo `mise` tasks

```bash
mise run install-all
```

This installs debug binaries into `~/.local/bin`.

## Quick Start

1. Initialize config:

```bash
voice-cmd config --init
```

2. Pre-download model:

```bash
voice-cmd model fetch
```

3. Start daemon (overlay enabled by default):

```bash
voice-cmd daemon --fork
```

4. Toggle recording on/off:

```bash
voice-cmd toggle
```

5. Stop daemon:

```bash
voice-cmd shutdown
```

## Commands

```text
voice-cmd daemon         Start daemon
voice-cmd toggle         Toggle recording
voice-cmd start          Start recording
voice-cmd stop           Stop recording
voice-cmd status         Recording status
voice-cmd daemon-status  Daemon reachability + status
voice-cmd reload         Reload runtime config in daemon
voice-cmd history        Show recent transcriptions
voice-cmd doctor         Print diagnostics
voice-cmd send <text>    Send text directly to output hook
voice-cmd shutdown       Stop daemon
voice-cmd config         Print config path
voice-cmd model fetch    Download model
voice-cmd model status   Show model readiness
voice-cmd audio devices  List available input devices
```

Overlay command:

```bash
voice-cmd-overlay --help
```

## Config

Default config path:

```text
~/.config/voice-cmd/config.toml
```

Important sections:

- `[model]`: model path, quantization, download URL
- `[vad]`: speech detection and silence timing
- `[audio]`: frame size/sample settings
- `[audio.input_device]`: optional input device name (exact or partial match)
- `[output]`: command with `{text}` placeholder
- `[sound]`: audio feedback command / builtin tone behavior
- `[history]`: in-memory history size
- `[ipc]`: custom socket path

## Notes

- Default socket: `$XDG_RUNTIME_DIR/voice-cmd.sock` (fallback `/tmp/voice-cmd.sock`)
- Daemon logs when forked: `~/.local/state/voice-cmd/daemon.log`
- Overlay logs when daemonized: `~/.local/state/voice-cmd/overlay.log`

## Release Automation (mise)

```bash
mise run release-build
mise run release-checksums
mise run release-package
# or everything:
mise run release-all
```

Artifacts are written to `dist/`.

## License

Licensed under either of:

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option.
