<p align="center">
  <img alt="peeky cursor" src="console/ui/shared/cursor.svg" width="128">
</p>

<h1 align="center">Peeky</h1>
<p align="center">Inspired by <a href="https://heyclicky.com">Clicky</a></p>
<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-orange?logo=rust&logoColor=white">
  <img alt="Platform" src="https://img.shields.io/badge/Linux%20%7C%20Hyprland-black?logo=linux&logoColor=white">
  <img alt="macOS" src="https://img.shields.io/badge/macOS-black?logo=apple&logoColor=white">
  <img alt="Status" src="https://img.shields.io/badge/Status-WIP-yellow">
  <img alt="CI" src="https://github.com/danielbusnz-lgtm/Peeky/actions/workflows/ci.yml/badge.svg">
</p>

<p align="center">Hold the hotkey. Ask a question. Peeky handles the rest.</p>

<p align="center">
  <img alt="Peeky Demo" src="peeky/assets/demo.gif" width="800">
</p>

## Why

[Clicky](https://heyclicky.com) proved the idea: a small AI that sits next to your cursor, sees your screen, and points at things. I wanted it on Linux. That meant rebuilding the bottom of the stack, since screen capture, input injection, push-to-talk, and the cursor overlay all work differently there.

Then I gave it hands. Clicky points. Peeky points, clicks, types, scrolls, and runs multi-step tasks. It also remembers facts about you between sessions, and it calls Gmail, Spotify, GitHub, and YouTube directly when the answer is not on the screen.

The part I care about most: Peeky's cursor is its own, separate from yours. You watch every move before it happens. Ask it to do the work, or ask it to show you where to click.

## Get started (macOS)

Go to **[getpeeky.ai](https://getpeeky.ai)**, download (enter your email for more free access), drag **Peeky** into Applications, and launch it. First launch walks you through the free trial (or your own keys) and your push-to-talk hotkey.

Blocked as unverified? Right-click the app and choose **Open**.

### Bring your own keys

On first launch, choose **use my own API keys** and paste your Anthropic, Deepgram, and Cartesia keys. They are stored in your OS keychain, every call goes straight to the provider, and nothing routes through the proxy or gets metered.

## Linux (Hyprland)

**Prerequisites**

- Rust (stable) via [rustup](https://rustup.rs)
- Hyprland (or any X11 WM, see below)
- PipeWire for audio capture (`pw-record`)

**Build and run**

```bash
git clone https://github.com/danielbusnz-lgtm/peeky.git
cd peeky
cargo run --release -p peeky
```

All API calls route through a hosted Cloudflare Worker by default, so no keys are needed locally to try it. Prefer not to build? Grab the raw `peeky` binary from the [Releases page](https://github.com/danielbusnz-lgtm/Peeky/releases/latest).

**Hotkey (Hyprland)**

Add the push-to-talk bind to `~/.config/hypr/hyprland.conf`:

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(peeky|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(peeky|test_)"
```

Then `hyprctl reload`. Hold INSERT, ask something, release. For X11 or another WM, build the winit path instead of Hyprland with `--no-default-features`.

**With Claude Code**

Open Claude Code in an empty directory and paste:

```
Hi Claude.

Clone https://github.com/danielbusnz-lgtm/peeky.git into my current directory.

Then read AGENTS.md. I want to get Peeky running locally on Linux/Hyprland.

Help me set up everything: building it with cargo, wiring the push-to-talk
hotkey into my Hyprland config, and (optionally) pointing it at my own API
keys instead of the hosted proxy. Walk me through it.
```

### Use your own keys (Linux)

Build from source and call the providers directly, no proxy, nothing metered. Drop a `.env` in the repo root:

```
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
CARTESIA_API_KEY=...
PEEKY_ANTHROPIC_DIRECT=1
PEEKY_DEEPGRAM_DIRECT=1
PEEKY_CARTESIA_DIRECT=1
```

Each `_DIRECT=1` opts that provider out of the proxy. Mix and match, e.g. your own Anthropic key but the proxy for the rest.

## How it routes

Every voice turn picks one of five paths based on what you said. Each path has a focused Claude prompt and a tight tool set.

| Path | When it fires | What it does |
| --- | --- | --- |
| `find_action` | "where is X", "click X", "type X", "show me Y", "press Z" | One Claude call with a screenshot; cursor moves or input fires |
| `integration` | "play X", "pause", "check my email", "show my PRs" | Calls the service API directly (Spotify, Gmail, GitHub, YouTube); spoken summary |
| `chat` | "what's your name", "explain X", "how does Y work" | Pure Q&A with TTS streaming, no screen, no tools |
| `memory` | "remember my X is Y", "what's my Z" | Local JSONL store at `~/.config/peeky/memory.jsonl` |
| `agent` | Multi-step chains: "open YouTube, search for X, play the top result" | Full agent loop with iterative screenshots |

A hybrid classifier picks the path: sub-millisecond keyword match for clear cases (~80%), LLM fallback (~700ms) for ambiguous ones. Total release to action is typically ~1.2s with on-device keys, ~1.5s with the proxy.

## Built with

| Layer | Tech |
| --- | --- |
| Core agent | Rust, Tokio (async) |
| On-device classifier | ONNX via tract, BERT embeddings, int8 quantization |
| Speech | Deepgram (STT) and Cartesia (TTS) over WebSocket streaming |
| LLM | Anthropic Claude, SSE streaming, forced tool use |
| Audio I/O | cpal capture, rodio playback |
| Desktop app | Tauri 2 |
| Backend proxy | Cloudflare Workers (TypeScript), KV, D1, R2 |
| Platforms | Hyprland/Wayland, macOS, Windows (in progress) |

## Demos and benchmarks

Standalone binaries live in the `peeky-demos` crate, run with `cargo run -p peeky-demos --bin <name>`:

```bash
# record a sample first (24kHz mono 16-bit PCM)
pw-record --rate 24000 --channels 1 --format s16 peeky/fixtures/sample.wav

cargo run -p peeky-demos --bin bench_stt -- peeky/fixtures/sample.wav "hi my name is daniel" 5
cargo run -p peeky-demos --bin bench_find_action -- peeky/fixtures/find_action_sample.wav 5
cargo run -p peeky-demos --bin demo_stt
```

Each reports per-stage latency.

## Tunable behavior

`peeky/src/tuning.rs` holds every behavior dial in one place. Each constant has a `↑` / `↓` tradeoff comment. Knobs include: pre-roll buffer length, STT quiescence window, TTS first-flush minimum, agent loop step cap and settle time, screenshot history depth.

## Privacy

Peeky runs on your machine. Intent routing happens fully on-device. On-device logging for improving the router is off by default and opt-in (`PEEKY_ROUTELET_LOG=1`); when on, lines are redacted, capped, and stored only at `~/.config/peeky/`, never uploaded. Details in [PRIVACY.md](PRIVACY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines and [AGENTS.md](AGENTS.md) for project-specific rules (for both humans and agents).

## Acknowledgements

Inspired by [farzaa/clicky](https://github.com/farzaa/clicky), [earendil-works/pi](https://github.com/earendil-works/pi), and [mem0ai/mem0](https://github.com/mem0ai/mem0).

## License

[MIT](LICENSE)
