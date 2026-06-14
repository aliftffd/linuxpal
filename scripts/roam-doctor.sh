#!/usr/bin/env bash
# roam-doctor — diagnose LinuxPal dual-screen roaming without live-watching.
#
# Shows monitor geometry + the reachable target-x range the pet should respect,
# then scans the log for crosses and wall-edge repicks and flags edge-bonking
# (many wall repicks clustered at the same x — the "keeps crashing into it" bug).
#
# Usage:
#   ./scripts/roam-doctor.sh                 # one-shot report from /tmp/linuxpal.log
#   ./scripts/roam-doctor.sh /path/to.log    # custom log
#   ./scripts/roam-doctor.sh --watch         # refresh every 2s
#
# Needs the pet running with RUST_LOG=info so cross/wall lines are emitted.
set -u

SPRITE_W=520       # must match SPRITE_W in src/main.rs
BONK_THRESHOLD=3   # >= this many wall repicks within 60px = edge-bonking

LOG=/tmp/linuxpal.log
WATCH=0
for a in "$@"; do
  case "$a" in
    --watch) WATCH=1 ;;
    *) LOG="$a" ;;
  esac
done

report() {
  echo "== monitors =="
  local mons=""
  command -v hyprctl >/dev/null 2>&1 && mons=$(hyprctl monitors -j 2>/dev/null)
  if [ -n "$mons" ]; then
    # program from heredoc (stdin); data via env so the pipe isn't consumed
    SPRITE_W="$SPRITE_W" MONS="$mons" python3 <<'PY'
import json, os
sw = int(os.environ['SPRITE_W'])
mons = json.loads(os.environ['MONS'])
xs = [(m['x'], m['x'] + m['width'], m['name']) for m in mons]
for x0, x1, name in sorted(xs):
    print(f"  {name:12s} x=[{x0},{x1})  w={x1-x0}")
min_x = min(x for x, _, _ in xs)
max_x = max(x1 for _, x1, _ in xs)
print(f"  global x = [{min_x},{max_x})")
print(f"  reachable target_x = [{min_x},{max_x-sw}]  (sprite right edge stays <= {max_x})")
print(f"  -> a wall repick with x > {max_x-sw} = target leaked past the outer edge (BUG)")
PY
  else
    echo "  (hyprctl unavailable)"
  fi

  echo
  echo "== log: $LOG =="
  if [ ! -f "$LOG" ]; then
    echo "  no log file. Run the pet with: RUST_LOG=info ... linuxpal > $LOG 2>&1"
    return
  fi

  local crosses walls
  crosses=$(grep -c "cross →" "$LOG" 2>/dev/null); crosses=${crosses:-0}
  walls=$(grep -c "wall edge" "$LOG" 2>/dev/null); walls=${walls:-0}
  echo "  crosses: $crosses    wall-edge repicks: $walls"

  if [ "$walls" -gt 0 ]; then
    echo
    echo "  last wall-edge repick positions:"
    grep "wall edge" "$LOG" | tail -8 | sed 's/^/    /'

    echo
    echo "  edge-bonk check (cluster of repicks within 60px):"
    grep "wall edge" "$LOG" | grep -oE '\([0-9-]+,' | tr -d '(,' | \
      awk -v thr="$BONK_THRESHOLD" '
        { x=$1; b=int(x/60); cnt[b]++; if(x>mx[b])mx[b]=x; if(mn[b]==""||x<mn[b])mn[b]=x }
        END {
          flagged=0
          for (b in cnt) if (cnt[b]>=thr) {
            printf "    BONK: %d repicks near x=%d..%d\n", cnt[b], mn[b], mx[b]; flagged=1
          }
          if (!flagged) print "    ok — no clustered bonking"
        }'
  fi
}

if [ "$WATCH" = 1 ]; then
  while true; do clear; report; sleep 2; done
else
  report
fi
