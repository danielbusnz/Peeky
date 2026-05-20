# Intent Routing Architecture — Research and Tradeoffs

**Status:** research notes, not yet implemented
**Context:** the 5-path architecture (`find_action` / `chat` / `integration` / `memory` / `agent`) uses a pure LLM classifier that adds ~686ms to every voice turn. This doc maps the design space for reducing that latency, drawing from production voice/agent systems and academic work.

## The current bottleneck

Per the `test_find_action_bench` results on 2026-05-20:

```
stt tail        : ~1ms        (overlap with hold)
classify        : 651-733ms   (CRITICAL PATH)
find_action     : 1.0-1.3s    (the actual work)
─────────────────────────────────
total           : ~1.8s release → action
```

The 686ms classify cost comes from a single Haiku call returning a one-token category. Anthropic's prompt cache minimum is 4,096 tokens for Haiku 4.5; our classifier system prompt is ~360 tokens, so the `cache_control` marker we set is silently ignored. Every classifier call pays full uncached preprocessing.

**Why this matters:** the classifier exists *only* because we split one agent loop into five focused paths. The old monolithic `run_agent_loop` had no classifier — but it had the consistency bug where "where is X" sometimes returned text and sometimes a Point action. We pay 686ms to make routing predictable.

---

## Approaches found in research

Six patterns from production systems and recent papers. Each has documented latency / accuracy / cost numbers where available.

### 1. Pure LLM classifier (current)

A dedicated LLM call returns a category enum via forced tool use. What aegis does today.

| Trait | Value |
|---|---|
| Latency | 250-700ms per turn (depends on cache hit) |
| Accuracy | High on in-distribution queries (~95%+) |
| Cost | One extra Claude call per turn |
| Implementation | ~80 lines |

**Pros:** smartest classifier, handles novel phrasings, no taxonomy maintenance.
**Cons:** always pays the latency tax, even for trivially classifiable queries.

**Real-world data point:** TianPan.co reports LLM-based classification at ~1-5 seconds for cold and ~250-700ms for cached. Used as the "catch-all at the bottom of a classification cascade" — not as the primary router in latency-sensitive systems.

**Best for:** prototypes, low-volume systems, novel intent spaces still being explored.

### 2. Hybrid keyword + LLM fallback

Pattern: a rule-based matcher runs first (sub-millisecond). If the rules don't produce a confident match, fall through to the LLM.

```rust
fn classify(transcript: &str) -> Intent {
    if let Some(intent) = keyword_classify(transcript) {
        return intent;  // ~1ms
    }
    llm_classify(transcript)  // ~700ms, only for ambiguous
}
```

| Trait | Value |
|---|---|
| Latency (keyword hit) | ~1-5ms |
| Latency (LLM fallback) | same as pure LLM (~700ms) |
| Mean latency | depends on hit rate; for ~80% hit rate, ~145ms average |
| Cost | LLM only on ambiguous queries |
| Implementation | ~50-100 lines |

**Pros:** captures the high-frequency, unambiguous cases instantly. Predictable. Cheap.
**Cons:** keyword patterns can drift, edge phrasings get misclassified, taxonomy maintenance.

**Real-world data point:** Ashish Kumar's "Intent Routing in Production Voice AI" (Medium, Jan 2026) recommends this as the *default* pattern: "Most production intent routing is not AI-first. Rule-based matching first; LLM fallback only if rules fail."

**Best for:** systems with a stable, small intent taxonomy where 70%+ of queries fit a clear pattern. Aegis qualifies.

### 3. Embedding-based routing (semantic-router pattern)

Pre-compute embeddings for example utterances per intent. Embed the user's transcript, find the nearest intent by cosine similarity. Pure local computation after the first run.

```
intent examples (pre-embedded):
  find_action: ["where is X", "click X", "show me Y", ...]
  integration: ["play X", "pause", "check email", ...]
  chat:        ["what's your name", "how does X work", ...]

at query time:
  user_emb = embed(transcript)
  best = argmax_over_intents(cosine(user_emb, intent_centroid))
```

| Trait | Value |
|---|---|
| Latency | 16-100ms (local embed + nearest-neighbor) |
| Accuracy | 92-96% after iterative example tuning |
| Cost | sub-penny per query vs ~$0.65/10k for LLM (~65x cheaper) |
| Implementation | ~200 lines + an embedding model |

**Pros:** no API call at all for routing. Genuinely fast (16-100ms). Cheap.
**Cons:** requires running an embedding model locally OR calling a hosted embedding API (still cheaper than LLM). Struggles with out-of-distribution queries. Needs example curation per intent.

**Real-world data point:** TianPan.co cites a production deployment that "reduced end-to-end routing latency from 5,000ms to 100ms while achieving 92-96% precision after iterative example refinement." Mosheh Haim Makias' Medium piece: "50x faster, 100x cheaper, and accurate enough for production." Aurelio Labs maintains an open-source [`semantic-router`](https://github.com/aurelio-labs/semantic-router) library implementing this exact pattern.

**Best for:** medium-traffic systems with a relatively stable intent set. Higher implementation cost than hybrid keyword, but better accuracy on phrasing variants.

### 4. Fine-tuned small models (SetFit, DistilBERT, ModernBERT)

Train a small classifier (e.g., a 110M parameter encoder) on labeled intent examples. Run inference locally.

| Trait | Value |
|---|---|
| Latency | 50-200ms (local inference) |
| Accuracy | Within 8-10% of frontier LLM F1 |
| Cost | ~free at inference; training cost is one-time |
| Implementation | training pipeline + model serving (~500+ lines) |

**Pros:** highest accuracy outside of LLM. SetFit can train on just 8 examples per intent. IBM Research's vLLM semantic router uses ModernBERT and achieves +10.24 percentage points on MMLU-Pro with 47.1% latency reduction vs LLM routing.
**Cons:** training infrastructure, model deployment, ongoing retraining as intents drift. Heavy for an indie/solo project.

**Real-world data point:** Google Research's two-stage approach (small model summarizes, then small fine-tuned model extracts intent) matches Gemini Pro accuracy while enabling on-device classification.

**Best for:** production systems with serious volume (1000+ requests/day) and team resources to maintain a training pipeline.

### 5. Cascade pattern

Combine all of the above. Stack increasingly expensive classifiers until one returns a high-confidence answer.

```
1. Keyword filter      (sub-ms)     ─► handles 60-70% (clear cases)
2. Embedding router    (16-100ms)   ─► handles 20-25% (common variants)
3. Fine-tuned model    (50-200ms)   ─► handles 5-10% (nuanced)
4. LLM catch-all       (700-5000ms) ─► handles <5% (novel, compositional)
```

Confidence thresholds gate movement up the cascade. >0.8 confidence routes automatically. 0.5-0.8 routes but flags for review. <0.5 escalates.

| Trait | Value |
|---|---|
| Mean latency | 50-200ms (dominated by the fast paths) |
| P99 latency | LLM catch-all latency (~5s) |
| Accuracy | Within 2% of pure LLM accuracy on in-distribution data |
| Implementation | ~500-1000 lines, multiple subsystems |

**Pros:** best accuracy/latency tradeoff at production scale. Each tier handles what it's best at.
**Cons:** by far the most code. Requires confidence calibration. Needs a feedback loop to keep the cascade tuned.

**Real-world data point:** Microsoft Research's GeckOpt pattern, deployed on "100+ GPT-4-Turbo nodes in a Copilot system," achieves 24.6% token reduction with <1% accuracy loss using this cascade.

**Best for:** mature production systems where every millisecond and dollar matters.

### 6. Combined intent + first response (NVIDIA AI-Q pattern)

A single LLM call does *both* classification AND generates the response for the common ("meta") case. Skips an entire round-trip when the answer is short.

```
User: "Hello! What can you do?"
  ↓
LLM (one call):
  - classify intent → "meta"
  - generate response → "Hi! I'm an assistant..."
  - return JSON { intent, meta_response }
  ↓
(if intent == "meta") → speak meta_response directly. Done.
(if intent == "research") → fan out to research path with depth flag.
```

| Trait | Value |
|---|---|
| Latency (meta query) | one LLM call (~700ms) — no second call needed |
| Latency (research query) | one LLM call to classify + one to research |
| Total | similar single-call cost regardless of intent |
| Implementation | ~200 lines |

**Pros:** eliminates the round-trip for chat/conversation queries. The LLM call you'd make anyway *is* the classifier.
**Cons:** the classifier prompt grows to include response-generation instructions. Less clean separation of concerns.

**Real-world data point:** NVIDIA's [AI-Q Blueprint](https://docs.nvidia.com/aiq-blueprint/2.0.0/architecture/agents/intent-classifier.html) production system uses this. Documented as "minimizes latency for the common case (meta queries get an instant response) and avoids an extra round-trip."

**Best for:** systems where the chat-style intent is the most common and the classifier was going to make an LLM call anyway. Could apply to aegis's Chat path.

### 7. Speculative classification on STT interims

Fire the classifier on partial STT results during the user's hold, not after release. By the time the final transcript is ready, the classifier has either already returned or is moments away.

```
hold begins ──► STT emits interim ──► classifier fires on interim
                STT emits interim ──► classifier fires (cancel previous)
                STT emits interim ──► classifier fires (cancel previous)
release ────► STT final ──► classifier likely done already → branch immediately
```

| Trait | Value |
|---|---|
| User-visible latency | ~0ms (classifier overlaps with hold) |
| Wasted API calls | 1-3 per turn (interims that get superseded) |
| Cost | ~3x classifier cost per turn |
| Implementation | ~200-300 lines, cancellation logic |

**Pros:** classifier latency becomes invisible.
**Cons:** wasted API spend; classifier might commit to wrong intent if user changes course mid-utterance.

**No production system documented this exact pattern in our research**, but it's a natural extension of speculative LLM inference (which Anthropic, OpenAI, and Google all do internally for TTFT reduction).

**Best for:** latency-critical voice systems with deep pockets.

### 8. State-aware intent routing (FSM-based)

Different intents are allowed in different conversation states. Classification only chooses between the small set valid for the current state.

```
At state "ASK_PURPOSE":     allowed = [sales, support, unknown]
At state "CONFIRM_DETAILS": allowed = [yes, no, repeat, unknown]
At state "CLOSE":           allowed = [okay, restart, unknown]
```

The same utterance ("yes") routes differently depending on what state the system is in. The classifier picks from a list of ~3-5 intents instead of N global intents.

| Trait | Value |
|---|---|
| Latency | depends on classifier choice (any of the above) |
| Accuracy | dramatically higher per-state (smaller decision space) |
| Implementation | FSM definition + per-state intent allowlists |

**Pros:** smaller decision space per turn means simpler/faster classifiers work well. Eliminates many "ambiguous" cases.
**Cons:** requires an explicit FSM model of the conversation. Heavyweight for single-turn voice commands.

**Real-world data point:** Kumar's series on production voice AI argues this is the "real brain" of voice systems and a defining pattern of mature production deployments.

**Best for:** multi-turn conversations with structured flows (IVR, customer support bots). Less applicable to aegis's single-turn command model.

---

## What fits aegis

Aegis has specific constraints that filter the options:

- **Solo indie project**: heavy training pipelines (option 4) and full cascades (option 5) are too much code to maintain
- **Single-turn voice commands**: FSM-based state routing (option 8) is overkill
- **Small intent set (5)**: most queries fall into clear patterns
- **Latency-critical**: every 100ms shows up in release→speech
- **Already paying for Anthropic API**: LLM calls aren't a new cost center

This points at two practical winners:

### Recommended: Hybrid keyword + LLM fallback (Option 2)

Pattern matching covers the ~80% of queries with clear phrasings ("play X", "where is X", "click X", "remember X is Y"). LLM only runs for the ambiguous ~20%.

**Expected impact:**
- Mean classifier latency: 686ms → ~150ms (LLM fires on 20% of turns at 700ms each, plus 1ms keyword check on the other 80%)
- Total release→action: ~1.8s → ~1.3s (similar to the old pre-classifier architecture)
- Accuracy: ~95% (loses 1-2% on edge phrasings the keywords miss)
- Implementation: ~50-100 lines

**Why this first:** dollars-to-latency ratio. We get most of the win for the least code.

### Future: Embedding-based router (Option 3)

If the keyword classifier drifts or hit rate drops below ~60%, swap it for a local embedding-based classifier. Better accuracy on phrasing variants, similar latency (~50ms), no API call at all.

**Stack on top of hybrid:** keyword → embedding → LLM cascade. Three tiers. We'd add the middle tier only when the data shows we need it.

### Worth considering: Combined intent + first response (Option 6) for Chat

For Chat intent specifically, the classifier and the response are *both* Claude calls. The NVIDIA pattern merges them: the same call that classifies also generates the chat response. Saves ~700ms when the intent ends up being Chat (most common case).

**Implementation cost:** medium. Requires restructuring `chat.rs` to also handle the classification output format.

### Decline for now

- **Fine-tuned model**: not worth the training/serving overhead at current scale
- **Speculative on interims**: hides latency but adds complexity and wasted API spend
- **Full cascade**: premature optimization

---

## Recommended sequence

1. **Implement Option 2 (hybrid keyword + LLM fallback)** — quick, high impact, easy to revert
2. **Measure hit rate over a week of real use** — log which queries fall through to LLM
3. **Decide based on data:**
   - If keyword hit rate >80% and accuracy is fine → ship as-is
   - If hit rate <60% → upgrade to embedding-based (Option 3)
   - If Chat is the most common intent → consider combined intent+response (Option 6)

The point isn't to pick the perfect architecture upfront. It's to pick a good-enough one fast, measure, and iterate.

---

## References

Cited or drawn from:

- Ashish Kumar, ["Intent Routing in Production Voice AI"](https://medium.com/@ashishkumar_81395/intent-routing-in-production-voice-ai-2dc9702dae48) (Medium, 2026-01)
- TianPan, ["The Intent Classification Layer Most Agent Routers Skip"](https://tianpan.co/blog/2026-04-16-intent-classification-agent-routers) (2026-04)
- NVIDIA, ["AI-Q Blueprint Intent Classifier"](https://docs.nvidia.com/aiq-blueprint/2.0.0/architecture/agents/intent-classifier.html) (2026)
- Moshe Haim Makias, ["Building a Real-Time Intent Router: Why You Don't Need a Large LLM"](https://moshe-haim-makias.medium.com/building-a-real-time-intent-router-why-you-dont-need-a-large-llm-44ff0eda24b6) (Medium)
- Aurelio Labs, [`semantic-router`](https://github.com/aurelio-labs/semantic-router) library
- IBM Research, [vLLM semantic-router blog](https://blog.vllm.ai/2025/09/11/semantic-router.html)
- Google Research, [Small Models for Intent Extraction](https://research.google/blog/small-models-big-results-achieving-superior-intent-extraction-through-decomposition/)
- Microsoft GeckOpt, [arxiv:2404.15804](https://arxiv.org/abs/2404.15804)
- Chroma Research, [Context Rot](https://research.trychroma.com/context-rot)
- Anthropic, [Prompt caching docs](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching) (for the 4096-token cache minimum)
