# Aegis voice loop optimization plan

**Date:** 2026-05-12
**Branch:** `streaming`
**Status:** Roadmap, not yet implemented
**Goal:** drive end-to-end release-to-audio latency from ~8s today to <1s with all optimizations

---

## Current latency breakdown

Working flow today (master, commit `9c52923`):

| Step | Time |
|---|---|
| Wait for transcript after release (Whisper batch) | 1-3s |
| Screenshot capture (grim + JPEG + base64) | 100-200ms |
| Claude full response (Opus 4.7 + vision, no stream) | 3-8s |
| Cartesia full WAV (waits for full claude response) | 500-1500ms |
| Rodio playback setup + first audio | ~50ms |
| **TOTAL release → first audio** | **~5-13s** |

Bottleneck is Claude. Every step blocks the next. No streaming anywhere.

---

## Optimization techniques (5 independent wins)

### 1. Pre-capture screenshot on press

Today screenshot runs *after* user releases. Move it to fire the moment SUPER+space is pressed, in parallel with the user's speech.

**Win:** 100-200ms

**Effort:** ~10 lines in `voice.rs`. Spawn a thread on press, join after recording stops.

**Already tracked:** Issue #8 (continuous pre-capture); this is the simpler "fire-on-press" variant.

### 2. Prompt caching of screenshots

Anthropic supports server-side prompt caching with `"cache_control": {"type": "ephemeral"}`. Mark the screenshot block as cached when sending. Image gets stored on Anthropic's side for 5 minutes.

Combined with pre-capture: send a "warmup" request the moment SUPER+space is pressed (using the screenshot we just took) with a dummy prompt. The real question (after release) hits the warm cache and returns ~200ms faster.

**Win:** ~200ms per request + significant cost savings on subsequent calls

**Effort:** small change to claude.rs body builder; small change to voice.rs to fire warmup request.

**Action:** create new GitHub issue for prompt caching.

### 3. Streaming STT (Whisper → Deepgram or AssemblyAI)

Today's flow waits for the entire WAV to upload + transcribe after release. With streaming STT, audio chunks pipe to the STT provider via WebSocket as the user is still speaking. Final transcript ready ~100-300ms after release.

**Win:** 1-2s

**Effort:** big — new provider (Deepgram or AssemblyAI), new WebSocket handling. Already tracked as part of Issue #3.

**Bonus:** enables VAD (#5 below).

### 4. Streaming LLM (Claude SSE)

Add `"stream": true` to Anthropic body. Parse SSE response. Pipe tokens out as they arrive instead of waiting for full response.

**Win:** 3-5s on its own. ONLY pays off when paired with streaming TTS (#4) — otherwise tokens still wait for synthesis.

**Effort:** medium. Adapt the Tabby pattern (`/home/dan/Projects/Tabby/src-tauri/src/claude.rs`) which already implements SSE parsing. Need to add `tokio` as a dep or use a blocking SSE parser.

**Already tracked:** Issue #2.

### 5. Streaming TTS (Cartesia /tts/sse or /tts/websocket)

Today Cartesia returns a full WAV blob. With streaming, audio chunks (raw PCM, base64-encoded) arrive in 50-100ms increments. Pipe directly to rodio's mixer as they arrive — audio starts playing within ~300ms of first text token.

**Win:** 500-1500ms

**Effort:** medium. New `synthesize_stream` method on `TtsCartesia`. Parse SSE chunks, base64-decode, append to rodio mixer as `SamplesBuffer`.

**Already tracked:** part of Issue #3.

### 6. Voice Activity Detection (VAD)

Detect 500ms of silence in the audio stream — fire the pipeline before the user formally releases the hotkey. The model starts thinking while the user is literally lifting their finger.

**Win:** 200-500ms

**Effort:** medium. Add a lightweight VAD (e.g., `webrtc-vad` crate) running on the live audio buffer. Trigger downstream pipeline on pause detection.

**Action:** create new GitHub issue for VAD.

---

## Fully-optimized pipeline (target architecture)

```
T = -500ms (during user's pause, before formal release)
        ↓
        VAD detects 500ms of silence
        ↓
T = -500ms: streaming STT transcript already complete
T = -500ms: screenshot already cached on Anthropic side (prompt cache hit)
        ↓
T = -500ms: POST sealed Claude request (streaming + cached image)
T = -200ms: First Claude token arrives
T = 0ms:    First complete sentence emitted ("It's at the top right")
        ↓
T = 0ms:    POST sentence to Cartesia streaming endpoint
T = +200ms: First Cartesia audio bytes arrive
T = +200ms: Rodio plays first audio
T = +200ms: USER HEARS RESPONSE
```

| | Latency from "user finished speaking" |
|---|---|
| Today | 5-13 seconds |
| Streaming everything | 800-1200ms |
| + VAD | **~300-700ms** |
| Theoretical hard floor | ~800ms (without VAD) |

## Theoretical hard floor

Even with infinite engineering, you can't beat:

```
Network RTT to Anthropic        ~50ms
Anthropic first-token latency   ~300-500ms  (the model's own startup)
Sentence accumulation           ~200ms      (waiting for natural break)
Network to Cartesia             ~50ms
Cartesia first byte             ~200ms      (their advertised number)
Decode + queue to rodio         ~5ms
                              ────────
Hard floor                    ~800ms
```

With VAD, you can pre-start the pipeline, effectively giving you "negative latency" relative to the user's perceived end-of-speech — but you can't beat physics.

---

## Implementation order

Order matters because some optimizations only pay off when paired:

1. **Pre-capture screenshot on press** (10 lines, ~200ms win, no dependencies) — do first
2. **Prompt caching** (small change, ~200ms win) — pairs with #1
3. **Streaming TTS (Cartesia SSE)** (medium, ~1s win) — enables real-time playback
4. **Streaming LLM (Claude SSE)** (medium, ~3s win when paired with #3) — pipeline tokens to #3
5. **Streaming STT** (big, ~1-2s win) — biggest re-architecture; new vendor
6. **VAD** (cherry on top, ~500ms win) — depends on streaming STT having live audio buffer

After #1-4: ~1s release-to-audio. Sub-second territory.
After #1-5: ~500-700ms. Top-tier voice agent.
After #1-6: ~300-500ms. State of the art.

---

## Mapping to GitHub issues

| Optimization | Issue |
|---|---|
| Pre-capture screenshot on press | New variant of #8 |
| Prompt caching | **Need new issue** |
| Streaming STT | #3 (part) |
| Streaming Claude SSE | #2 |
| Streaming TTS | #3 (part) |
| VAD | **Need new issue** |
| Reuse reqwest::Client | #9 |
| Haiku-vs-Opus routing | #1 |
| Tool use replacing regex | #7 (partially done) |

Two new issues to file: **prompt caching** and **VAD**.

---

## Comparison to competitors

| Product | Reported release-to-audio |
|---|---|
| Aegis today | 5-13s |
| Vapi | 700-1500ms |
| Cartesia Agents | 500-1000ms |
| ElevenLabs Conversational | 300-700ms |
| **Aegis target** | **~500-800ms** |

At the target, you're at parity with Vapi/Cartesia/Clicky. Industry-standard voice agent latency, achievable with the techniques above.

---

## Notes on what NOT to do

**Don't:** stream words individually to Claude as user speaks. Each new word would cancel + re-POST, ~20 API calls per turn, mostly wasted. Slower than batching.

**Don't:** use synchronous Whisper polling with fake "streaming" parsing — actual streaming STT requires a WebSocket-based provider (Deepgram, AssemblyAI, Cartesia Ink).

**Don't:** skip prompt caching — it's a free 200ms + meaningful cost savings once the screenshot is sent twice in a session.

**Don't:** over-engineer VAD — `webrtc-vad` or a simple RMS-based threshold is enough. Don't ship a tiny neural net for this.
