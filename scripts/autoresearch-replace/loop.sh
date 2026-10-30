#!/usr/bin/env bash
# RePlAce parameter research: one agent change per round, keep it only if the
# RePlAce score improves without growing the held-out corpus.
# Ctrl-C stops cleanly: kills the agent, drops half-finished edits, runs cargo clean.
set -u
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$0")/../.."
MODEL=${MODEL:-opencode-go/deepseek-v4.1-flash}
MAX_EXP=${MAX_EXP:-40}
MAX_SECONDS=${MAX_SECONDS:-28800}
MAX_DISCARDS=${MAX_DISCARDS:-10}
HEARTBEAT=${HEARTBEAT:-60}
AR=scripts/autoresearch-replace
GATE=$AR/gate_boards.py
export AR_PLACE_REPLACE=1

LOG=results-replace.tsv
LOGDIR=logs/autoresearch-replace
VISION_DIR=$LOGDIR/vision
BEST_VISION=$VISION_DIR/best
export AR_VISION_BOARDS=${AR_VISION_BOARDS:-secure_tracker,sensor_logger}
NOTE=$PWD/ar-note.txt
FOCUS=${FOCUS:-"tune replace_params.rs: reduce dead space and area, routability first"}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
mkdir -p "$LOGDIR"
pid=
START_SECONDS=$SECONDS

stamp() { date +%H:%M:%S; }
dur() { printf '%dm%02ds' $(($1 / 60)) $(($1 % 60)); }
elapsed() { dur $((SECONDS - START_SECONDS)); }
say() { printf '[%s +%s] %s\n' "$(stamp)" "$(elapsed)" "$*" | tee -a "$LOGDIR/loop.log" >&2; }
dbg() { printf '[%s +%s]   %s\n' "$(stamp)" "$(elapsed)" "$*" >>"$LOGDIR/loop.log"; }
section() { say "===== $* ====="; }
delta() { awk -v a="$1" -v b="$2" 'BEGIN { if (a + 0 == a) printf "%.1f (%+.1f)", a, a - b; else printf "%s", a }'; }
log() { printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >>"$LOG"; }
revert() { git reset -q --hard HEAD && git clean -fdq -- crates fixtures examples scripts/autoresearch-replace/boards; }
vision_promote() {
  local source=$1
  mkdir -p "$BEST_VISION"
  if [ -d "$source" ]; then
    cp -f "$source"/*.png "$BEST_VISION"/ 2>/dev/null || true
  fi
}
finish() {
  say "STOP: $1"
  say "summary: elapsed $(elapsed)${base_t:+ | tuning $base_t -> ${best_t:-?} | held-out $base_h -> ${best_h:-?}}"
  say "logs: $LOGDIR/loop.log | table: $LOG | per-round: $LOGDIR/"
  say "cleaning cargo target..."
  cargo clean -q
  exit "${2:-0}"
}
interrupted() {
  trap '' INT TERM
  say "Ctrl-C: stopping the agent, dropping half-finished edits, running cargo clean (about a minute)..."
  [ -n "$pid" ] && kill -TERM -- "-$pid" 2>/dev/null
  pkill -P $$ 2>/dev/null
  revert
  finish "interrupted" 130
}
trap interrupted INT TERM

total() {
  local file=${3:-$2}
  dbg "scoring $2 corpus ($(wc -l <"$1") boards)"
  python3 $AR/score.py $(cat "$1") >"$LOGDIR/$file.log" 2>&1
  awk '/^SCORE/ {print $2}' "$LOGDIR/$file.log"
}
wire_total() {
  local file=${3:-$2}
  awk '/^WIRE/ {print $2}' "$LOGDIR/$file.log"
}
gates_total() {
  local file=${3:-$2}
  awk '/^GATES/ {print $2}' "$LOGDIR/$file.log"
}
build() {
  local b0=$SECONDS
  if cargo build --release -p synth-cli >build.log 2>&1; then
    say "build ok in $(dur $((SECONDS - b0)))"
    return 0
  fi
  say "build FAILED after $(dur $((SECONDS - b0))): $(grep -m1 '^error' build.log | cut -c1-140)"
  dbg "build log tail:"; tail -n 25 build.log >>"$LOGDIR/loop.log"
  return 1
}
board_report() {
  local file=$1 label=$2
  python3 - "$file" "$label" <<'PY' | tee -a "$LOGDIR/loop.log" >&2
import json, sys

path, label = sys.argv[1], sys.argv[2]


def num(value):
    return 0 if value is None else value


print(f"  {label}:")
print(f"    {'board':<34} {'score':>9} {'dead':>7} {'area':>8} {'wire':>8} {'open':>5} {'drc':>4}")
for line in open(path):
    line = line.strip()
    if not line or line.startswith(("SCORE", "WIRE")):
        continue
    parts = line.split(" ", 1)
    if len(parts) != 2:
        continue
    name, blob = parts
    try:
        data = json.loads(blob)
    except ValueError:
        continue
    board = name.rsplit("/", 1)[-1].replace(".synth", "")
    if not data.get("ok"):
        print(f"    {board:<34} FAIL {str(data.get('why', ''))[:90]}")
        continue
    print(
        f"    {board:<30} {num(data.get('score')):>8.1f} {num(data.get('dead_mm2')):>6.0f} "
        f"{num(data.get('area_mm2')):>7.1f} {num(data.get('wire_length_mm')):>7.1f} "
        f"{num(data.get('fitness_score')):>5.1f} {num(data.get('fitness_gate_failures')):>4} "
        f"{num(data.get('support_mean_mm')):>5.2f} {num(data.get('mating_inset_mm')):>5.2f}"
    )
PY
}

GUARD=$(mktemp -d)
REAL_CARGO=$(command -v cargo)
cat >"$GUARD/cargo" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do
  case "\$a" in
    clippy|test|--workspace|--all-targets|--all)
      echo "cargo \$a is disabled for the agent: the driver builds and scores. Use: cargo check -p <crate>" >&2
      exit 2;;
  esac
done
exec $REAL_CARGO "\$@"
EOF
chmod +x "$GUARD/cargo"

say "start: experiment=$(basename "$PWD") model=$MODEL rounds<=$MAX_EXP discards<=$MAX_DISCARDS budget=${MAX_SECONDS}s"
say "corpus: tuning $(wc -l <"$AR/corpus_tuning.txt") boards, held-out $(wc -l <"$AR/corpus_heldout.txt") boards"
say "focus: $FOCUS"
say "table: $LOG | agent: $AR | logs: $LOGDIR"
git diff --quiet -- crates || finish "uncommitted changes under crates/, refusing to start" 1
git add $AR && git commit -qsm "NOISSUE - replace params autoresearch harness" >/dev/null 2>&1
if [ -f "$LOG" ] && [ "$(head -1 "$LOG" | awk -F'\t' '{print NF}')" != "10" ]; then
  mv "$LOG" "$LOG.old.$(date +%s)"
fi
[ -f $LOG ] || printf 'round\tcommit\ttuning_score\theldout_score\ttuning_wire\theldout_wire\ttuning_gates\theldout_gates\tstatus\tnote\n' >$LOG

section "baseline"
say "baseline: building"
build || finish "baseline build failed" 1
say "baseline: scoring ($(wc -l <"$AR/corpus_tuning.txt") tuning + $(wc -l <"$AR/corpus_heldout.txt") held-out boards)"
export AR_VISION_DIR="$VISION_DIR/baseline"
best_t=$(total $AR/corpus_tuning.txt tuning); best_h=$(total $AR/corpus_heldout.txt heldout)
best_w=$(wire_total $AR/corpus_tuning.txt tuning); best_hw=$(wire_total $AR/corpus_heldout.txt heldout)
base_gates_t=$(gates_total $AR/corpus_tuning.txt tuning); base_gates_h=$(gates_total $AR/corpus_heldout.txt heldout)
case $base_gates_t in ""|*FAIL*) finish "baseline tuning gates missing, see $LOGDIR/tuning.log" 1;; esac
case $base_gates_h in ""|*FAIL*) finish "baseline held-out gates missing, see $LOGDIR/heldout.log" 1;; esac
case $best_t in ""|*FAIL*) finish "baseline tuning did not score, see $LOGDIR/tuning.log" 1;; esac
case $best_h in ""|*FAIL*) finish "baseline held-out did not score, see $LOGDIR/heldout.log" 1;; esac
case $best_w in ""|*FAIL*) finish "baseline tuning wire missing, see $LOGDIR/tuning.log" 1;; esac
case $best_hw in ""|*FAIL*) finish "baseline held-out wire missing, see $LOGDIR/heldout.log" 1;; esac
base_t=$best_t; base_h=$best_h; base_w=$best_w; base_hw=$best_hw
best_gates_t=$base_gates_t; best_gates_h=$base_gates_h
board_report "$LOGDIR/tuning.log" "baseline tuning"
board_report "$LOGDIR/heldout.log" "baseline held-out"
vision_promote "$AR_VISION_DIR"
say "baseline: tuning $base_t (wire $base_w), held-out $base_h (wire $base_hw), gates $base_gates_t/$base_gates_h, elapsed $(elapsed)"
log 0 "$(git rev-parse --short HEAD)" "$best_t" "$best_h" "$best_w" "$best_hw" "$base_gates_t" "$base_gates_h" baseline "start"

start=$SECONDS; discards=0
for i in $(seq 1 "$MAX_EXP"); do
  [ $((SECONDS - start)) -lt "$MAX_SECONDS" ] || finish "time limit"
  [ "$discards" -lt "$MAX_DISCARDS" ] || finish "$MAX_DISCARDS discards in a row"
  r="r$i/$MAX_EXP"
  section "$r"
  say "$r start: elapsed $(elapsed), budget left $(dur $((MAX_SECONDS - (SECONDS - start)))), best tuning $best_t / held-out $best_h, discards $discards"
  rm -f $NOTE
  vision_args=()
  for image in "$BEST_VISION"/*.png; do
    [ -f "$image" ] && vision_args+=(--file "$image")
  done
  vision_note=""
  if [ ${#vision_args[@]} -gt 0 ]; then
    vision_note="Attached current best placement image(s): $(printf '%s ' "$BEST_VISION"/*.png | sed 's|.*/||'). Use them to see dead space; the numeric table follows."
  fi
  prompt="$(sed "s|/tmp/ar-note.txt|$NOTE|g" $AR/program.md)

${FOCUS:+Focus of this loop (stay on it unless it is exhausted): $FOCUS
}
Best so far: tuning $best_t, held-out $best_h, gates $best_gates_t/$best_gates_h.
${vision_note:+$vision_note
}Recent rounds:
$(tail -n 8 $LOG)"

  alog=$LOGDIR/agent-$i.log
  t0=$SECONDS; tick=0
  dbg "agent log: $alog"
  PATH="$GUARD:$PATH" setsid timeout 1800 opencode run --auto --agent build --model "$MODEL" "${vision_args[@]}" "$prompt" >"$alog" 2>&1 &
  pid=$!
  while kill -0 $pid 2>/dev/null; do
    sleep "$HEARTBEAT"
    kill -0 $pid 2>/dev/null || break
    tick=$((tick + 1))
    dbg "agent working $(dur $((SECONDS - t0))), $(wc -c <"$alog") bytes of output"
    [ $((tick % 5)) = 0 ] && say "$r agent still working ($(dur $((SECONDS - t0))))"
  done
  wait $pid; code=$?; pid=
  adur=$(dur $((SECONDS - t0)))
  dbg "agent exit $code; last lines:"; tail -n 8 "$alog" | cut -c1-160 | sed 's/^/    /' >>"$LOGDIR/loop.log"
  if [ "$code" -ne 0 ]; then
    say "$r agent exit $code after $adur (log: $alog)"
    tail -n 3 "$alog" | cut -c1-180 | sed 's/^/    /' | tee -a "$LOGDIR/loop.log" >&2
  else
    say "$r agent done in $adur"
  fi
  note=$(head -c 160 $NOTE 2>/dev/null | tr '\t\n' '  ')
  dbg "note: ${note:-none}"
  git add -N -- crates 2>/dev/null

  if git diff --quiet -- crates; then
    say "$r NOOP agent $adur, no edit${note:+ ($note)}"
    log "$i" - - - - - - - noop "${note:-no change}"; discards=$((discards + 1)); continue
  fi
  files=$(git diff --name-only -- crates | xargs -n1 basename | paste -sd, -)
  say "$r agent $adur $(git diff --numstat -- crates | awk '{a += $1; d += $2} END {printf "+%d/-%d", a, d}') [$files]"
  if git diff --name-only | grep -Evq '^crates/synth-place/src/(replace_params\.rs|replace\.rs|lib\.rs)$'; then
    say "$r REJECT touched a frozen file (only replace_params.rs, replace.rs and the legalizer in lib.rs are allowed)"
    revert; log "$i" - - - - - - - rejected "touched frozen files"; discards=$((discards + 1)); continue
  fi
  if git status --porcelain | grep -Eq '^\?\? (fixtures|examples|scripts/autoresearch-replace/boards)/'; then
    say "$r REJECT created an untracked design sidecar"
    revert; log "$i" - - - - - - - rejected "untracked design file"; discards=$((discards + 1)); continue
  fi
  if git diff | grep -Eq '^-.*#\[test\]'; then
    say "$r REJECT removed a test"
    revert; log "$i" - - - - - - - rejected "removed a test"; discards=$((discards + 1)); continue
  fi
  say "$r building"
  if ! build; then
    revert; log "$i" - - - - - - - crash "build failed: $note"; discards=$((discards + 1)); continue
  fi

  say "$r scoring ($(wc -l <$AR/corpus_tuning.txt) tuning + $(wc -l <$AR/corpus_heldout.txt) held-out boards)"
  export AR_VISION_DIR="$VISION_DIR/round-$i"
  t=$(total $AR/corpus_tuning.txt tuning "tuning-$i"); h=$(total $AR/corpus_heldout.txt heldout "heldout-$i")
  w_t=$(wire_total $AR/corpus_tuning.txt tuning "tuning-$i"); w_h=$(wire_total $AR/corpus_heldout.txt heldout "heldout-$i")
  gt=$(gates_total $AR/corpus_tuning.txt tuning "tuning-$i"); gh=$(gates_total $AR/corpus_heldout.txt heldout "heldout-$i")
  board_report "$LOGDIR/tuning-$i.log" "$r tuning"
  board_report "$LOGDIR/heldout-$i.log" "$r held-out"
  gate_msg=$(python3 "$GATE" "$LOGDIR/tuning-$i.log" "$LOGDIR/tuning.log" "$LOGDIR/heldout-$i.log" "$LOGDIR/heldout.log" 2>&1)
  if [ $? -ne 0 ]; then
    say "$r REJECT per-board gate:"
    printf '%s
' "$gate_msg" | tee -a "$LOGDIR/loop.log" >&2
    revert; log "$i" - "$t" "$h" "$w_t" "$w_h" "$gt" "$gh" rejected "per-board gate"
    discards=$((discards + 1)); continue
  fi
  failed=$(grep -h '"ok": false' $LOGDIR/tuning-$i.log $LOGDIR/heldout-$i.log | awk '{n = split($1, a, "/"); printf "%s ", a[n]}')
  lognote="$note${failed:+ [failed: $failed]}"
  dt=$(delta "$t" "$best_t"); dh=$(delta "$h" "$best_h")
  if python3 -c "import sys; sys.path.insert(0,'$AR'); import score; sys.exit(0 if score.should_keep(*sys.argv[1:13]) else 1)" "$t" "$h" "$best_t" "$best_h" "$w_t" "$w_h" "$base_w" "$base_hw" "$gt" "$gh" "$best_gates_t" "$best_gates_h"; then
    git add -A crates && git commit -qsm "NOISSUE - replace params research round $i: ${lognote:-change}"
    c=$(git rev-parse --short HEAD)
    vision_promote "$AR_VISION_DIR"
    say "$r KEEP tuning $dt | held-out $dh | wire $w_t/$w_h | gates $gt/$gh | commit $c | elapsed $(elapsed) | ${note:0:70}"
    log "$i" "$c" "$t" "$h" "$w_t" "$w_h" "$gt" "$gh" keep "$lognote"
    best_t=$t; best_h=$h; best_w=$w_t; best_hw=$w_h; best_gates_t=$gt; best_gates_h=$gh; discards=0
    cp "$LOGDIR/tuning-$i.log" "$LOGDIR/tuning.log"
    cp "$LOGDIR/heldout-$i.log" "$LOGDIR/heldout.log"
  else
    revert; discards=$((discards + 1))
    say "$r DISCARD tuning $dt | held-out $dh | wire $w_t/$w_h | gates $gt/$gh${failed:+ | failed: $failed} | elapsed $(elapsed) | ${note:0:60}"
    log "$i" - "$t" "$h" "$w_t" "$w_h" "$gt" "$gh" discard "$lognote"
  fi
done

section "test"
say "test: scoring the unseen corpus once"
test_score=$(total $AR/corpus_test.txt test)
test_wire=$(wire_total $AR/corpus_test.txt test)
test_gates=$(gates_total $AR/corpus_test.txt test)
say "TEST score $test_score wire $test_wire gates $test_gates"
say "fitness debt: tuning $best_gates_t / held-out $best_gates_h"
if [ "${REQUIRE_CLEAN:-0}" = "1" ] && [ $((best_gates_t + best_gates_h)) -gt 0 ]; then
  finish "fitness debt remains" 1
fi
finish "reached $MAX_EXP rounds"
