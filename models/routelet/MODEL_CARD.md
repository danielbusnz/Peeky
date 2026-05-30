# Routelet Intent Classifier

On-device intent classifier for voice commands. Runs entirely locally with no network round-trip.

## Model Description

- **Architecture:** Sentence transformer encoder (384-dim) + logistic regression head
- **Classes:** 6 (agent, chat, find_action, integration, memory, none)
- **Head:** 6x384 coefficient matrix + 6 intercepts
- **Inference:** ~5-30ms on modern desktop CPU (see `bench_routelet.rs`)

## Files

| File | Size | Purpose |
|------|------|---------|
| `embedder.onnx` | ~133MB | ONNX sentence encoder |
| `head.json` | ~63KB | Logistic regression weights + labels |
| `tokenizer.json` | ~712KB | HuggingFace WordPiece tokenizer |

## Intended Use

Classify short voice transcripts into one of six intents for the Aegis voice assistant:

| Intent | Description |
|--------|-------------|
| `agent` | Multi-step tasks requiring iterative screenshots |
| `chat` | General Q&A, no screen interaction |
| `find_action` | Point/click/type/scroll on visible UI elements |
| `integration` | Service calls (Spotify, Gmail, GitHub, YouTube) |
| `memory` | Store or recall user facts |
| `none` | Ambiguous or out-of-scope; defer to LLM fallback |

## Training

- **Data:** Synthetic examples generated via Claude + paraphrases
- **Head training:** Logistic regression on frozen encoder embeddings
- **Calibration:** Temperature scaling (currently 1.0)

## Limitations

- English only
- Short utterances (<50 tokens); not tested on long passages
- May struggle with ambiguous phrasing (e.g., "play the button" could be find_action or integration)
- No accent or dialect variation in training data

## Privacy

Input text is preprocessed through `redact.rs` before logging:
- Emails replaced with `<EMAIL>`
- 4+ digit runs replaced with `<NUM>`
- Secret keywords (password, token, etc.) trigger tail masking with `<SECRET>`

## Usage

```rust
use aegis::routelet::Routelet;

let routelet = Routelet::load(Path::new("models/routelet"))?;
let (intent, confidence) = routelet.classify_with_confidence("play despacito")?;
```
