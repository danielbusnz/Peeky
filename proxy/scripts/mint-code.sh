#!/usr/bin/env bash
# Mint an invite code and register it in Cloudflare KV.
#
# Usage:
#   ./scripts/mint-code.sh <label> [days] [max_devices] [--local]
#
# Args:
#   label        Human tag baked into the code. Uppercase A-Z, 0-9, dashes.
#                Example: "RECRUITER-ACME"
#   days         Days until the code expires. Default 30.
#   max_devices  Number of distinct devices this code allows. Default 2.
#
# Codes meter against per-UTC-day budgets (Anthropic tokens, Deepgram/Cartesia
# mint counts), not a lifetime turn count. The daily budgets default to the
# values below; edit them here if a code needs more or less headroom.
#
# Requirements:
#   - wrangler logged in to the right Cloudflare account
#   - The USAGE_KV namespace already exists (see wrangler.toml)
#
# This writes to the REMOTE KV namespace, not the local dev one. Pass --local
# to mint a code only against local wrangler dev.

set -euo pipefail

if [[ $# -lt 1 || $# -gt 4 ]]; then
    echo "usage: $0 <label> [days] [max_devices] [--local]" >&2
    exit 64
fi

LABEL="${1^^}"
DAYS="${2:-30}"
MAX_DEVICES="${3:-2}"
LOCAL_FLAG=""
for arg in "$@"; do
    if [[ "$arg" == "--local" ]]; then LOCAL_FLAG="--local"; fi
done

if ! [[ "$LABEL" =~ ^[A-Z0-9-]+$ ]]; then
    echo "error: label must be uppercase A-Z, 0-9, and dashes only" >&2
    exit 64
fi
if ! [[ "$DAYS" =~ ^[0-9]+$ ]]; then
    echo "error: days must be a positive integer" >&2
    exit 64
fi
if ! [[ "$MAX_DEVICES" =~ ^[0-9]+$ ]]; then
    echo "error: max_devices must be a positive integer" >&2
    exit 64
fi

# Per-day budgets. Anthropic in estimated tokens (the Worker charges a flat
# per-turn estimate); Deepgram/Cartesia in token mints (one mint per session
# thanks to client caching). Roughly 100 voice turns/day at the current
# estimate. Edit per code if needed.
DAILY_INPUT_TOKENS=600000
DAILY_OUTPUT_TOKENS=60000
DAILY_DEEPGRAM=100
DAILY_CARTESIA=100

EXPIRES_AT=$(date -u -d "+${DAYS} days" +%Y-%m-%dT%H:%M:%SZ)

SUFFIX=$(openssl rand -hex 3 | tr 'a-z' 'A-Z')
CODE="${LABEL}-${SUFFIX}"

PAYLOAD=$(cat <<EOF
{
  "daily_input_tokens": ${DAILY_INPUT_TOKENS},
  "daily_output_tokens": ${DAILY_OUTPUT_TOKENS},
  "daily_deepgram_tokens": ${DAILY_DEEPGRAM},
  "daily_cartesia_tokens": ${DAILY_CARTESIA},
  "max_devices": ${MAX_DEVICES},
  "expires_at": "${EXPIRES_AT}",
  "devices_seen": []
}
EOF
)

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
cd "${SCRIPT_DIR}/.."

# wrangler 4 syntax. KV namespace is resolved from wrangler.toml binding name.
# Use npx so this works without a global wrangler install. Pass an explicit
# --local or --remote so wrangler doesn't prompt and default to "no" in a
# non-interactive shell (which silently writes to nothing useful).
REMOTE_FLAG=""
if [[ -z "$LOCAL_FLAG" ]]; then
    REMOTE_FLAG="--remote"
fi
npx wrangler kv key put ${LOCAL_FLAG} ${REMOTE_FLAG} \
    --binding=USAGE_KV \
    "invite:${CODE}" \
    "${PAYLOAD}" >/dev/null

echo "Code:         ${CODE}"
echo "Expires:      ${EXPIRES_AT} (${DAYS} days)"
echo "Max devices:  ${MAX_DEVICES}"
echo "Daily budget: ${DAILY_INPUT_TOKENS} in / ${DAILY_OUTPUT_TOKENS} out tokens, ${DAILY_DEEPGRAM} STT / ${DAILY_CARTESIA} TTS mints"
echo
echo "Send the recipient:"
echo "  Paste this code into Peeky settings: ${CODE}"
