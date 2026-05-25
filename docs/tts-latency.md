# TTS latency: text-done → speech-start

The gap between Claude finishing its text and the first audio playing is the
Cartesia round trip for the first sentence. `SPEECH STARTS` fires on the first
PCM byte received (`orchestrator.rs`), so the delay is network + synthesis, not
client buffering.

`synthesize_stream` (`providers/tts_cartesia.rs`) does three serial steps:

1. **Mint auth token** (`bearer_token`). Proxy mode (default): a full HTTPS
   round-trip to the Cloudflare Worker, **once per synthesis call**. Direct mode
   (`AEGIS_CARTESIA_DIRECT`): free.
2. **POST** the text to `api.cartesia.ai/tts/sse`. Connection is pre-warmed at
   session start (`warm()`), so mostly upload + server queue.
3. **`sonic-2` synthesizes** and streams the first PCM chunk (~150-300ms typical,
   usually the largest piece).

Observed ~1.1s where best case is ~500ms.

## Levers

- **Per-turn token mint.** Proxy mode mints per sentence; the first sentence pays
  a serial round-trip before Cartesia is even contacted. Mint once per turn (or
  cache for the token's lifetime). Flagged in `tts_cartesia.rs` ("Per-call vs
  per-turn minting").
- **Warm the proxy host.** `warm()` only pre-opens `api.cartesia.ai`, not the
  Worker, so the first mint can pay TLS setup. Warm both at session start.
- **`sonic-2` TTFB** is Cartesia's; not much to do beyond model choice.

Check the startup line `[tts-cartesia] mode=Proxy` to know if the mint lever
applies.
