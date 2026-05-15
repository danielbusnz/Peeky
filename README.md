# aegis

A voice-controlled AI cursor. Hold a hotkey, ask a question, and the cursor flies to whatever you asked about.
Built in Rust. Targets Hyprland today, cross-platform later.

Sub-3-second release-to-speech latency on a typical desktop.


## Quickstart

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis

cat > .env <<EOF
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
CARTESIA_API_KEY=...
EOF

cargo build --release
```

Add hotkey bindings to your Hyprland config (`~/.config/hypr/hyprland.conf`):

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(aegis|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(aegis|test_)"
```

Then:
```bash
hyprctl reload
cargo run --release --bin aegis
```

Hold INSERT, ask something, release.

#
## Windows / macOS / X11 build

Cross-platform impls live behind the `winit-window` + `crossplatform` features.
Build the end-to-end smoke test (hotkey + mouse + screenshot + cursor) with:

```bash
cargo run --bin test_win --no-default-features --features winit-window,crossplatform
```

Hold `Insert`, release. Each turn logs mouse pos, saves a screenshot to the
temp dir, and flies the cursor sprite to the mouse position. Click-through is
on, so apps below the overlay still receive input.

## Status

Hyprland is the default. Cross-platform support (winit + xcap + mouse_position
+ global-hotkey) is in active development behind the `winit-window` and
`crossplatform` Cargo features.

## License

MIT
