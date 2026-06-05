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

# Mint an HS256 session JWT the way auth/jwt.ts does, signed with the local dev
# secret from .dev.vars so the account-tier path verifies it. $1 = user id.
JWT_SECRET="$(grep -E '^JWT_SECRET=' .dev.vars | cut -d= -f2-)"
mint_jwt() {
    local sub="$1"
    node -e '
        const c = require("crypto");
        const b = (o) => Buffer.from(JSON.stringify(o)).toString("base64url");
        const [secret, sub] = process.argv.slice(1);
        const now = Math.floor(Date.now() / 1000);
        const h = b({ alg: "HS256", typ: "JWT" });
        const p = b({ sub, email: null, tier: "free", iat: now, exp: now + 3600 });
        const sig = c.createHmac("sha256", secret).update(h + "." + p).digest("base64url");
        console.log(`${h}.${p}.${sig}`);
    ' "$JWT_SECRET" "$sub"
}

# POST one metered call, echo the HTTP status. $1 = device id, $2 = invite code
# (optional).
hit() {
    local device="$1" code="${2:-}"
    if [[ -n "$code" ]]; then
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-peeky-device-id: ${device}" -H "x-peeky-invite-code: ${code}"
    else
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-peeky-device-id: ${device}"
    fi
}

# POST one metered call as a signed-in account. $1 = device id, $2 = bearer JWT.
hit_jwt() {
    curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
        -H "x-peeky-device-id: ${1}" -H "authorization: Bearer ${2}"
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
# label, $2 cap, $3 expected error, $4 device, $5 invite (optional), $6 bearer
# JWT (optional; mutually exclusive with an invite code).
assert_cap() {
    local label="$1" cap="$2" want_err="$3" device="$4" code="${5:-}" bearer="${6:-}"
    local slack=3

    local i status admitted=1
    for ((i = 1; i <= cap; i++)); do
        if [[ -n "$bearer" ]]; then
            status="$(hit_jwt "$device" "$bearer")"
        else
            status="$(hit "$device" "$code")"
        fi
        [[ "$status" != "429" ]] || { admitted=0; break; }
    done
    check "${label}: ${cap} calls admitted" "$admitted" "1"

    # Within `slack` more calls, one must 429. Capture that 429's body.
    local body="" capped=0
    for ((i = 1; i <= slack; i++)); do
        if [[ -n "$bearer" ]]; then
            body="$(curl -s -X POST "$ENDPOINT" -H "x-peeky-device-id: ${device}" -H "authorization: Bearer ${bearer}")"
        elif [[ -n "$code" ]]; then
            body="$(curl -s -X POST "$ENDPOINT" -H "x-peeky-device-id: ${device}" -H "x-peeky-invite-code: ${code}")"
        else
            body="$(curl -s -X POST "$ENDPOINT" -H "x-peeky-device-id: ${device}")"
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

# Cartesia mint budgets: trial and account from constants.ts, recruiter from
# the mint script's DAILY_CARTESIA default.
TRIAL_CAP="$(grep -A6 'TRIAL_DAILY_BUDGET' src/constants.ts | grep -oE 'cartesia: *[0-9_]+' | grep -oE '[0-9]+' | head -1)"
ACCOUNT_CAP="$(grep -A6 'ACCOUNT_DAILY_BUDGET' src/constants.ts | grep -oE 'cartesia: *[0-9_]+' | grep -oE '[0-9]+' | head -1)"
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
echo "── account tier (session JWT, daily cartesia cap=${ACCOUNT_CAP}) ──"
ACCT_USER="$(uuid)"
ACCT_JWT="$(mint_jwt "$ACCT_USER")"
[[ -n "$ACCT_JWT" ]] || { echo "jwt mint failed"; exit 1; }
assert_cap "account" "$ACCOUNT_CAP" "account_exhausted" "$(uuid)" "" "$ACCT_JWT"

echo
echo "── per-user counter (same JWT, new device) ──"
# A second device using the same account must stay capped, proving the counter
# is keyed per user id, not per device: usage follows the user across machines.
DEV_C_STATUS="$(hit_jwt "$(uuid)" "$ACCT_JWT")"
if [[ "$DEV_C_STATUS" == "429" ]]; then
    echo "  PASS  account: new device still capped (per-user counter)"
    PASS=$((PASS + 1))
else
    echo "  FAIL  account: new device admitted (${DEV_C_STATUS}), counter not per-user"
    FAIL=$((FAIL + 1))
fi

echo
echo "── bad device id rejected ──"
check "missing device id 401" \
    "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$ENDPOINT")" "401"

echo
echo "${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
