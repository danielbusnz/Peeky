# Security Policy

## Supported Versions

peeky is pre-1.0 and under active development. Only the `main` branch
receives fixes. There are no LTS or backported releases.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security problems.**

Email **danielbusnz@gmail.com** with:

- A clear description of the issue and the impact you believe it has.
- Reproduction steps, ideally a minimal proof of concept.
- The commit hash or release you reproduced against.
- Your suggested fix or mitigation, if you have one.

You should expect a first response within 5 business days. Reports
submitted Friday through Sunday land in the Monday queue. Reports that
turn out to be real are fixed on `main`, credited (unless you ask
otherwise) in the release notes, and disclosed after the fix ships.

## Out of Scope

The following are not treated as vulnerabilities:

- Bugs that require an already-compromised local user account.
- Issues in third-party services we route through (Anthropic, Deepgram,
  Cartesia, Spotify, etc.). Report those to the vendor directly.
- Findings from automated scanners with no exploit path demonstrated.
- Self-XSS or social-engineering scenarios.

## Hardening Notes

peeky ships with an opinionated default configuration:

- API calls route through a hosted Cloudflare Worker so no provider keys
  live on the client unless the user opts in via `.env`.
- The local memory store lives at `~/.config/peeky/memory.jsonl` and is
  never transmitted off-device.
- The agent loop has a hard step cap configured in `peeky/src/tuning.rs`
  to prevent runaway tool calls.

If you find a configuration in the repo that contradicts the above,
treat it as a security finding and report it.
