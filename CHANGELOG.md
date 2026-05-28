# Changelog

All notable changes to aegis are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). aegis follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it hits 1.0; pre-1.0 versions may break anything between minor bumps.

## [Unreleased]

### Added

- On-device ONNX intent classifier (`aegis/src/routelet.rs`) via tract. Replaces the per-turn Claude classifier call with a local SetFit model. Falls back to Claude only when calibrated confidence is below `ROUTELET_CONFIDENCE_THRESHOLD`.
- `demo_routelet` bin (`demos/src/bin/demo_routelet.rs`) for hand-testing the classifier.
- `PRIVACY.md` documenting what leaves the device and how on-device logging works.
- `AEGIS_ROUTELET_LOG` env var to opt into redacted router logging at `~/.config/aegis/routelet_log.jsonl` (capped at 5000 lines).
- SQLite + FTS5 conversation log (`aegis/src/providers/claude/history.rs`). Per-turn record + keyword search with porter stemming and BM25 ranking.
- Tool-routing eval harness (`aegis/evals/runners/tool_routing.rs`). Runs cases from `aegis/evals/cases/tool_routing.json` through the live classifier and reports pass rate, per-category breakdown, and confusion matrix.
- Cursor SVG hero in the project README.
- `CONTRIBUTING.md` with contributor gate (auto-close, `lgtm`/`lgtmi` approval, quality bar).
- `AGENTS.md` with development rules for humans and agents working in the repo.
- GitHub issue templates (bug, feature) and auto-close workflow for new contributors.

### Changed

- Renamed `aegis/examples/` → `aegis/demos/`. Files renamed from `test_*.rs` → `demo_*.rs` (hand-run dev tools) and `bench_*.rs` (latency benchmarks). Each is now a `[[bin]]` entry in `Cargo.toml`; run with `cargo run --bin <name>`.
- Dropped `AUDIO_POST_RELEASE_GRACE_MS` from 800 to 0 in `tuning.rs`.

## [0.1.0]

Initial development. Voice-controlled cursor with five intent paths (find_action, integration, chat, memory, agent), Hyprland-native rendering, hosted Cloudflare Worker proxy, and JSONL fact storage.
