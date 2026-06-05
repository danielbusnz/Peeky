#!/usr/bin/env bash
# /daniel — a little resume-card easter egg. Zero deps; colors degrade gracefully.
o=$'\033[38;5;208m'   # orange
d=$'\033[2m'          # dim
b=$'\033[1m'          # bold
r=$'\033[0m'          # reset

W=64
rule() { printf '%*s' "$W" '' | tr ' ' '─'; }
top="${o}╭$(rule)╮${r}"
mid="${o}├$(rule)┤${r}"
bot="${o}╰$(rule)╯${r}"

cat <<EOF

$top
${o}│${r}  ${b}DANIEL BROOKS${r}
${o}│${r}  ${d}builder · self-taught · ships in Rust, TypeScript & Python${r}
$mid
   ${o}◆${r} ${b}Peeky${r} — AI cursor that runs your computer for you
   ${o}◆${r} ${b}Routelet${r} — on-device ML classifier, routes intents in ms
   ${o}◆${r} ${b}HiveNet${r} — full-stack social network with a real-time feed
$mid
${o}│${r}  ${d}github.com/danielbusnz-lgtm   ·   x.com/rackSpreader1${r}
$bot

EOF
