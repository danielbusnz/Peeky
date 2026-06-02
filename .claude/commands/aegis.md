Spawn Aegis, the voice-controlled cursor agent, as a detached background process so it keeps running after this command. Aegis's own single-instance guard kills any prior copy on launch, so just start it.

1. Find the repo root with `git rev-parse --show-toplevel`. The aegis binary is whichever of `<root>/target/release/aegis` or `<root>/target/debug/aegis` was built most recently (`ls -t`; on Windows it is `aegis.exe`). If neither exists, stop and tell the user to build it first: `cargo build -p aegis`.
2. Spawn it detached, run from the repo root so it finds `./models/routelet`:
   `cd <root> && nohup <binary> > /tmp/aegis.log 2>&1 &`
3. Wait about 3 seconds, then confirm it started — check that `aegis ready` appears in the aegis log (`~/Library/Application Support/aegis/logs/aegis.log` on macOS, `~/.config/aegis/logs/aegis.log` on Linux).
4. Tell the user Aegis is up — hold Ctrl+Space to talk.
