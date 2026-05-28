#!/usr/bin/env bash
# Mint an invite code and register it in Cloudflare KV.
#
# Usage:
#   ./scripts/mint-code.sh <label> [uses] [max_devices] [--local]
#
# Args:
#   label        Human tag baked into the code. Uppercase A-Z and 0-9 only.
#                Example: "RECRUITER-ACME"
#   uses         Lifetime voice queries this code grants. Default 10. Stored as
#                turns_cap = uses * 3 (each query bills STT + Claude + TTS).
#   max_devices  Number of distinct devices this code allows. Default 2.
#
# Codes do not expire. They run until their lifetime uses are spent, using the
# same per-device counter as the free trial, just a higher cap.
#
# Requirements:
#   - wrangler logged in to the right Cloudflare account
#   - The USAGE_KV namespace already exists (see wrangler.toml)
#
# This writes to the REMOTE KV namespace, not the local dev one. Pass --local
# to mint a code only against local wrangler dev.

set -euo pipefail

if [[ $# -lt 1 || $# -gt 4 ]]; then
    echo "usage: $0 <label> [uses] [max_devices] [--local]" >&2
    exit 64
fi

LABEL="${1^^}"
USES="${2:-10}"
MAX_DEVICES="${3:-2}"
LOCAL_FLAG=""
for arg in "$@"; do
    if [[ "$arg" == "--local" ]]; then LOCAL_FLAG="--local"; fi
done

if ! [[ "$LABEL" =~ ^[A-Z0-9-]+$ ]]; then
    echo "error: label must be uppercase A-Z, 0-9, and dashes only" >&2
    exit 64
fi
if ! [[ "$USES" =~ ^[0-9]+$ ]]; then
    echo "error: uses must be a positive integer" >&2
    exit 64
fi
if ! [[ "$MAX_DEVICES" =~ ^[0-9]+$ ]]; then
    echo "error: max_devices must be a positive integer" >&2
    exit 64
fi

# Each voice query bills three calls (STT, Claude, TTS), so the call cap is
# uses * 3. Mirrors the TRIAL_TURNS_CAP convention in wrangler.toml.
TURNS_CAP=$((USES * 3))

SUFFIX=$(openssl rand -hex 3 | tr 'a-z' 'A-Z')
CODE="${LABEL}-${SUFFIX}"

PAYLOAD=$(cat <<EOF
{
  "turns_cap": ${TURNS_CAP},
  "max_devices": ${MAX_DEVICES},
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

echo "Code:        ${CODE}"
echo "Uses:        ${USES} (${TURNS_CAP} calls)"
echo "Max devices: ${MAX_DEVICES}"
echo
echo "Send the recipient:"
echo "  Paste this code into Aegis settings: ${CODE}"
