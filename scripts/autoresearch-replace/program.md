# RePlAce parameter research

Goal: tune the RePlAce equations so the placement is denser (lower
`dead_mm2` and `area_mm2`) without losing routing or electrical placement.
The scorer is frozen; it exports with `SYNTH_PLACE_REPLACE=1`, so every round
tests the RePlAce path, not the CEM default.

The score is

    dead_mm2 + 0.25 * area_mm2 + 200 * connector_violations
            + 50 * label_overlaps + 100 * labels_off_board

summed over the tuning corpus; lower is better. Every board must export with
0 open nets, 0 KiCad DRC errors and 0 unconnected pads. A keep also requires
routed wire length not to grow by more than 2% on either corpus, and the
electrical fitness gate must not regress.

## Electrical gate

The driver passes `--fitness-json` to every export, so each run writes a
placement fitness report. It measures four things the area score cannot see:

- **support distance** — a bound decoupling capacitor more than 3 mm from the
  pin it serves, or a rail/load capacitor more than 5 mm; a part's declared
  `max_distance_mm` overrides the default;
- **rail band** — two-pin passives parked in the top connector lane; passives
  within 2 mm of a connector are treated as serving it and excluded. The band
  counts as one failure when any passive remains in the lane;
- **mating inset** — a connector more than 3 mm from the edge it faces (the
  nearest edge when no face is declared), or a body outside the outline;
- **area efficiency** — reported, never gated.

`gate_failures` is the count of broken limits. The driver records the total on
both corpora as `FITNESS`; a keep requires it not to rise above the current
best on either corpus. The 0–100 composite is **not** an objective: do not trade a stranded
capacitor for a better rail ratio or a smaller outline. The fitness gate is a
floor, not a score to maximise.

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
- Trading support distance, rail-band discipline or mating inset for area.
  The `FITNESS` gate rejects it.

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
- If dead area drops but `FITNESS` rises, stop: the density force is probably
  pushing decoupling capacitors away from their anchors. Back off `lambda_*`
  or the bin density rather than accepting the trade.
- Compare `Poisson` only if you are also fixing its self-force; otherwise keep
  `Bin`.
