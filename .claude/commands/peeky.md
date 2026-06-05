Spawn Peeky, the voice-controlled cursor agent, handing off the current conversation so it knows what the user was working on. Peeky's single-instance guard kills any prior copy on launch, so just start it.

1. Write a short, high-signal summary of THIS conversation — the task, key files/decisions, and current state, just a few sentences — to the peeky handoff file: `~/Library/Application Support/peeky/handoff.md` on macOS, or `~/.config/peeky/handoff.md` on Linux. Create the directory if needed. Keep it terse; Peeky injects it into its prompt and consumes (deletes) it on launch.
2. Find the repo root with `git rev-parse --show-toplevel`. The peeky binary is whichever of `<root>/target/release/peeky` or `<root>/target/debug/peeky` was built most recently (`ls -t`; on Windows it is `peeky.exe`). If neither exists, stop and tell the user to build it first: `cargo build -p peeky`.
3. Spawn it detached, run from the repo root so it finds `./models/routelet`:
   `cd <root> && nohup <binary> > /tmp/peeky.log 2>&1 &`
4. Wait about 3 seconds, then confirm it started — check that `peeky ready` appears in the peeky log (`~/Library/Application Support/peeky/logs/peeky.log` on macOS, `~/.config/peeky/logs/peeky.log` on Linux).
5. Tell the user Peeky is up with the conversation context — hold Ctrl+Space to talk.
