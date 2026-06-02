<p align="center">
  <img alt="aegis cursor" src="console/ui/shared/cursor.svg" width="128">
</p>

<h1 align="center">Aegis</h1>
<p align="center">visit https://countdown.si9num.com for free trial</p>
<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-orange?logo=rust&logoColor=white">
  <img alt="License" src="https://img.shields.io/badge/License-MIT-blue">
  <img alt="Platform" src="https://img.shields.io/badge/Linux%20%7C%20Hyprland-black?logo=linux&logoColor=white">
  <img alt="Status" src="https://img.shields.io/badge/Status-WIP-yellow">
  <img alt="CI" src="https://github.com/danielbusnz-lgtm/Aegis/actions/workflows/ci.yml/badge.svg">
  <a href="https://codecov.io/gh/danielbusnz-lgtm/Aegis"><img alt="Coverage" src="https://codecov.io/gh/danielbusnz-lgtm/Aegis/branch/main/graph/badge.svg"></a>
</p>

<p align="center">
    hold the hotkey. Ask a Question. Aegis handles the rest.

<p align="center">
  <img alt="Aegis Demo" src="aegis/assets/demo.gif" width="800">
</p>

<p align="center">
Built in Rust. Primary target is Linux; macOS and Windows on the way.

## Get started with Claude Code

The fastest way to get Aegis running is with Claude Code. Open it in an empty directory and paste this:

```
Hi Claude.

Clone https://github.com/danielbusnz-lgtm/aegis.git into my current directory.

Then read AGENTS.md. I want to get Aegis running locally on Linux/Hyprland.

Help me set up everything: building it with cargo, wiring the push-to-talk
hotkey into my Hyprland config, and (optionally) pointing it at my own API
keys instead of the hosted proxy. Walk me through it.
```

That's it. It clones the repo, reads the docs, and walks you through the whole setup. 

## Manual setup

If you want to do it yourself.

**Prerequisites**

- Rust (stable) via [rustup](https://rustup.rs)
- Linux with Hyprland (or any X11 WM, see below), macOS, or Windows
- PipeWire for audio capture (`pw-record`)

**Build and run**

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis
cargo run --release -p aegis
```

All API calls route through a hosted Cloudflare Worker by default, so no keys are needed locally to try it.

**Hotkey (Hyprland)**

Add the push-to-talk bind to `~/.config/hypr/hyprland.conf`:

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(aegis|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(aegis|test_)"
```

Then `hyprctl reload`. Hold INSERT, ask something, release.

**Other platforms**

macOS and Windows build flag-free from the same `cargo run --release -p aegis`; the backend is picked by target OS. On Linux, build the winit/X11 path instead of Hyprland with `--no-default-features`.

**Prebuilt binaries**

Prefer not to build? Grab one from the [Releases page](https://github.com/danielbusnz-lgtm/Aegis/releases/latest). On macOS (Apple Silicon), download `aegis-macos-aarch64.dmg`, open it, and drag Aegis into Applications. If macOS blocks it as unverified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine /Applications/Aegis.app`. First launch walks you through onboarding (access code or your own API keys), then shows the push-to-talk hotkey. Linux and Windows releases ship the raw `aegis` binary.

## How it routes

Every voice turn picks one of five paths based on what you said. Each path has a focused Claude prompt and a tight tool set, so the model can't get distracted into the wrong category.

| Path | When it fires | What it does |
| --- | --- | --- |
| `find_action` | "where is X", "click X", "type X", "show me Y", "press Z" | One Claude call with a screenshot; cursor moves or input fires |
| `integration` | "play X", "pause", "check my email", "show my PRs" | Calls the service API directly (Spotify, Gmail, GitHub, YouTube); spoken summary |
| `chat` | "what's your name", "explain X", "how does Y work" | Pure Q&A with TTS streaming, no screen, no tools |
| `memory` | "remember my X is Y", "what's my Z" | Local JSONL store at `~/.config/aegis/memory.jsonl` |
| `agent` | Multi-step chains: "open YouTube, search for X, play the top result" | Full agent loop with iterative screenshots |

A hybrid classifier picks the path: sub-millisecond keyword match for clear cases (~80%), LLM fallback (~700ms) for ambiguous ones. Total release → action is typically ~1.2s for on device keys, 1.5s with proxy.

## Demos and benchmarks

Standalone binaries live in the `aegis-demos` crate, run with `cargo run -p aegis-demos --bin <name>`:

```bash
# record a sample first (24kHz mono 16-bit PCM)
pw-record --rate 24000 --channels 1 --format s16 aegis/fixtures/sample.wav

cargo run -p aegis-demos --bin bench_stt -- aegis/fixtures/sample.wav "hi my name is daniel" 5
cargo run -p aegis-demos --bin bench_find_action -- aegis/fixtures/find_action_sample.wav 5
cargo run -p aegis-demos --bin demo_stt
```

Each reports per-stage latency.

## Tunable behavior

`aegis/src/tuning.rs` holds every behavior dial in one place. Each constant has a `↑` / `↓` tradeoff comment so it's clear what changing the number does. Edit, recompile, see the effect.

Knobs include: pre-roll buffer length, STT quiescence window, TTS first-flush minimum, agent loop step cap and settle time, screenshot history depth.

## Use your own API keys

To bypass the proxy and call the providers directly, drop a `.env`:

```
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
CARTESIA_API_KEY=...
AEGIS_ANTHROPIC_DIRECT=1
AEGIS_DEEPGRAM_DIRECT=1
AEGIS_CARTESIA_DIRECT=1
```

Each `_DIRECT=1` opts that provider out of the proxy. Mix and match.

## Privacy

Aegis runs on your machine. Intent routing happens fully on-device. On-device logging for improving the router is off by default and opt-in (`AEGIS_ROUTELET_LOG=1`); when on, lines are redacted, capped, and stored only at `~/.config/aegis/`, never uploaded. Details in [PRIVACY.md](PRIVACY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines and [AGENTS.md](AGENTS.md) for project-specific rules (for both humans and agents).

## Acknowledgements

Inspired by [farzaa/clicky](https://github.com/farzaa/clicky), [earendil-works/pi](https://github.com/earendil-works/pi), and [mem0ai/mem0](https://github.com/mem0ai/mem0).

## License

[MIT](LICENSE)
