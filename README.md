<p align="center">
  <img alt="aegis cursor" src="launcher/src/welcome/cursor.svg" width="128">
</p>

<h1 align="center">Aegis</h1>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-orange?logo=rust&logoColor=white">
  <img alt="License" src="https://img.shields.io/badge/License-MIT-blue">
  <img alt="Platform" src="https://img.shields.io/badge/Linux%20%7C%20Hyprland-black?logo=linux&logoColor=white">
  <img alt="Status" src="https://img.shields.io/badge/Status-WIP-yellow">
  <img alt="CI" src="https://github.com/danielbusnz-lgtm/Aegis/actions/workflows/ci.yml/badge.svg">
  <a href="https://codecov.io/gh/danielbusnz-lgtm/Aegis"><img alt="Coverage" src="https://codecov.io/gh/danielbusnz-lgtm/Aegis/branch/main/graph/badge.svg"></a>
</p>

<p align="center">
  Voice-controlled AI cursor for Linux. Hold a hotkey, say something, the cursor flies to whatever you mentioned or the right action fires.
</p>

```
"where is the search bar"      → cursor flies to it
"click the play button"        → cursor moves + real click fires
"play despacito on spotify"    → spotify API call, music starts
"check my email"               → gmail unread count, spoken aloud
"remember my name is Daniel"   → stored locally, recalled later
"what's your name"             → spoken reply, no screen used
```

Built in Rust. Primary target is Linux/Hyprland; macOS and Windows build flag-free from the same `cargo run`.

## Download

Prebuilt binaries are on the [Releases page](https://github.com/danielbusnz-lgtm/Aegis/releases/latest).

**macOS (Apple Silicon):** download `aegis-macos-aarch64.dmg`, open it, and drag Aegis into Applications. If macOS blocks it as unverified, right-click the app and choose Open, or run:

```bash
xattr -dr com.apple.quarantine /Applications/Aegis.app
```

First launch walks you through onboarding: drop in an access code or your own API keys, then it shows the push-to-talk hotkey (hold Ctrl+Space). With a code you need no keys of your own.

**Linux / Windows:** the release ships the raw `aegis` binary, or build from source below.

## Run it

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis
cargo run --release -p aegis
```

All API calls route through a hosted Cloudflare Worker by default, so no keys needed locally to try it.

On Hyprland, add the hotkey to `~/.config/hypr/hyprland.conf`:

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(aegis|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(aegis|test_)"
```

`hyprctl reload`. Hold INSERT, ask something, release.

## How it routes

Every voice turn picks one of five paths based on what you said. Each path has a focused Claude prompt and a tight tool set, so the model can't get distracted into the wrong category.

| Path | When it fires | What it does |
| --- | --- | --- |
| `find_action` | "where is X", "click X", "type X", "show me Y", "press Z" | One Claude call with a screenshot; cursor moves or input fires |
| `integration` | "play X", "pause", "check my email", "show my PRs" | Calls the service API directly (Spotify, Gmail, GitHub, YouTube); spoken summary |
| `chat` | "what's your name", "explain X", "how does Y work" | Pure Q&A with TTS streaming, no screen, no tools |
| `memory` | "remember my X is Y", "what's my Z" | Local JSONL store at `~/.config/aegis/memory.jsonl` |
| `agent` | Multi-step chains: "open YouTube, search for X, play the top result" | Full agent loop with iterative screenshots |

A hybrid classifier picks the path: sub-millisecond keyword match for clear cases (~80%), LLM fallback (~700ms) for ambiguous ones. Total release → action is typically ~1.2s.

## macOS / Windows

Same command, no flags. The backend is picked by target OS:

```bash
cargo run --release -p aegis
```

On Linux, build the winit/X11 path instead of Hyprland with `--no-default-features`.

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines and [AGENTS.md](AGENTS.md) for project-specific rules (for both humans and agents).

## Acknowledgements

Inspired by [farzaa/clicky](https://github.com/farzaa/clicky), [earendil-works/pi](https://github.com/earendil-works/pi), and [mem0ai/mem0](https://github.com/mem0ai/mem0).

## License

[MIT](LICENSE)
