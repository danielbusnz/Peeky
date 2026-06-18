# Testing Peeky on macOS

First-run guide for grabbing the latest CI artifact and verifying it on a Mac.

## Status

- CI builds + tests pass on macOS.
- Retina position fix: shipped.
- Transparency: wgpu-backed (softbuffer 0.4 strips alpha via `CGImageAlphaInfo::NoneSkipFirst`; wgpu's `PostMultiplied` honors per-pixel alpha).
- Cursor visible across Spaces and over fullscreen apps via `setCollectionBehavior`.
- End-to-end voice turn on macOS: pending verification.

## Prerequisites

- A Mac (`macos-latest` CI runs arm64; if the binary fails with "bad CPU type" we need an x86_64 matrix).
- `gh` CLI: `brew install gh && gh auth login`.
- Terminal you can read stderr in.

## 1. Download

```bash
mkdir -p ~/peeky-test && cd ~/peeky-test
LATEST=$(gh run list --workflow=macos.yml --limit 1 \
  --repo danielbusnz/Peeky \
  --json databaseId --jq '.[0].databaseId')
gh run download "$LATEST" --repo danielbusnz/Peeky --name peeky-macos
chmod +x peeky
```

## 2. Strip Gatekeeper quarantine

```bash
xattr -d com.apple.quarantine ./peeky
```

Without this, macOS shows "could not be verified" and refuses to run.

## 3. Run from terminal

```bash
./peeky
```

Run from terminal, not Finder. We need stderr.

## What to expect

### Provider config missing

If you see `STT init failed` or `missing CARTESIA_API_KEY`, the proxy URL or keys aren't wired. Capture the panic and report.

### Permission prompts

macOS should prompt for:
- **Microphone** (cpal)
- **Accessibility** (global hotkey + input injection)
- **Screen Recording** (xcap)

Approve each. macOS may force a re-launch after Accessibility.

No prompts at all = binary failing silently before reaching those code paths.

### Hotkey

Expect:
```
[hotkey] registered (global)
peeky ready. hold Ctrl+Space to talk
```

`register: ...` followed by an error usually means Accessibility was denied or Ctrl+Space is taken.

### Overlay

The cursor should swell into a soundwave when you hold the hotkey. Renderer: winit + wgpu + tiny-skia (see `ai_cursor/macos.rs`, `renderer.rs`).

Untested:
- Premultiplied-vs-postmultiplied alpha mismatch may darken sprite edges.
- Flicker or tearing.

If the screen still goes black, transparency setup is deeper than expected.

### Voice turn

Hold `Ctrl+Space`, say "what's my name", release.

Best case: transcribe → classify as `chat` → Claude → Cartesia → spoken reply.

Pure-logic paths (classifier, memory, parsing) all pass tests. I/O paths (mic, screen capture, TTS playback) are first-time runs on macOS.

### find_action and clicks

"click the Chrome tab" runs the `find_action` path via `actions.rs`. macOS uses CGEvent (objc2-core-graphics) instead of Linux's ydotool.

"where is the search bar" should still move the cursor even if clicks don't fire.

## What to report

1. Full stderr (first 50 lines minimum).
2. Permission prompts seen / not seen.
3. Whether the overlay appeared, where.
4. `sw_vers -productVersion`.
5. `uname -m`.

## Known gaps

- No `.app` bundle yet (running raw Mach-O).
- No signing or notarization.
- No `Info.plist` permission usage strings.
- Overlay not yet `setIgnoresMouseEvents:YES` in all builds.
- Premultiplied vs postmultiplied alpha mismatch.
- No first-run UX for permissions.

## Refreshing the artifact

```bash
LATEST=$(gh run list --workflow=macos.yml --limit 1 \
  --repo danielbusnz/Peeky \
  --json databaseId --jq '.[0].databaseId')
gh run download "$LATEST" --repo danielbusnz/Peeky --name peeky-macos
chmod +x peeky
xattr -d com.apple.quarantine ./peeky 2>/dev/null || true
./peeky
```
