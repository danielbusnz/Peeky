# Memory Architecture

Design doc for aegis's long-term memory. Captures the current state, the target three-tier design, and the research that informed it.

Status: design, not implemented past tier 1.

## Goals

1. Survive a sub-second hot path. Voice loop budget is ~1.2s release-to-action. Any memory read on the critical path needs to land in single-digit milliseconds.
2. Remember everything we've ever said, not just explicit "remember my X" facts.
3. Stay local. No remote vector DBs, no calls to a hosted memory service. Privacy is part of the product.
4. Stay simple until complexity is earned. File-based tiers first, vector indices only if benchmarks justify them.

## Non-goals

- Multi-user / per-profile memory. aegis is single-user, one machine.
- Cross-device sync.
- A queryable graph interface. Mem0 dropped theirs in 2026; entity linking as a retrieval signal is enough.

## Current state (tier 1 only)

Implemented in `aegis/src/providers/claude/memory.rs`.

- `MemoryStore` = `Arc<Mutex<MemoryInner>>` where `MemoryInner` is `Vec<(String, String)>` + `PathBuf`.
- Storage: append-only JSONL at `~/.config/aegis/memory.jsonl`. One line per write, latest-wins per key at load time.
- Two operations: `store_fact(key, value)` and an in-memory lookup during recall.
- Routing: Haiku 4.5 with forced tool call, `store_fact` or `recall_fact`.
- Exposed to other paths via `as_prompt_block()` which renders the facts as a multi-line string for system prompt injection.

This covers "remember my home city is Boston" type interactions. It does not cover "what did we talk about last Tuesday."

## Target three-tier design

Modeled after the convergent design across Anthropic's memory tool, Letta/MemGPT, OpenAI's ChatGPT memory, and Mem0.

### Tier 1: core facts (existing)

No changes.

- Storage: JSONL.
- Access: O(n) scan, but n is small (tens of entries).
- Injected into every system prompt via `as_prompt_block()`.
- Equivalent to Letta's "memory blocks" or ChatGPT's "user profile."

### Tier 2: conversation log

Every turn appends one row to a SQLite database with an FTS5 virtual table.

```sql
CREATE TABLE turns (
    id      INTEGER PRIMARY KEY,
    ts      INTEGER NOT NULL,            -- unix epoch
    user    TEXT NOT NULL,                -- transcript
    claude  TEXT NOT NULL,                -- spoken reply
    intent  TEXT                          -- find_action / chat / memory / agent / integration
);

CREATE VIRTUAL TABLE turns_fts USING fts5(
    user, claude,
    content=turns,
    tokenize='porter unicode61'
);
```

- Path: `~/.config/aegis/history.db`.
- Crate: `rusqlite` with the `bundled` feature so users don't need a system SQLite.
- Hot-path writes are async on a separate task. Recording the turn must not block TTS.
- Indexed read: `SELECT ... WHERE turns_fts MATCH ? ORDER BY rank LIMIT 5` runs in <10ms on 1M rows.
- Exposed to Claude as a `search_history(query)` tool, only when the router classifies intent as `Memory` or `Chat` with a temporal phrasing.

### Tier 3 (deferred): entity-linked retrieval

Only add when tier 2 keyword search proves insufficient on real usage. Mem0's data shows +29.6 points on temporal queries and +23.1 on multi-hop when entity matching is fused with semantic + BM25.

Local-first plan:

- Embedding: `fastembed-rs` with BGE-small or MiniLM (384-dim, runs on CPU, ~10ms/turn).
- Storage: extra `embedding BLOB` column on the `turns` table. No separate vector DB.
- Search: brute-force cosine in Rust. ~30ms across 100k 384-dim vectors.
- Score fusion: combine semantic similarity, FTS5 BM25 rank, and entity match into a single score. Mirror Mem0's multi-signal retrieval.
- Switch to an ANN index (sqlite-vec, lancedb) only above ~500k entries. We will not get there in any reasonable timeframe for a single user.

## Sleep-time consolidation

Steals Letta's sleep-time compute idea. The expensive work happens when the user isn't listening.

Trigger:

- `hotkey::is_recording()` is false.
- No user interaction for N seconds (start at 5 minutes).
- A scheduled daily window (e.g. 03:00 local) for the deeper pass.

Job:

1. Read tier 2 rows from the last 24 hours that haven't been summarized.
2. Single Haiku call with prompt: "summarize today's conversations into structured facts. Return tool calls to `store_fact` for any durable preference, identity, or recurring task. Ignore one-off questions."
3. Write extracted facts to tier 1 via the existing `store_fact` path. Mark consolidated tier 2 rows with a `summarized_at` timestamp so we don't re-process.

Optional second pass: collapse old tier 2 rows into a daily summary row, drop the originals if they're not still hot. Mirrors Letta's recursive summarization and the compaction pattern in Claude's API.

Net effect: durable facts accumulate in tier 1 without the user having to say "remember X." The hot path never pays for this work.

## System prompt hygiene

When tier 2 or tier 3 retrieval tools are exposed, include Anthropic's interruption protocol verbatim (or near-verbatim) in the system prompt:

```
IMPORTANT: Your context window may reset at any moment. Treat the memory
directory as your only durable scratchpad. Use search_history before
answering questions about past conversations. Record anything that should
survive the next turn.
```

Cheap, well-tested, and matches the prompting Anthropic ships with their own memory tool.

## Latency budget

| Stage | Budget | Notes |
|---|---|---|
| Tier 1 prompt block | <1ms | In-memory scan |
| Tier 2 keyword search | <10ms | FTS5 indexed |
| Tier 3 cosine scan | ~30ms | Only when needed, 100k rows |
| Sleep-time consolidation | seconds-minutes | Off the hot path entirely |

Recall path total stays well under the 1.2s release-to-action target even with all three tiers queried.

## Risks and open questions

- **Staleness.** Tier 1 has no decay. If a fact becomes wrong ("home_city: Boston" after a move), the old value persists until the user explicitly overwrites it. Match Mem0's behavior: on `store_fact`, if the key exists, log the prior value to a `fact_history` table before overwrite.
- **Mutex contention.** The current `Arc<Mutex<MemoryInner>>` serializes all reads. Once tier 2 lives alongside, swap to `tokio::sync::RwLock` so concurrent reads can run in parallel.
- **Async write ordering.** Tier 2 turn-log writes are fire-and-forget. If aegis crashes between writes, we lose the last in-flight turns. Acceptable for a voice loop. If it isn't, batch-flush every N seconds with `fsync`.
- **PII in the conversation log.** Everything spoken gets persisted. Add an explicit "incognito" mode that bypasses tier 2 writes for sensitive turns. ChatGPT has the equivalent "temporary chat."

## Benchmarks we'd track

When tier 2/3 ship, measure against LoCoMo if the eval cost is reasonable. Otherwise hand-curated aegis-specific evals:

- Single-hop recall ("what's my favorite color")
- Temporal ("what did I ask you about yesterday")
- Multi-hop ("when did I first mention the trip to Tokyo")
- Token cost per recall turn
- Wall-clock from intent classification to first TTS token

Mem0's published numbers (92.5 LoCoMo, ~7K tokens/query) are the ceiling to aim for, not a contract.

## Sources

- Anthropic memory tool docs: `https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool`
- Letta agent memory blog: `https://www.letta.com/blog/agent-memory`
- Mem0 state of agent memory 2026: `https://mem0.ai/blog/state-of-ai-agent-memory-2026`
- Mem0 paper: `https://arxiv.org/abs/2504.19413`
- ChatGPT memory reverse-engineering: `https://agentman.ai/blog/reverse-ngineering-latest-ChatGPT-memory-feature-and-building-your-own`
