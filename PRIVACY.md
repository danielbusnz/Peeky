# Privacy

Aegis runs on your machine and is built to keep your data local.

## What leaves your device

Each voice command is sent to the cloud services that fulfill it: Deepgram (speech to text), Anthropic Claude (reasoning and reading the screen), and Cartesia (text to speech). That is inherent to how the assistant answers you. The intent routing, deciding which kind of request you made, runs entirely on-device and sends nothing.

## On-device logging (off by default)

Aegis can log its routing decisions locally to improve the on-device classifier. It is opt-in: nothing is written unless you set `AEGIS_ROUTELET_LOG=1`.

When enabled:

- Each line is redacted before it is written. Passwords, secrets, tokens, emails, and runs of 4 or more digits (PINs, cards, phone numbers) are stripped.
- Lines are appended to `~/.config/aegis/routelet_log.jsonl`, on your machine only. Nothing is uploaded.
- The file is capped at the most recent 5000 lines.
- Delete it anytime: `rm ~/.config/aegis/routelet_log.jsonl`.

Redaction is pattern-based today, so it covers secrets, emails, and numbers but not names or addresses yet. If you dictate those and want them excluded, leave logging off. NER-based redaction for names and addresses is planned.

## Memory

Facts you ask Aegis to remember are stored locally at `~/.config/aegis/memory.jsonl` and never leave your device.
