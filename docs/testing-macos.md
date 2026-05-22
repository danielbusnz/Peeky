# Testing Aegis on macOS

First-run guide for grabbing the latest CI artifact and seeing whether
aegis actually runs on a Mac. This doc tracks what we know works, what
to expect, and what to report back when something breaks.

Status: CI builds and tests pass on macOS as of commit `4ce60eb`.
Runtime is unverified. This is the first manual test.

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
gh run download 26306486601 \
  --repo danielbusnz-lgtm/Aegis \
  --name aegis-macos
chmod +x aegis
```

Run ID `26306486601` is the first green macos-build. For a fresh run,
check `gh run list --workflow=macos.yml --limit 1` and use that ID.

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
macOS this uses winit + softbuffer + tiny-skia, which is the most
fragile path in the codebase right now. Possible issues:

- Overlay doesn't appear at all
- Overlay appears in the wrong location (Retina scale-factor bug)
- Overlay appears but blocks mouse clicks (we haven't set
  `ignoresMouseEvents: true` yet)
- Overlay flickers or tears

Note what you see.

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

These are tracked separately and not the goal of this first manual
test. Listing them so you know what we're NOT testing:

- No `.app` bundle yet. We're running a raw Mach-O binary.
- No code signing or notarization.
- No `Info.plist` with permission usage strings (some macOS versions
  refuse to launch without these).
- `actions.rs` ydotool shell-out won't work; needs to be swapped for
  enigo or CGEvent.
- Overlay window isn't configured as click-through transparent.
- No first-run UX explaining the permission grants.

Phase 1 is "does the binary launch and do anything useful." Once we
know that, the rest of the gaps get prioritized by what actually
matters.

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
