# Changelog

All notable changes to peeky are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). peeky follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it hits 1.0; pre-1.0 versions may break anything between minor bumps.

## [Unreleased]

## [0.1.20] - 2026-06-10

### Changed

- Retrain the on-device intent router for the v0.1.19 macOS integrations: "turn off wifi", "next slide", "facetime mom" now route on-device instead of falling through to the wrong path.
- Drop the `agent` class from the router. Agent turns route through the Claude classifier fallback until the spoken cue ships.
- Keep terminal question marks through transcript preprocessing, so capability questions ("can you see my screen?") route to chat instead of firing an action.

## [0.1.19] - 2026-06-10

### Added

- Finder integration: open, reveal, and trash files and folders by voice (`finder_open`, `finder_reveal`, `finder_trash`).
- Calendar integration: add events at relative times and list today's agenda.
- App control: open, quit, and list running macOS apps by name.
- Shortcuts integration: run any user Shortcut by name in the background, list shortcuts.
- Messages integration: send iMessages, with contact names resolved through the new Contacts lookup.
- Apple Music integration: transport control, current track, and library track search.
- Mail integration: send email and read the inbox unread count.
- Photos integration: show an album by name.
- Keynote integration: start, advance, and stop a presentation by voice.
- System controls: dark mode, sleep, screen saver (screen lock), wallpaper, notification banners, keep-awake via `caffeinate`, and Wi-Fi power.
- Clipboard read and write tools.
- Spotlight file search via `mdfind`, pairs with Finder reveal for "find that pdf".
- FaceTime calls and Apple Maps directions/search via Apple URL schemes.

## [0.1.8] - 2026-05-28

### Added

- On-device ONNX intent classifier (`peeky/src/routelet.rs`) via tract. Replaces the per-turn Claude classifier call with a local SetFit model. Falls back to Claude only when calibrated confidence is below `ROUTELET_CONFIDENCE_THRESHOLD`.
- `demo_routelet` bin (`demos/src/bin/demo_routelet.rs`) for hand-testing the classifier.
- `PRIVACY.md` documenting what leaves the device and how on-device logging works.
- `PEEKY_ROUTELET_LOG` env var to opt into redacted router logging at `~/.config/peeky/routelet_log.jsonl` (capped at 5000 lines).
- Chat mode now includes a screenshot so Claude can see the user's screen and answer contextual questions ([#51](https://github.com/danielbusnz-lgtm/Peeky/pull/51)).
- Onboarding prompts macOS for microphone, accessibility, and screen recording permissions on first launch.
- SQLite + FTS5 conversation log (`peeky/src/providers/claude/history.rs`). Per-turn record + keyword search with porter stemming and BM25 ranking.
- Tool-routing eval harness (`peeky/evals/runners/tool_routing.rs`). Runs cases from `peeky/evals/cases/tool_routing.json` through the live classifier and reports pass rate, per-category breakdown, and confusion matrix.
- Cursor SVG hero in the project README.
- `CONTRIBUTING.md` with contributor gate (auto-close, `lgtm`/`lgtmi` approval, quality bar).
- `AGENTS.md` with development rules for humans and agents working in the repo.
- GitHub issue templates (bug, feature) and auto-close workflow for new contributors.

### Changed

- Renamed `peeky/examples/` → `peeky/demos/`. Files renamed from `test_*.rs` → `demo_*.rs` (hand-run dev tools) and `bench_*.rs` (latency benchmarks). Each is now a `[[bin]]` entry in `Cargo.toml`; run with `cargo run --bin <name>`.
- Dropped `AUDIO_POST_RELEASE_GRACE_MS` from 800 to 0 in `tuning.rs`.

### Fixed

- macOS `.dmg` ships signed and notarized again. The release workflow now notarizes the bundle out of band with a bounded `notarytool` wait, preventing the 6 hour hang that wedged `v0.1.7` ([#52](https://github.com/danielbusnz-lgtm/Peeky/pull/52)).

## [0.1.0]

Initial development. Voice-controlled cursor with five intent paths (find_action, integration, chat, memory, agent), Hyprland-native rendering, hosted Cloudflare Worker proxy, and JSONL fact storage.
