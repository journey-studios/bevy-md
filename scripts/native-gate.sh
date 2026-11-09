#!/usr/bin/env bash
# The native gate: the `native_gate` example (a real window) over documents of
# 200, 2000 and 100000 blocks, ROUNDS times each, one process per run, and a
# summary of the raw samples (`scripts/native-gate-summary.py`).
#
#   scripts/native-gate.sh
#
# Settings (environment): ROUNDS (3), BLOCKS ("200 2000 100000"), PRESENT
# (vsync or novsync, vsync), OUT (where the reports go, default
# /tmp/bevymd-native-gate/<timestamp>), SHOTS (1 to also save screenshots of
# the first round), GATE_BIN (a release build of the example to run instead of
# building one), MAX_LOAD (refuse to run above this 1-minute load average;
# unset: run and record it), RUN_TIMEOUT (seconds before a run is killed, 240).
#
# It refuses to run with the screen locked (a locked or asleep display does
# not present frames, and the numbers would be of nothing), and never touches
# a process other than the ones it starts. Run it while nothing else needs
# the machine: the window takes the foreground and the numbers are wall
# times. The load average of every run is recorded next to its report.
set -euo pipefail

cd "$(dirname "$0")/.."

ROUNDS=${ROUNDS:-3}
BLOCKS=${BLOCKS:-"200 2000 100000"}
PRESENT=${PRESENT:-vsync}
OUT=${OUT:-/tmp/bevymd-native-gate/$(date +%Y%m%d-%H%M%S)}
RUN_TIMEOUT=${RUN_TIMEOUT:-240}
# GATE_BIN: a binary already built (release, example native_gate) to use
# instead of building one.
BUILT=${GATE_BIN:-target/release/examples/native_gate}

locked=$(ioreg -n Root -d1 -a 2>/dev/null | plutil -extract IOConsoleUsers.0.CGSSessionScreenIsLocked raw - 2>/dev/null || true)
if [ "$locked" = "true" ]; then
  echo "The screen is locked: the gate does not run on a locked display." >&2
  exit 2
fi

if [ -z "${GATE_BIN:-}" ]; then
  cargo build --release -p bevy_md_editor --example native_gate
fi
mkdir -p "$OUT"
# The machine's build cache cleaner can remove files of target/ while the gate
# runs (it did, between two runs), so the runs use a copy of the binary.
BIN="$OUT/native_gate"
cp "$BUILT" "$BIN"

{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "machine: $(sysctl -n hw.model) / $(sysctl -n machdep.cpu.brand_string) / $(sysctl -n hw.ncpu) cores"
  echo "os: $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -r))"
  echo "power: $(pmset -g batt | head -1)"
  echo "rustc: $(rustc --version)"
  echo "profile: release (lto fat, codegen-units 1)"
  echo "present: $PRESENT"
  echo "rounds: $ROUNDS"
  echo "blocks: $BLOCKS"
  echo "load at start: $(uptime)"
} > "$OUT/config.txt"

round=1
while [ "$round" -le "$ROUNDS" ]; do
  # Alternate the order between rounds so a drift in the machine does not
  # always land on the same size.
  if [ $((round % 2)) -eq 0 ]; then
    order=$(echo "$BLOCKS" | tr ' ' '\n' | sort -rn | tr '\n' ' ')
  else
    order=$BLOCKS
  fi
  for blocks in $order; do
    if [ -n "${MAX_LOAD:-}" ]; then
      load=$(sysctl -n vm.loadavg | awk '{print $2}')
      if awk "BEGIN {exit !($load > $MAX_LOAD)}"; then
        echo "Load $load is above MAX_LOAD=$MAX_LOAD; not running." >&2
        exit 3
      fi
    fi
    report="$OUT/blocks-$blocks-round-$round.json"
    echo "load before: $(uptime | sed 's/.*load averages*: //')" > "$OUT/blocks-$blocks-round-$round.load"
    shots=""
    if [ "${SHOTS:-0}" = "1" ] && [ "$round" -eq 1 ]; then
      shots="$OUT"
    fi
    echo "== $blocks blocks, round $round"
    GATE_BLOCKS=$blocks GATE_OUT=$report GATE_PRESENT=$PRESENT GATE_SHOTS=$shots \
      "$BIN" > "$OUT/blocks-$blocks-round-$round.log" 2>&1 &
    pid=$!
    waited=0
    while kill -0 "$pid" 2>/dev/null; do
      if [ "$waited" -ge "$RUN_TIMEOUT" ]; then
        echo "run exceeded ${RUN_TIMEOUT}s, stopping our own process $pid" >&2
        kill "$pid" 2>/dev/null || true
        break
      fi
      sleep 1
      waited=$((waited + 1))
    done
    wait "$pid" 2>/dev/null || true
    echo "load after: $(uptime | sed 's/.*load averages*: //')" >> "$OUT/blocks-$blocks-round-$round.load"
    if [ ! -s "$report" ]; then
      echo "no report for $blocks blocks, round $round (see $OUT/blocks-$blocks-round-$round.log)" >&2
    fi
  done
  round=$((round + 1))
done

python3 scripts/native-gate-summary.py "$OUT"
echo "reports in $OUT"
