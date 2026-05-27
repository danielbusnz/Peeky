# Aegis Agent Guide

Orientation and rules for humans and agents working in this repo. The first half maps the codebase (what it is, how a voice turn flows, where things live). The second half is the working rules. If you use an agent, run it from the repo root so it picks this file up automatically.

## Overview

Aegis is a voice-controlled AI cursor for Linux, written in Rust. Hold a push-to-talk hotkey, say something, release. The transcript is classified into one of five intents, the matching path runs, and the reply streams back as speech (and, when relevant, the cursor moves or a real click/type fires).

A single voice turn flows like this:

1. **Hotkey** (`aegis/src/hotkey/`) flips a global `RECORDING` atomic. On Hyprland, `bind`/`bindr` send `SIGUSR1` (press) / `SIGUSR2` (release) to the process; a signal-hook listener thread flips the flag. Other platforms use the `global-hotkey` crate polled from the winit loop.
2. **Audio capture** (`aegis/src/audio/input.rs`) holds a persistent `cpal` mic stream and forwards PCM to the STT channel while the key is held, with a pre-roll ring and a post-release grace window.
3. **STT** (`aegis/src/providers/stt_deepgram.rs`) streams PCM over a Deepgram websocket and returns the final transcript on release, after a short quiescence wait for multi-segment utterances.
4. **Classify** (hybrid). The keyword classifier (`aegis/src/intent.rs`) runs first, sub-millisecond. If it returns `None`, the LLM classifier (`aegis/src/providers/claude/classifier.rs`) makes a forced-tool Claude call, spawned in parallel with the screenshot capture so its latency is mostly hidden.
5. **Dispatch** (`aegis/src/orchestrator.rs`) routes to one of five paths, all under `aegis/src/providers/claude/`: `find_action`, `integration`, `chat`, `memory`, `agent`.
6. **TTS** (`aegis/src/providers/tts_cartesia.rs`). Claude deltas are split into sentences and streamed to Cartesia, which synthesizes PCM into the `rodio` sink. The first flush is permissive (fast first audio); later flushes are strict (natural prosody).
7. **Barge-in** (`aegis/src/barge_in.rs`). A watchdog polls the hotkey; a re-press mid-turn cancels the in-flight Claude and Cartesia streams so the next turn starts clean.

`aegis/src/orchestrator.rs` is the core loop: one voice turn per iteration. Start there.

## Architecture

**Workspace** (`Cargo.toml`). Four Rust members: `aegis` (the agent binary plus its library), `demos` (hand-run dev tools and benchmarks), `launcher/src-tauri` (the Tauri onboarding app), and `memex` (a localhost personal-data daemon). The `proxy/` directory is **not** a workspace member: it is a TypeScript Cloudflare Worker.

**The `aegis` crate** splits into `lib.rs` (every subsystem exposed as a public module) and a thin `main.rs`, so the out-of-tree `demos` crate builds against the same modules. Default feature is `hyprland`; the winit/X11 path builds with `--no-default-features`.

**Platform-backend pattern.** Several subsystems (`hotkey/`, `input/`, `desktop/`, `screenshot/`, `mouse_position/`, `ai_cursor/`) share one shape: a `mod.rs` facade, a `backend.rs` trait, and per-OS implementations (`hyprland.rs`, `macos.rs`, `windows.rs`, `crossplatform.rs`/`winit.rs`) selected by feature flags. When adding a platform capability, add it to the trait and implement it in every backend.

**Sibling crates and services:**

- **launcher** (`launcher/src-tauri/`): Tauri 2 first-run onboarding. Collects an invite code or the user's own API keys (stored in the OS keychain), requests macOS TCC permissions, then spawns the `aegis` binary as a child with the right env. If `~/.config/aegis/onboarded` exists, it spawns silently and exits.
- **memex** (`memex/`): an Axum daemon on `127.0.0.1:7142` (override with `MEMEX_ADDR`) meant to ingest personal data and serve a query API. Currently a scaffold: routes are wired (`/health`, `/search`, `/recent/{source}`), the store is TODO.
- **proxy** (`proxy/src/index.ts`): the Cloudflare Worker that holds the real API keys and enforces per-tier usage caps. Deployed with Wrangler. See routes below.

## API Proxy

The app ships without API keys. By default every provider call routes through the Worker, which holds the secrets and meters usage (free trial turns, or per-day caps under an invite code). Each provider can be pointed straight at its upstream with an `AEGIS_*_DIRECT=1` env var plus the matching key (see "Use your own API keys" in the README).

| Route | Method | Upstream | Purpose |
| --- | --- | --- | --- |
| `/v1/anthropic/messages` | POST | Anthropic Messages API | Full SSE proxy for Claude; enforces daily token caps |
| `/v1/deepgram/token` | POST | Deepgram | Mints a short-lived STT token; client opens the websocket directly |
| `/v1/cartesia/token` | POST | Cartesia | Mints a short-lived TTS token; client connects directly |
| `/v1/invite/verify` | POST | KV | Validates an invite code (exists, not expired, device slot free) |
| `OPTIONS *` | OPTIONS | none | CORS preflight |

Deepgram and Cartesia never sit on the data path: the Worker only mints a token, then the client streams to them directly. Worker secrets: `ANTHROPIC_API_KEY`, `DEEPGRAM_API_KEY`, `CARTESIA_API_KEY`. Source and deploy notes: `proxy/src/index.ts` and `proxy/README.md`.

## Key Files

Pipeline and entry points:

| File | ~Lines | Purpose |
| --- | --- | --- |
| `aegis/src/main.rs` | 48 | Thin binary. Builds the session and runs the turn loop. |
| `aegis/src/lib.rs` | - | Library root. Exposes every subsystem as a public module. |
| `aegis/src/orchestrator.rs` | 800 | Core loop. One turn: record → classify → dispatch → stream TTS → barge-in. |
| `aegis/src/voice_session.rs` | 64 | Session holder (tokio runtime, mic, audio sink, provider clients, memory), built once at startup. |
| `aegis/src/intent.rs` | 460 | Keyword classifier (fast path) and its disambiguation tests. |
| `aegis/src/tuning.rs` | 52 | Every behavior dial in one place, each with an `↑`/`↓` tradeoff comment. |

Claude paths (`aegis/src/providers/claude/`):

| File | ~Lines | Purpose |
| --- | --- | --- |
| `mod.rs` | 152 | Claude client. Auth (proxy vs `AEGIS_ANTHROPIC_DIRECT`), request builder, connection warming. |
| `classifier.rs` | 246 | LLM fallback classifier. Forced tool call returns the intent enum. |
| `find_action.rs` | 280 | Point/click/type/scroll from a single screenshot query; action fires mid-stream. |
| `integration.rs` | 266 | Service calls. Two-step: pick tool (forced), dispatch, then summarize. |
| `chat.rs` | 138 | Pure Q&A. No screen, no tools. Injects the user profile from memory. |
| `memory.rs` | 549 | Store/recall facts in `~/.config/aegis/memory.jsonl`. |
| `agent_loop.rs` | 526 | Multi-step loop with iterative screenshots (cap `AGENT_MAX_STEPS`). |
| `parsing.rs` | 815 | Tool schemas, SSE parsing, old-screenshot trimming, `cache_control` markers. |

Providers and I/O:

| File | Purpose |
| --- | --- |
| `aegis/src/providers/stt_deepgram.rs` | Deepgram websocket STT (proxy token or direct key). |
| `aegis/src/providers/tts_cartesia.rs` | Cartesia streaming TTS (proxy token or direct key). |
| `aegis/src/providers/device_id.rs`, `invite_code.rs` | Persisted identifiers for proxy auth, re-read per request. |
| `aegis/src/audio/input.rs`, `output.rs` | `cpal` mic capture and `rodio` playback. |
| `aegis/src/actions.rs` | Serialized executor draining Claude's input actions to the platform input backend. |
| `aegis/src/input/`, `desktop/`, `screenshot/`, `mouse_position/` | Platform backends: input injection, window/app management, capture, cursor position. |
| `aegis/src/ai_cursor/`, `painter.rs` | The blue cursor overlay and its renderer (GTK/cairo on Hyprland). |
| `aegis/src/barge_in.rs` | Re-press watchdog that fires the cancellation token. |
| `aegis/src/tray.rs` | macOS menu bar icon. |

Integrations (`aegis/src/integrations/`): `mod.rs` is the registry (`all_tools()` + `dispatch()`); `gmail.rs`, `spotify.rs`, `github.rs`, `youtube.rs`, `health.rs` are the service handlers, each gated on its own credentials.

## Architecture Decisions

The non-obvious "why"s. Check these before changing the related behavior.

- **Push-to-talk over Unix signals (Hyprland).** Hyprland's `bind`/`bindr` can signal a process matched by regex, so press/release map to `SIGUSR1`/`SIGUSR2` and a signal-hook thread flips a global atomic (no polling). The README's `pkill -SIGUSR1/-SIGUSR2` hotkey config depends on this exact mechanism.
- **Proxy by default, per-provider direct opt-out.** Zero keys needed to run. `AEGIS_ANTHROPIC_DIRECT` / `AEGIS_DEEPGRAM_DIRECT` / `AEGIS_CARTESIA_DIRECT` (each with its matching key) bypass the Worker. Mix and match.
- **Hybrid classify, LLM overlapped with the screenshot.** The keyword path resolves most turns with no round-trip. When it can't, the LLM classifier call runs in parallel with the screenshot capture/resize so its latency is largely hidden. Voice turns are latency-bound; do not add synchronous work before TTS without checking the budget in `tuning.rs`.
- **Keyword order: FindAction before Integration.** Locator verbs ("where is", "click", "show me") are stronger signals than a bare service name. "where's my YouTube button" must point at the button, not play a video. There are tests pinning this; keep them green.
- **TTS first flush permissive, later flushes strict.** The first sentence flushes on a clause break (comma/semicolon/colon) once past `TTS_FIRST_FLUSH_MIN_CHARS`, to start audio fast. Once speech is rolling, only `.!?` flush, for natural prosody.
- **Don't early-cancel on integration actions.** Visual actions (point/click/type) get on-screen feedback, so the Claude stream exits early to cut chatter. Integration actions are silent API calls whose results must flow back for Claude to speak a summary, so they are not cancelled.
- **Agent loop bounds requests.** `AGENT_KEEP_RECENT_SCREENSHOTS` strips image bytes from older tool results to keep request bodies small; `AGENT_SETTLE_MS` waits for the UI to repaint before the next screenshot so the model doesn't act on a pre-animation frame.
- **Invite code and device id are re-read per request.** So the onboarding window can change them without restarting aegis.

## Conversational Style

- Keep answers short and concise.
- No emojis in commits, issues, PR comments, or code.
- No fluff or cheerful filler text.
- Technical prose only. Be direct.
- When the user asks a question, answer it first before making edits or running implementation commands.
- When responding to user feedback or an analysis, explicitly say whether you agree or disagree before saying what you changed.
- Never use em dashes or en dashes in prose, comments, or commit messages. Use periods, commas, or restructure.

## Code Quality

- Read files in full before wide-ranging changes, before editing files you have not fully inspected, and when asked to investigate or audit. Do not rely on search snippets for broad changes.
- No `unwrap()` or `expect()` in production paths without a `// SAFETY:` or `// reason:` comment explaining why the invariant holds. Tests, demos, evals, and `main()` startup may unwrap freely.
- Prefer `Result<T, E>` over `panic!` outside `main()`.
- Inline single-line helpers that have only one call site.
- Reach for `?` over manual match arms when the only thing happening on the Err side is propagation.
- Don't add `pub` you don't need. Field and function visibility is the smallest interface that compiles.
- Match the existing comment style. Comments describe what code can't say (Ousterhout discipline). Don't restate what's obvious from the symbol names.
- Always ask before removing functionality or code that appears intentional.
- Do not preserve backward compatibility unless the user asks for it.
- Never hardcode tuning constants in the middle of logic. Put them in `aegis/src/tuning.rs` with an `↑` / `↓` tradeoff comment.
- When touching `providers/claude/*`, remember the hot path is voice-latency-bound. Don't add work that runs synchronously before TTS without checking the budget in `tuning.rs`.

## Commands

- After code changes (not docs): `cargo check` (full output, no tail). Fix all errors and warnings before committing.
- After touching anything in `src/`, run `cargo test --bin aegis` and confirm all unit tests pass.
- Don't run `cargo build --release` unless asked. It's slow and unnecessary for verification.
- Don't run evals (`cargo run --bin eval_*`) unless asked. They cost real API credits.
- Don't run demos (`cargo run --bin demo_*` / `bench_*`) unless asked. Many require hardware (mic, screen, hotkey daemon) or live API access.
- For features other than the default (`hyprland`), verify with `cargo check --no-default-features --features <combo>` (e.g. `winit-window,crossplatform`).
- If you create or modify a unit test, run it and iterate on test or implementation until it passes.
- For ad-hoc scripts, write them to `/tmp` (not `~/Scripts/`), run, remove when done. Don't embed multi-line scripts in `bash` commands.
- Never commit unless the user asks.

## Where Things Go

- **Unit tests**: inline in source files via `#[cfg(test)] mod tests`. Run with `cargo test`. Must be deterministic and free.
- **Integration tests**: `aegis/tests/`. Run with `cargo test`. None exist yet but the directory is reserved.
- **Demos**: `aegis/demos/demo_*.rs`. Hand-run dev tools. Each is a `[[bin]]` entry in `Cargo.toml`.
- **Benchmarks**: `aegis/demos/bench_*.rs`. Same shape as demos but report latency stats over N iterations.
- **Evals**: `aegis/evals/runners/*.rs` (runner code) + `aegis/evals/cases/*.json` (case data) + `aegis/evals/results/` (gitignored output). Run with `cargo run --bin eval_<name>`. LLM behavior tests. Stochastic, paid, not part of CI.
- **Providers**: `aegis/src/providers/`. Each external service (Claude, Deepgram, Cartesia, integrations) is its own module.
- **Tuning constants**: `aegis/src/tuning.rs`. Every behavior dial in one place.
- **Memory architecture**: `aegis/docs/memory-architecture.md`. Three-tier design (facts JSONL, history SQLite+FTS5, future embeddings).

## Dependencies

- Treat `Cargo.lock` changes as reviewed code. Direct external deps stay pinned to the minor version (`"1.2.3"`-style entries in `Cargo.toml`).
- Update locally with `cargo update -p <crate>` for targeted bumps. Avoid blanket `cargo update`.
- New dependencies require justification. Prefer pulling in fewer features (`default-features = false`) over the whole crate when possible.

## Git

Multiple agent sessions may be running in this cwd at the same time, each modifying different files. Git operations that touch unstaged, staged, or untracked files outside your own changes will stomp on other sessions' work. Follow these rules:

Committing:

- Only commit files YOU changed in THIS session.
- Stage explicit paths (`git add <path1> <path2>`); never `git add -A` / `git add .`.
- Before committing, run `git status` and verify you are only staging your files.

Never run (destroys other agents' work or bypasses checks):

- `git reset --hard`, `git checkout .`, `git clean -fd`, `git stash`, `git add -A`, `git add .`, `git commit --no-verify`.

Commit message style:

- Lowercase prefix matching the area of change, then a colon, then a terse description. Examples: `history: add sqlite + fts5 conversation log`, `tuning: zero post-release grace`, `demos: move examples/ → demos/`.
- No emoji, no AI-tell words ("comprehensive", "enhance", "streamline", "robust", "leverage").
- No `Co-Authored-By:` lines. Maintainer commits look solo.
- No `Generated with Claude Code` footers.

If rebase conflicts occur:

- Resolve conflicts only in files you modified.
- If a conflict is in a file you did not modify, abort and ask the user.
- Never force push.

## Issues and PRs

See `CONTRIBUTING.md` for the contributor gate (auto-close, `lgtm`/`lgtmi`, quality bar).

When creating issues, add area labels for affected modules:

- `area:voice` (STT, TTS, audio pipeline)
- `area:classifier` (intent routing)
- `area:memory` (facts, history, eval, retrieval)
- `area:agent` (multi-step agent loop)
- `area:integrations` (Gmail, GitHub, Spotify, YouTube)
- `area:ui` (cursor overlay, soundwave, loading)
- `area:platform` (Hyprland, winit, cross-platform)
- `area:build` (Cargo, CI, dependency management)

Use all that apply.

When posting issue/PR comments:

- Write the comment to a temp file and post with `gh issue/pr comment --body-file` (never multi-line markdown via `--body`).
- Keep comments concise, technical, in the user's tone.

When closing issues via commit:

- Include `fixes #<number>` or `closes #<number>` in the message so merging auto-closes the issue. For multiple issues, repeat the keyword per issue (`closes #1, closes #2`); a shared keyword only closes the first.

## Releases

aegis is a binary crate, not a published library. Releases are:

1. Update `aegis/CHANGELOG.md` under the `## [Unreleased]` section (create if missing).
2. Bump `version` in `aegis/Cargo.toml`.
3. Build a release binary: `cargo build --release --bin aegis`.
4. Smoke test the binary on the target platform.
5. Tag the commit (`git tag v0.X.Y`) and push the tag.
6. (Optional) Create a GitHub release with the binary attached.

No npm, no crates.io publish, no 2FA flow.

## Keeping This File Current

When a change makes the orientation half stale, update it in the same session:

- New or deleted source files that matter for orientation: add or remove the row in "Key Files".
- A new subsystem, crate, platform backend, or proxy route: update "Architecture" or "API Proxy".
- A new non-obvious tradeoff or gotcha: add it to "Architecture Decisions".
- A pipeline change (a stage moves, splits, or is reordered): fix the flow in "Overview".

Skip updates for minor edits, bug fixes, and refactors that leave the documented structure intact. Approximate line counts only need fixing when they drift by more than ~50 lines.

## User Override

If the user's instructions conflict with any rule in this document, ask for explicit confirmation before overriding. Only then execute their instructions.
