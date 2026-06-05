# Ideas

On-device ML models we could add to Peeky. The pattern routelet set: replace a
cloud call with a tiny on-device model, benchmarked against the LLM baseline.
Extending it across modalities (text, audio, vision) is the portfolio story.

## UI grounding (vision)

Take a screenshot plus a text query ("where's the pause button") and predict the
click point on-device, replacing the ~1.4s Claude vision call that find_action
turns spend today. Technique: fine-tuned tiny VLM, CLIP-style text-to-region
matching, or YOLO-style UI element detection (RICO, Screen2Words datasets).

## Wake-word spotting (audio)

Replace push-to-talk with "Hey Peeky" via a tiny keyword-spotting model on
mel-spectrograms. The textbook tinyML task and the cheapest new modality to add
(Speech Commands dataset plus our own samples).

## Memory retrieval embeddings

Fine-tune a small embedding model for semantic recall of stored facts ("what did
I tell you about my car"), replacing the flat JSONL lookup. Covers the
contrastive-embedding and RAG stack; see docs/memory-architecture.md.

## Integration tool-router

When the intent is integration, pick the service and tool on-device instead of
with a Claude call. The real work is argument extraction (slot-filling/NER), not
just service classification, so frame it as "tool + slots."

## ASR-correction

A small seq2seq model that fixes domain ASR errors ("open setti" into "open
settings") before classification. Bad transcripts cause misroutes; distill it
from a Claude teacher the same way routelet was trained.
