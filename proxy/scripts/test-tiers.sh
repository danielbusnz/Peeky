#!/usr/bin/env bash
# Behavioral test for the two usage tiers, run against a local wrangler dev.
#
# Usage:
#   ./scripts/test-tiers.sh
#
# It starts `wrangler dev --local`, then drives the cartesia token endpoint with
# fresh device ids to prove each tier's daily mint budget is enforced:
#   - trial (no invite code): cap = TRIAL_DAILY_BUDGET.cartesia in constants.ts
#   - recruiter (minted code): cap = daily_cartesia_tokens from mint-code.sh
#
# Why this works without provider secrets: the gate runs before the upstream
# call, so a missing secret only 401s after the request already passed (or was
# 429'd at) the budget check. We only care whether it got past the gate.
#
# Note: usage is recorded via ctx.waitUntil (after the response), so the write
# can lag the next request. The cap isn't enforced to the exact call; we assert
# that the budget admits at least `cap` calls and then 429s within a little
# slack, not that call cap+1 is the first 429.

set -uo pipefail
cd "$(dirname "$0")/.."

PORT=8799
URL="http://localhost:${PORT}"
# Single metered endpoint is enough; it consumes one turn per call.
ENDPOINT="${URL}/v1/cartesia/token"
PASS=0
FAIL=0

uuid() { cat /proc/sys/kernel/random/uuid; }

# POST one metered call, echo the HTTP status. $1 = device id, $2 = invite code
# (optional).
hit() {
    local device="$1" code="${2:-}"
    if [[ -n "$code" ]]; then
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-aegis-device-id: ${device}" -H "x-aegis-invite-code: ${code}"
    else
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-aegis-device-id: ${device}"
    fi
}

check() {
    local label="$1" got="$2" want="$3"
    if [[ "$got" == "$want" ]]; then
        echo "  PASS  ${label} (${got})"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  ${label}: got ${got}, want ${want}"
        FAIL=$((FAIL + 1))
    fi
}

# Drive `cap` calls (expect all admitted, since the budget covers them) then
# keep going until a 429 appears within a little slack, and confirm its body
# carries the expected error code. Slack absorbs the deferred-write lag. $1
# label, $2 cap, $3 expected error, $4 device, $5 invite (optional).
assert_cap() {
    local label="$1" cap="$2" want_err="$3" device="$4" code="${5:-}"
    local slack=3

    local i status admitted=1
    for ((i = 1; i <= cap; i++)); do
        status="$(hit "$device" "$code")"
        [[ "$status" != "429" ]] || { admitted=0; break; }
    done
    check "${label}: ${cap} calls admitted" "$admitted" "1"

    # Within `slack` more calls, one must 429. Capture that 429's body.
    local body="" capped=0
    for ((i = 1; i <= slack; i++)); do
        if [[ -n "$code" ]]; then
            body="$(curl -s -X POST "$ENDPOINT" -H "x-aegis-device-id: ${device}" -H "x-aegis-invite-code: ${code}")"
        else
            body="$(curl -s -X POST "$ENDPOINT" -H "x-aegis-device-id: ${device}")"
        fi
        grep -q "\"${want_err}\"" <<<"$body" && { capped=1; break; }
    done
    if [[ "$capped" == "1" ]]; then
        echo "  PASS  ${label}: capped with ${want_err} within slack"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  ${label}: expected ${want_err} within ${slack} calls, got ${body}"
        FAIL=$((FAIL + 1))
    fi
}

# ── boot a local worker, tear it down on exit ──────────────────────────────
LOG="$(mktemp)"
setsid npx wrangler dev --port "$PORT" --local >"$LOG" 2>&1 &
WD_PGID=$!
# Kill the whole process group (wrangler spawns a detached workerd child that a
# plain kill on the parent leaves behind). TERM, then KILL anything left.
cleanup() {
    kill -TERM -"$WD_PGID" 2>/dev/null
    sleep 0.5
    kill -KILL -"$WD_PGID" 2>/dev/null
}
trap cleanup EXIT

echo "starting wrangler dev on :${PORT} ..."
for _ in $(seq 1 60); do
    grep -q "Ready on" "$LOG" && break
    sleep 1
done
grep -q "Ready on" "$LOG" || { echo "wrangler dev never came up:"; tail -20 "$LOG"; exit 1; }

# Cartesia mint budgets: trial from TRIAL_DAILY_BUDGET.cartesia, recruiter from
# the mint script's DAILY_CARTESIA default.
TRIAL_CAP="$(grep -A6 'TRIAL_DAILY_BUDGET' src/constants.ts | grep -oE 'cartesia: *[0-9_]+' | grep -oE '[0-9]+' | head -1)"
RECRUITER_CAP="$(grep -oE 'DAILY_CARTESIA=[0-9]+' scripts/mint-code.sh | grep -oE '[0-9]+')"

echo
echo "── trial tier (no code, daily cartesia cap=${TRIAL_CAP}) ──"
assert_cap "trial" "$TRIAL_CAP" "trial_exhausted" "$(uuid)"

echo
echo "── recruiter tier (minted code, daily cartesia cap=${RECRUITER_CAP}) ──"
MINT_OUT="$(bash scripts/mint-code.sh TESTTIER 10 5 --local 2>&1)"
CODE="$(grep -E '^Code:' <<<"$MINT_OUT" | awk '{print $2}')"
[[ -n "$CODE" ]] || { echo "mint failed:"; echo "$MINT_OUT"; exit 1; }
echo "  minted ${CODE}"
DEV_A="$(uuid)"
assert_cap "recruiter" "$RECRUITER_CAP" "code_exhausted" "$DEV_A" "$CODE"

echo
echo "── per-device counter (same code, new device) ──"
# A second device under the same code must start fresh, proving the counter is
# keyed per (code, device), not per code. Any non-429 means it got past the gate.
DEV_B_STATUS="$(hit "$(uuid)" "$CODE")"
if [[ "$DEV_B_STATUS" != "429" ]]; then
    echo "  PASS  recruiter: device B admitted (${DEV_B_STATUS})"
    PASS=$((PASS + 1))
else
    echo "  FAIL  recruiter: device B capped (429), counter not per-device"
    FAIL=$((FAIL + 1))
fi

echo
echo "── bad device id rejected ──"
check "missing device id 401" \
    "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$ENDPOINT")" "401"

echo
echo "${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
