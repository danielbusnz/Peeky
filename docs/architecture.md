# Architecture

One voice turn end to end. The hot path is roughly 1.2s from hotkey
release to action firing.

## Pipeline

```mermaid
flowchart LR
    HK[Hotkey hold/release] --> MIC[Mic capture<br/>cpal + ring buffer]
    MIC --> STT[Deepgram STT<br/>streaming WS]
    HK -. parallel .-> SHOT[Screenshot<br/>active workspace]

    STT --> CLS{Intent classifier<br/>keyword then Claude}

    CLS -->|memory| MEM[Local JSONL<br/>~/.config/aegis/memory.jsonl]
    CLS -->|find_action| FA[Claude + screenshot<br/>one shot]
    CLS -->|integration| INT[Service APIs<br/>Spotify Gmail GitHub YouTube]
    CLS -->|chat| CHAT[Claude chat<br/>no tools]
    CLS -->|agent| AG[Agent loop<br/>iterative screenshots]

    SHOT -.-> FA
    SHOT -.-> AG

    AG --> FA
    AG --> INT

    FA --> ACT[Cursor move<br/>input fires]
    MEM --> TTS[Cartesia TTS<br/>streaming]
    INT --> TTS
    CHAT --> TTS
    ACT -.-> TTS

    TTS --> SPK[Speaker<br/>audio out]
```

Solid arrows are the data path. Dotted arrows are state that flows
sideways (the pre-captured screenshot, the optional spoken confirmation
of a cursor action).

## The five intent paths

| Path | When it fires | Output |
| --- | --- | --- |
| `memory` | "remember my X is Y", "what's my Z" | Local store write/read |
| `find_action` | "where is X", "click X", "type X" | Cursor or input event |
| `integration` | "play X", "check my email", "show my PRs" | Service call + spoken summary |
| `chat` | "what's your name", "how does X work" | Spoken reply |
| `agent` | Multi-step chains | Iterative tool use |

The keyword classifier in `aegis/src/intent.rs` covers around 80% of
turns in roughly a millisecond. The remaining ambiguous turns fall
through to a small Claude classifier call (around 700ms).

## Why this shape

- **Screenshot in parallel with STT.** Roughly 60% of turns never need
  it (chat, integration, memory). Capturing eagerly trades the small
  cost of an unused PNG for a much faster `find_action` path when one
  does come through.
- **Keyword classifier before LLM classifier.** Most utterances are
  unambiguous. We don't pay 700ms of LLM latency to find that out.
- **Per-turn TTS channel.** Sentences stream into Cartesia as soon as
  the model produces them. First-flush minimums live in `tuning.rs`.
- **Barge-in cancellation.** Pressing the hotkey during TTS playback
  cancels the in-flight turn and starts a new one. See `barge_in.rs`.

## Where each piece lives

| Module | Responsibility |
| --- | --- |
| `audio/` | Mic capture, ring buffer, RMS level for the overlay |
| `providers/stt_deepgram` | Streaming STT over WS |
| `providers/claude` | Classifier, find_action, chat, agent calls |
| `providers/tts_cartesia` | Streaming TTS |
| `intent.rs` | Keyword classifier |
| `orchestrator.rs` | The per-turn state machine drawn above |
| `voice_session.rs` | Tokio runtime + shared per-process state |
| `screenshot/` | Active workspace capture, resize for Claude |
| `actions.rs` | Cursor move, click, key input |
| `barge_in.rs` | Cancel an in-flight turn when the hotkey re-fires |
| `tuning.rs` | Every behavioral knob, with up/down tradeoff comments |
| `painter.rs` | Overlay rendering (Hyprland layer-shell or winit) |

## Memory layer

The `memory` path writes facts as JSONL lines and looks them up by
keyword. The deeper design (vector index, embeddings cache, future
SQLite migration) lives in [memory-architecture.md](./memory-architecture.md).
