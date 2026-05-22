<p align="center">
  <img alt="aegis cursor" src="launcher/src/welcome/cursor.svg" width="128">
</p>

> New issues and PRs from new contributors are auto-closed by default. Maintainers review auto-closed issues daily. See [CONTRIBUTING.md](CONTRIBUTING.md).

---

<h1 align="center">Aegis</h1>

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

Built in Rust. Linux/Hyprland natively; Windows/macOS via the `winit-window` feature.

## Run it

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis
cargo run --release --bin aegis
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

## Windows / macOS / X11

```bash
cargo run --release --bin aegis --no-default-features --features winit-window,crossplatform
```

## Diagnostic tools

The `examples/` directory has standalone programs that exercise specific parts of the pipeline:

```bash
# Record a sample first (24kHz mono 16-bit PCM):
pw-record --rate 24000 --channels 1 --format s16 aegis/fixtures/sample.wav

# STT-only benchmark (mic → Deepgram, no Claude)
cargo run --release --example test_stt_bench -- aegis/fixtures/sample.wav "hi my name is daniel" 5

# Full pipeline benchmark (WAV → STT → classifier → find_action)
cargo run --release --example test_find_action_bench -- aegis/fixtures/find_action_sample.wav 5

# Live STT timing test
cargo run --release --example test_stt
```

Each example reports per-stage latency so you can see where the time goes.

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

## Acknowledgements

Inspired by [farzaa/clicky](https://github.com/farzaa/clicky) and [earendil-works/pi](https://github.com/earendil-works/pi).

## License

MIT
