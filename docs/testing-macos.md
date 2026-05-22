# Testing Aegis on macOS

First-run guide for grabbing the latest CI artifact and seeing whether
aegis actually runs on a Mac. This doc tracks what we know works, what
to expect, and what to report back when something breaks.

Status as of commit `c21315a` (2026-05-22):

- CI builds and tests pass on macOS.
- Retina scale-factor bug is fixed: the cursor sprite renders at the
  correct position.
- Transparency is now wgpu-backed (softbuffer 0.4 hardcodes
  `CGImageAlphaInfo::NoneSkipFirst` which strips alpha on macOS; wgpu's
  `CompositeAlphaMode::PostMultiplied` honors per-pixel alpha).
- Runtime is partially verified. Black-screen issue is expected to be
  fixed by the wgpu swap; first end-to-end voice turn on macOS is still
  the next thing to verify.

## Prerequisites

- A Mac (Apple Silicon or Intel both fine, the artifact is built for
  whichever runner CI happened to pick; if the binary fails to launch
  with "bad CPU type", you've got the wrong arch and we need to add a
  matrix build).
- `gh` CLI installed: `brew install gh` then `gh auth login`.
- A terminal you can read stderr in.

## 1. Download the artifact

```bash
mkdir -p ~/aegis-test && cd ~/aegis-test
gh run download 26310905925 \
  --repo danielbusnz-lgtm/Aegis \
  --name aegis-macos
chmod +x aegis
```

Run ID `26310905925` is the first green macos-build with the wgpu
renderer. For a fresh run, check `gh run list --workflow=macos.yml
--limit 1` and use that ID, or use the "Refreshing the artifact"
script at the bottom of this doc.

Alternative if you don't want gh: download from the run page in a
browser. The artifact is at the bottom of the run page under
**Artifacts**.

## 2. Strip the Gatekeeper quarantine flag

macOS marks anything downloaded as quarantined and refuses to run
unsigned binaries. The flag is a per-file extended attribute. Remove it:

```bash
xattr -d com.apple.quarantine ./aegis
```

Without this, you'd get a "could not be verified" dialog. With it,
macOS treats the binary like anything you compiled locally.

If you ever lose track of which flags are set:

```bash
xattr -l ./aegis
```

Should print nothing after the strip.

## 3. Launch from a terminal

```bash
./aegis
```

Run from terminal, not from Finder. The binary writes diagnostic logs
to stderr and we need to see them.

## What to expect

In order of how the process unfolds:

### Immediate failure: missing provider config

If you see a panic line like `STT init failed` or `missing
CARTESIA_API_KEY`, the binary couldn't find provider config. The
codebase usually routes through a hosted Cloudflare Worker proxy by
default. You may need a `.env` next to the binary with the proxy
endpoint or specific keys.

If this happens, capture the full panic message and report it.

### Permission prompts

macOS should prompt you for three permissions on first use:

- **Microphone**: cpal needs this to capture audio.
- **Accessibility**: required to register the global hotkey and to
  inject cursor moves and clicks.
- **Screen Recording**: xcap needs this to capture the active workspace
  for `find_action` calls.

Approve each one. macOS may force a re-launch after Accessibility is
granted; that's normal.

If you don't see any prompts at all, the binary is likely failing
silently before it reaches those code paths. Capture stderr and report.

### Hotkey registration

You should see something like:

```
[hotkey] registered (global)
aegis ready. hold Cmd+Shift+Space to talk
```

`Cmd+Shift+Space` is the macOS-specific binding (the Linux build uses
plain `Insert`). If you see `register: …` followed by an error, the
global hotkey couldn't bind, usually because Accessibility was denied
or the chord is already taken by something else.

### Overlay rendering

The cursor should swell into a soundwave when you hold the hotkey. On
macOS the renderer is winit + wgpu + tiny-skia (the wgpu path is
documented in `aegis/src/ai_cursor/macos.rs` and `renderer.rs`).
Previously verified:

- Position: correct after the Retina scale-factor fix.
- Transparency: should now show the desktop through the overlay window
  rather than rendering as opaque black.

Still possible / not yet tested:

- Overlay blocks mouse clicks (we haven't set `ignoresMouseEvents: true`
  on the NSWindow yet).
- Sprite anti-aliased edges look slightly darker than intended (we're
  feeding premultiplied RGBA into wgpu's PostMultiplied alpha mode for
  v1; small visual artifact at the rounded edges).
- Flicker or tearing.

If transparency still fails (entire screen still goes black), the
black-screen issue is deeper than expected and we'll need to look at
wgpu surface configuration.

### Try a voice turn

Hold `Cmd+Shift+Space`, say something simple ("what's my name"),
release.

Expected best case: aegis transcribes, classifies as `chat`, calls
Claude, streams back through Cartesia, you hear a spoken reply.

Expected current case: probably crashes or fails somewhere in the
pipeline. The pure-logic parts (classifier, memory, Claude parsing)
work; the I/O parts (mic, screen capture, TTS playback) are first-time
runs on macOS.

### `find_action` and click actions

If you say "click the Chrome tab" or similar, aegis runs the
find_action path which calls `actions.rs` to inject a real click. On
Linux this shells out to `ydotool`. On macOS, `ydotool` doesn't exist,
so the click will silently fail.

The cursor move part might still work (it uses a different code path).
Verify by saying "where is the search bar"; the cursor should jump even
if subsequent clicks don't fire.

## What to report back

For any failure, capture:

1. The full stderr output (first 50 lines minimum; everything if it's
   short).
2. Which permission prompts you saw (and which you didn't).
3. Whether the overlay appeared, and where if so.
4. macOS version: `sw_vers -productVersion`.
5. Architecture: `uname -m`.

Paste those into a session with me and we'll fix whatever surfaces.

## Known gaps before this is shippable to end users

These are tracked separately and not the goal of this manual test.
Listing them so you know what we're NOT testing:

- No `.app` bundle yet. We're running a raw Mach-O binary.
- No code signing or notarization.
- No `Info.plist` with permission usage strings (some macOS versions
  refuse to launch without these).
- `actions.rs` ydotool shell-out won't work; needs to be swapped for
  enigo or CGEvent.
- Overlay window isn't `setIgnoresMouseEvents:YES`, so it may block
  clicks.
- Premultiplied vs postmultiplied alpha mismatch in the wgpu renderer
  may make sprite edges slightly darker than intended.
- No first-run UX explaining the permission grants.

What's already addressed:

- Retina position scaling: fixed.
- macOS transparency at the renderer level: addressed by swapping
  softbuffer for wgpu. Pending runtime verification.

Phase 1 is "does the binary launch and put a working transparent overlay
on screen with a working voice turn." Once we know that, the rest of
the gaps get prioritized by what actually matters.

## Refreshing the artifact

Each push to main rebuilds the artifact. To grab the latest:

```bash
LATEST=$(gh run list --workflow=macos.yml --limit 1 \
  --repo danielbusnz-lgtm/Aegis \
  --json databaseId --jq '.[0].databaseId')
gh run download "$LATEST" \
  --repo danielbusnz-lgtm/Aegis \
  --name aegis-macos
chmod +x aegis
xattr -d com.apple.quarantine ./aegis 2>/dev/null || true
./aegis
```
