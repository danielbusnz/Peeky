Spawn Aegis, the voice-controlled cursor agent, handing off the current conversation so it knows what the user was working on. Aegis's single-instance guard kills any prior copy on launch, so just start it.

1. Write a short, high-signal summary of THIS conversation — the task, key files/decisions, and current state, just a few sentences — to the aegis handoff file: `~/Library/Application Support/aegis/handoff.md` on macOS, or `~/.config/aegis/handoff.md` on Linux. Create the directory if needed. Keep it terse; Aegis injects it into its prompt and consumes (deletes) it on launch.
2. Find the repo root with `git rev-parse --show-toplevel`. The aegis binary is whichever of `<root>/target/release/aegis` or `<root>/target/debug/aegis` was built most recently (`ls -t`; on Windows it is `aegis.exe`). If neither exists, stop and tell the user to build it first: `cargo build -p aegis`.
3. Spawn it detached, run from the repo root so it finds `./models/routelet`:
   `cd <root> && nohup <binary> > /tmp/aegis.log 2>&1 &`
4. Wait about 3 seconds, then confirm it started — check that `aegis ready` appears in the aegis log (`~/Library/Application Support/aegis/logs/aegis.log` on macOS, `~/.config/aegis/logs/aegis.log` on Linux).
5. Tell the user Aegis is up with the conversation context — hold Ctrl+Space to talk.
