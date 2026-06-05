# Memory Architecture

Long-term memory design. Tier 1 ships today; tiers 2 and 3 are designed, not built.

## Goals

1. Sub-millisecond reads on the hot path (1.2s release-to-action budget).
2. Remember everything said, not just explicit "remember my X" facts.
3. Local only. No remote vector DBs, no hosted memory services.
4. Earn complexity. Flat files first; vector indices only if benchmarks justify.

## Non-goals

- Multi-user / per-profile memory.
- Cross-device sync.
- Queryable graph interface (Mem0 dropped theirs; entity linking as a retrieval signal is enough).

## Tier 1: core facts (shipped)

`peeky/src/providers/claude/memory.rs`.

- `MemoryStore` = `Arc<Mutex<Vec<(String, String)>>>` plus a `PathBuf`.
- Append-only JSONL at `~/.config/peeky/memory.jsonl`. Latest-wins per key on load.
- Two ops: `store_fact(key, value)` and in-memory lookup at recall.
- Routed via Haiku 4.5 forced tool call.
- Injected into other paths via `as_prompt_block()`.

Covers "remember my home city is Boston." Does not cover "what did we talk about Tuesday."

## Tier 2: conversation log (designed)

SQLite + FTS5. One row per turn.

```sql
CREATE TABLE turns (
    id      INTEGER PRIMARY KEY,
    ts      INTEGER NOT NULL,           -- unix epoch
    user    TEXT NOT NULL,              -- transcript
    claude  TEXT NOT NULL,              -- spoken reply
    intent  TEXT
);

CREATE VIRTUAL TABLE turns_fts USING fts5(
    user, claude,
    content=turns,
    tokenize='porter unicode61'
);
```

- Path: `~/.config/peeky/history.db`.
- `rusqlite` with the `bundled` feature.
- Writes async off the hot path.
- Indexed search: `<10ms` on 1M rows.
- Exposed as `search_history(query)` tool when intent is `Memory` or temporal-phrased `Chat`.

## Tier 3: entity-linked retrieval (deferred)

Only if tier 2 keyword search proves insufficient. Mem0 reports +29.6 pts on temporal, +23.1 on multi-hop with entity matching fused into semantic + BM25.

- Embeddings: `fastembed-rs` with BGE-small or MiniLM (384-dim, CPU, ~10ms/turn).
- Storage: `embedding BLOB` column on `turns`. No separate vector DB.
- Search: brute-force cosine in Rust. ~30ms across 100k vectors.
- Score fusion: semantic + BM25 + entity match.
- Switch to ANN (sqlite-vec, lancedb) only past ~500k entries.

## Sleep-time consolidation

Letta's pattern: expensive work happens when the user isn't listening.

Trigger:
- `hotkey::is_recording()` false.
- No interaction for N seconds (start at 5 min).
- Daily 03:00 local window.

Job:
1. Read tier-2 rows from the last 24h not yet summarized.
2. One Haiku call: "summarize today into durable facts. Return tool calls to `store_fact`."
3. Write to tier 1, mark tier-2 rows `summarized_at`.

Optional second pass: collapse old tier-2 rows into daily summaries.

## System prompt for tier 2/3

Include Anthropic's interruption protocol verbatim:

```
IMPORTANT: Your context window may reset at any moment. Treat the memory
directory as your only durable scratchpad. Use search_history before
answering questions about past conversations.
```

## Latency budget

| Stage | Budget | Notes |
|---|---|---|
| Tier 1 prompt block | <1ms | In-memory scan |
| Tier 2 keyword search | <10ms | FTS5 indexed |
| Tier 3 cosine scan | ~30ms | 100k rows |
| Sleep-time consolidation | seconds-minutes | Off hot path |

Total stays well under 1.2s even with all three queried.

## Risks

- **Staleness in tier 1.** No decay. Mirror Mem0: on overwrite, log prior value to a `fact_history` table.
- **Mutex contention.** Swap to `tokio::sync::RwLock` when tier 2 lands.
- **Async write loss.** Fire-and-forget tier-2 writes lose in-flight turns on crash. Acceptable for voice loop; batch-flush with fsync if not.
- **PII in the log.** Everything spoken persists. Add incognito mode that bypasses tier-2 writes.

## Benchmarks

When tier 2/3 ships, measure against LoCoMo if cost is reasonable. Otherwise hand-curated:

- Single-hop recall ("what's my favorite color")
- Temporal ("what did I ask yesterday")
- Multi-hop ("when did I first mention Tokyo")
- Tokens per recall turn
- Wall-clock from intent → first TTS token

Mem0's 92.5 LoCoMo, ~7K tokens/query is the ceiling, not the contract.

## Sources

- https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool
- https://www.letta.com/blog/agent-memory
- https://mem0.ai/blog/state-of-ai-agent-memory-2026
- https://arxiv.org/abs/2504.19413
- https://agentman.ai/blog/reverse-ngineering-latest-ChatGPT-memory-feature-and-building-your-own
