#!/usr/bin/env bash
# RePlAce parameter research: one agent change per round, keep it only if the
# RePlAce score improves without growing the held-out corpus.
# Ctrl-C stops cleanly: kills the agent, drops half-finished edits, runs cargo clean.
set -u
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$0")/../.."
MODEL=${MODEL:-deepseek/deepseek-v4.1-flash}
MAX_EXP=${MAX_EXP:-40}
MAX_SECONDS=${MAX_SECONDS:-28800}
MAX_DISCARDS=${MAX_DISCARDS:-10}
HEARTBEAT=${HEARTBEAT:-60}
AR=scripts/autoresearch-replace

LOG=results-replace.tsv
LOGDIR=logs/autoresearch-replace
NOTE=$PWD/ar-note.txt
FOCUS=${FOCUS:-"tune replace_params.rs: reduce dead space and area, routability first"}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
mkdir -p $LOGDIR
pid=

stamp() { date +%H:%M:%S; }
say() { printf '[%s] %s\n' "$(stamp)" "$*" | tee -a $LOGDIR/loop.log >&2; }
dbg() { printf '[%s]   %s\n' "$(stamp)" "$*" >>$LOGDIR/loop.log; }
dur() { printf '%dm%02ds' $(($1 / 60)) $(($1 % 60)); }
delta() { awk -v a="$1" -v b="$2" 'BEGIN { if (a + 0 == a) printf "%.1f (%+.1f)", a, a - b; else printf "%s", a }'; }
log() { printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >>"$LOG"; }
revert() { git reset -q --hard HEAD && git clean -fdq -- crates; }
finish() {
  say "stopping: $1${base_t:+  | tuning $base_t -> ${best_t:-?}, held-out $base_h -> ${best_h:-?}}"
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
build() {
  cargo build --release -p synth-cli >build.log 2>&1 && return 0
  say "build FAILED: $(grep -m1 '^error' build.log | cut -c1-110)"
  dbg "build log tail:"; tail -n 15 build.log >>$LOGDIR/loop.log
  return 1
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

say "start: model=$MODEL rounds<=$MAX_EXP focus=$FOCUS"
git diff --quiet -- crates || finish "uncommitted changes under crates/, refusing to start" 1
git add $AR && git commit -qsm "NOISSUE - replace params autoresearch harness" >/dev/null 2>&1
[ -f $LOG ] || printf 'round\tcommit\ttuning_score\theldout_score\ttuning_wire\theldout_wire\tstatus\tnote\n' >$LOG
say "baseline: building"
build || finish "baseline build failed" 1
say "baseline: scoring"
best_t=$(total $AR/corpus_tuning.txt tuning); best_h=$(total $AR/corpus_heldout.txt heldout)
best_w=$(wire_total $AR/corpus_tuning.txt tuning); best_hw=$(wire_total $AR/corpus_heldout.txt heldout)
case $best_t in ""|*FAIL*) finish "baseline tuning did not score, see $LOGDIR/tuning.log" 1;; esac
case $best_h in ""|*FAIL*) finish "baseline held-out did not score, see $LOGDIR/heldout.log" 1;; esac
case $best_w in ""|*FAIL*) finish "baseline tuning wire missing, see $LOGDIR/tuning.log" 1;; esac
case $best_hw in ""|*FAIL*) finish "baseline held-out wire missing, see $LOGDIR/heldout.log" 1;; esac
base_t=$best_t; base_h=$best_h; base_w=$best_w; base_hw=$best_hw
say "baseline: tuning $base_t (wire $base_w), held-out $base_h (wire $base_hw)"
log 0 "$(git rev-parse --short HEAD)" "$best_t" "$best_h" "$best_w" "$best_hw" baseline "start"

start=$SECONDS; discards=0
for i in $(seq 1 "$MAX_EXP"); do
  [ $((SECONDS - start)) -lt "$MAX_SECONDS" ] || finish "time limit"
  [ "$discards" -lt "$MAX_DISCARDS" ] || finish "$MAX_DISCARDS discards in a row"
  r="r$i/$MAX_EXP"
  say "$r start    best: tuning $best_t, held-out $best_h  (discards in a row: $discards)"
  rm -f $NOTE
  prompt="$(sed "s|/tmp/ar-note.txt|$NOTE|g" $AR/program.md)

${FOCUS:+Focus of this loop (stay on it unless it is exhausted): $FOCUS
}
Best so far: tuning $best_t, held-out $best_h.
Recent rounds:
$(tail -n 8 $LOG)"

  alog=$LOGDIR/agent-$i.log
  t0=$SECONDS; tick=0
  dbg "agent log: $alog"
  PATH="$GUARD:$PATH" setsid timeout 1800 cmd -p "$prompt" -m $MODEL --yolo --max-turns "${MAX_TURNS:-80}" --no-session >"$alog" 2>&1 &
  pid=$!
  while kill -0 $pid 2>/dev/null; do
    sleep "$HEARTBEAT"
    kill -0 $pid 2>/dev/null || break
    tick=$((tick + 1))
    dbg "agent working $(dur $((SECONDS - t0))), $(wc -c <"$alog") bytes of output"
    [ $((tick % 5)) = 0 ] && say "$r agent still working ($(dur $((SECONDS - t0))))"
  done
  wait $pid; code=$?; pid=
  dbg "agent exit $code; last lines:"; tail -n 5 "$alog" | cut -c1-160 | sed 's/^/    /' >>$LOGDIR/loop.log
  note=$(head -c 160 $NOTE 2>/dev/null | tr '\t\n' '  ')
  dbg "note: ${note:-none}"
  adur=$(dur $((SECONDS - t0)))
  git add -N -- crates 2>/dev/null

  if git diff --quiet -- crates; then
    say "$r NOOP     agent $adur, no edit${note:+  ($note)}"
    log "$i" - - - - - noop "${note:-no change}"; discards=$((discards + 1)); continue
  fi
  files=$(git diff --name-only -- crates | xargs -n1 basename | paste -sd, -)
  say "$r agent    $adur  $(git diff --numstat -- crates | awk '{a += $1; d += $2} END {printf "+%d/-%d", a, d}')  [$files]"
  if git diff --name-only | grep -Evq '^crates/(synth-place|synth-layout|synth-kicad)/src/'; then
    say "$r REJECT   touched files outside the allowed folders"
    revert; log "$i" - - - - - rejected "touched frozen files"; discards=$((discards + 1)); continue
  fi
  if git diff | grep -Eq '^-.*#\[test\]'; then
    say "$r REJECT   removed a test"
    revert; log "$i" - - - - - rejected "removed a test"; discards=$((discards + 1)); continue
  fi
  say "$r building"
  if ! build; then
    revert; log "$i" - - - - - crash "build failed: $note"; discards=$((discards + 1)); continue
  fi

  say "$r scoring  ($(wc -l <$AR/corpus_tuning.txt) tuning + $(wc -l <$AR/corpus_heldout.txt) held-out boards)"
  t=$(total $AR/corpus_tuning.txt tuning "tuning-$i"); h=$(total $AR/corpus_heldout.txt heldout "heldout-$i")
  w_t=$(wire_total $AR/corpus_tuning.txt tuning "tuning-$i"); w_h=$(wire_total $AR/corpus_heldout.txt heldout "heldout-$i")
  failed=$(grep -h '"ok": false' $LOGDIR/tuning-$i.log $LOGDIR/heldout-$i.log | awk '{n = split($1, a, "/"); printf "%s ", a[n]}')
  lognote="$note${failed:+ [failed: $failed]}"
  dt=$(delta "$t" "$best_t"); dh=$(delta "$h" "$best_h")
  if python3 -c "import sys; sys.path.insert(0,'$AR'); import score; sys.exit(0 if score.should_keep(*sys.argv[1:9]) else 1)" "$t" "$h" "$best_t" "$best_h" "$w_t" "$w_h" "$best_w" "$best_hw"; then
    git add -A crates && git commit -qsm "NOISSUE - replace params research round $i: ${lognote:-change}"
    c=$(git rev-parse --short HEAD)
    say "$r KEEP     tuning $dt  held-out $dh  wire $w_t/$w_h  $c  ${note:0:70}"
    log "$i" "$c" "$t" "$h" "$w_t" "$w_h" keep "$lognote"
    best_t=$t; best_h=$h; best_w=$w_t; best_hw=$w_h; discards=0
  else
    revert; discards=$((discards + 1))
    say "$r DISCARD  tuning $dt  held-out $dh  wire $w_t/$w_h${failed:+  failed: $failed}  ${note:0:60}"
    log "$i" - "$t" "$h" "$w_t" "$w_h" discard "$lognote"
  fi
done
finish "reached $MAX_EXP rounds"
