# RePlAce parameter research

Goal: tune the RePlAce equations so the placement is denser (lower
`dead_mm2` and `area_mm2`) without losing routing. The scorer is frozen; it
exports with `SYNTH_PLACE_REPLACE=1`, so every round tests the RePlAce path,
not the CEM default.

The score is

    dead_mm2 + 0.25 * area_mm2 + 200 * connector_violations
            + 50 * label_overlaps + 100 * labels_off_board

summed over the tuning corpus; lower is better. Every board must export with
0 open nets, 0 KiCad DRC errors and 0 unconnected pads. A keep also requires a
tuning drop of at least one 1 mm dead cell (or 0.5 %, whichever is larger),
routed wire within +2 % of the baseline, and no per-board rise in fitness
failures, wire or support distance. `irrigation_controller` is a test board
scored once at the end; never tune against it.

## What you do each round

1. Read `crates/synth-place/src/replace_params.rs` and the recent results.
2. Change **one** parameter (or one tight group of related parameters) that
   should reduce dead space or area without increasing unrouted nets/wire.
   Prefer the params file. A structural change in `replace.rs` or the legalizer
   is allowed only with a note explaining why a parameter cannot express it.
3. Write one line to the note path in the prompt.
4. Stop. Do not build, score, commit or revert. The driver does all of that.

Budget: at most 12 tool calls, then edit. A no-edit round is wasted. Past
rounds that failed a board name it in the results ("failed:"); fix that cause.

## Allowed

- `crates/synth-place/src/replace_params.rs` (preferred).
- Limited structural changes in `crates/synth-place/src/replace.rs` and the
  legalizer in `crates/synth-place/src/lib.rs`.
- `cargo check -p synth-place`; other cargo commands are blocked.

## Forbidden

- The scorer, corpus, DRC rules, routers and sidecars are frozen.
- Removing or weakening a test, assertion or DRC/clearance rule.
- Randomness, timing or anything that makes output differ between runs.
- New dependencies.
- Moving a connector off its declared edge.

## Knobs and what they do

- `density_model`: `Bin` is the stable model; `Poisson` is experimental.
- `bin_count`: density grid resolution. Too coarse hides congestion; too fine
  makes a spiky force.
- `stages`, `iterations_per_stage`: more search vs runtime.
- `gamma_start_mm`/`gamma_end_mm`/`gamma_decay`: WA smoothing schedule. Large
  gamma early is smooth; small gamma late sharpens the wirelength.
- `lambda_start`/`lambda_growth`: density weight. Too low leaves parts
  clustered; too high spreads them and can grow wire.
- `step_mm`/`max_step_mm`: Nesterov step and per-iteration clamp.
- `net_weight`: wirelength weight.
- `legalizer_gap_mm`: extra free-part clearance in the legalizer. Zero is the
  current baseline; nonzero can preserve routing channels but grows area.

## Ideas worth trying

- Raise `lambda_start` until dead space drops; back off if wire grows.
- Try a slower gamma decay or a smaller `gamma_end_mm` to sharpen the final
  wirelength.
- Sweep `bin_count` (32, 48, 64, 96) with the same lambda.
- Increase `stages` with more iterations only if the score is still moving.
- A small `legalizer_gap_mm` (0.25-0.5) may recover unrouted nets on a dense
  board; do not exceed the area/wire gates.
- Compare `Poisson` only if you are also fixing its self-force; otherwise keep
  `Bin`.

## Electrical fitness and vision

The scorer writes `*.fitness.json` (PR #400) and prints `fit` and `gate` in the
per-board table. Fitness is a **gate**, not a score term: the 0-100 composite
is never added to the objective. `gate_boards.py` compares every board with the
last kept run; a rise in failures, routed wire or support distance rejects the
round even if another board improves. The full report (support distance, rail
band, mating inset, area efficiency, assumptions) is available through the MCP
`synth_describe_placement` tool.

`env_logger` and the `boards/` canaries are sidecar-pinned and excluded from
the placer corpora: a sidecar means the placer is not under test.

The driver attaches the current best placement image(s) to your prompt. Look at
the dead space and the cluster placement, then make one targeted change. The
image is rendered from the un-routed PCB; the numeric table is authoritative.
