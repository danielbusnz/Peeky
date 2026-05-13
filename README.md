# aegis

A voice-controlled AI cursor for Linux. Hold a hotkey, ask a question, and the cursor flies to whatever you asked about while a voice answer plays.

Built in Rust. Targets Hyprland today, cross-platform later.

## What it does

```
You hold INSERT and say "where is the close button"
   ↓
Cursor flies to the close button
   ↓
Voice says "It's in the top right corner of the window"
```

Sub-3-second release-to-speech latency on a typical desktop.

## How it works

```
microphone → Deepgram (STT, streaming)
                ↓
            transcript
                ↓
   Anthropic Claude Haiku 4.5 with Computer Use
       ↙                          ↘
  cursor coords                  spoken answer
       ↓                              ↓
   GTK4 sprite                Cartesia (TTS, streaming)
                                     ↓
                              rodio audio output
```

Three streaming providers + one local cpal mic capture, orchestrated by ~1700 lines of Rust.

## Requirements

- Linux with Hyprland (other Wayland compositors with `wlr-layer-shell` may work)
- `grim` for screenshots: `sudo pacman -S grim`
- A microphone
- API keys (free tiers available):
  - Anthropic: https://console.anthropic.com
  - Deepgram: https://console.deepgram.com
  - Cartesia: https://play.cartesia.ai

## Quickstart

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis

cat > .env <<EOF
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
CARTESIA_API_KEY=...
EOF

cargo build --release
```

Add hotkey bindings to your Hyprland config (`~/.config/hypr/hyprland.conf`):

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(aegis|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(aegis|test_)"
```

Then:
```bash
hyprctl reload
cargo run --release --bin aegis
```

Hold INSERT, ask something, release.

## Architecture

| Module | Job |
|---|---|
| `voice.rs` | The orchestrator. One turn = press → record → transcribe → Claude → speak |
| `audio.rs` | cpal mic capture with 300ms pre-roll buffer; cached rodio output sink |
| `cursor/hyprland.rs` | GTK4 layer-shell sprite that lerps toward a target |
| `providers/stt_deepgram.rs` | WebSocket STT with Strategy C (Finalize + grace period) |
| `providers/claude.rs` | Computer Use for cursor coords + vision for spoken answer |
| `providers/tts_cartesia.rs` | Streaming PCM via SSE |
| `hotkey/unix_signals.rs` | SIGUSR1/SIGUSR2 from Hyprland keybinds |
| `screenshot/grim.rs` | grim subprocess + JPEG resize for Computer Use |

The voice loop runs on its own thread; GTK owns the main thread; cpal owns the audio thread; tokio runs the async I/O.

## Test bins

```bash
cargo run --bin test_hotkey   # verify SIGUSR1/2 firing
cargo run --bin test_stt      # mic → Deepgram with full timing logs
cargo run --bin test_vision   # Claude describes your screen
cargo run --bin test_point    # cursor flies to a Claude-picked target
```

## Windows / macOS / X11 build

Cross-platform impls live behind the `winit-window` + `crossplatform` features.
Build the end-to-end smoke test (hotkey + mouse + screenshot + cursor) with:

```bash
cargo run --bin test_win --no-default-features --features winit-window,crossplatform
```

Hold `Insert`, release. Each turn logs mouse pos, saves a screenshot to the
temp dir, and flies the cursor sprite to the mouse position. Click-through is
on, so apps below the overlay still receive input.

## Status

Hyprland is the default. Cross-platform support (winit + xcap + mouse_position
+ global-hotkey) is in active development behind the `winit-window` and
`crossplatform` Cargo features.

## License

MIT
