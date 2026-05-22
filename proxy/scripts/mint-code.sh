#!/usr/bin/env bash
# Mint an invite code and register it in Cloudflare KV.
#
# Usage:
#   ./scripts/mint-code.sh <label> [days] [max_devices]
#
# Args:
#   label        Human tag baked into the code. Uppercase A-Z and 0-9 only.
#                Example: "RECRUITER-ACME"
#   days         Days until the code expires. Default 30.
#   max_devices  Number of distinct devices this code allows. Default 2.
#
# Caps:
#   Defaults are generous-but-bounded for a recruiter demo. Override by
#   editing the JSON block at the bottom of this script.
#
# Requirements:
#   - wrangler logged in to the right Cloudflare account
#   - The USAGE_KV namespace already exists (see wrangler.toml)
#
# This writes to the REMOTE KV namespace, not the local dev one. Use
# `--local` flag below to mint a code only against local wrangler dev.

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

SUFFIX=$(openssl rand -hex 3 | tr 'a-z' 'A-Z')
CODE="${LABEL}-${SUFFIX}"

# macOS `date -u -v+...` vs GNU `date -u -d ...` split.
if date -u -v+1d >/dev/null 2>&1; then
    EXPIRES_AT=$(date -u -v+"${DAYS}"d +"%Y-%m-%dT%H:%M:%SZ")
else
    EXPIRES_AT=$(date -u -d "+${DAYS} days" +"%Y-%m-%dT%H:%M:%SZ")
fi

PAYLOAD=$(cat <<EOF
{
  "daily_input_tokens": 500000,
  "daily_output_tokens": 100000,
  "daily_deepgram_tokens": 500,
  "daily_cartesia_tokens": 1000,
  "expires_at": "${EXPIRES_AT}",
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
echo "Expires:     ${EXPIRES_AT}"
echo "Max devices: ${MAX_DEVICES}"
echo
echo "Send the recipient:"
echo "  Paste this code into Aegis settings: ${CODE}"
