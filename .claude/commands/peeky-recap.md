Recap the most recent Aegis session — what the user said, what Aegis did, and what it answered — by reading Aegis's log. This is the return half of the handoff: it tells Claude Code what happened while the user was using Aegis.

1. Read the Aegis log: `~/Library/Application Support/aegis/logs/aegis.log` on macOS, or `~/.config/aegis/logs/aegis.log` on Linux. The previous run is the same path with a `.log.old` suffix; read it too if the user asks for an earlier session. The log rotates per launch, so the current file is the latest session.
2. Ignore the noisy `[render]` FPS lines. The meaningful lines are:
   - `you said:` — what the user spoke
   - `[intent] →` — how it was classified (Chat / FindAction / etc.)
   - `ACTION FIRES` / `emitted=` / `[action:...]` / `[input:type] injecting` — what Aegis did (click, type, open_url)
   - `claude:` — what Aegis said back
3. Summarize as a short, ordered recap — one line or two per voice turn: what the user asked, what Aegis did, how it responded. Keep it tight and skip the noise.
4. If the log is empty or Aegis hasn't run, just say so.
