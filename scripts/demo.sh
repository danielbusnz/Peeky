#!/usr/bin/env bash
# Demo launch preset. NOT for regular use: regular runs are plain
# `cargo run --release -p peeky` (overlay at 1.0x, hosted proxy).
#
# What this preset changes, and why:
#   PEEKY_CURSOR_SCALE=2.4   overlay (cursor, soundwave, spinner) reads
#                            clearly after footage is shrunk into a video
#                            frame on the landing page
#   PEEKY_*_DIRECT=1         providers called directly with the keys in
#                            .env, so a filming session can't hit the
#                            hosted proxy's daily caps mid-take
#
# Requires ANTHROPIC_API_KEY, DEEPGRAM_API_KEY, CARTESIA_API_KEY in the
# repo-root .env.
set -euo pipefail
cd "$(dirname "$0")/.."

PEEKY_CURSOR_SCALE=2.4 \
PEEKY_ANTHROPIC_DIRECT=1 \
PEEKY_DEEPGRAM_DIRECT=1 \
PEEKY_CARTESIA_DIRECT=1 \
exec cargo run --release -p peeky
