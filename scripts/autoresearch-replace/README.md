# RePlAce parameter research

An autoresearch loop on top of the analytical-placement branch. It reuses the
`003-shape-connector` scorer: dead cells, area, connector and label penalties,
with hard routing gates and a 2% wire cap.

## Metric

    dead_mm2 + 0.25 * area_mm2 + 200 * connector_violations
            + 50 * label_overlaps + 100 * labels_off_board
            + 2 * (100 - fitness_score) + 50 * fitness_gate_failures

Lower is better. `fitness_score` is PR #400's 0-100 electrical composite
(higher is better); `fitness_gate_failures` is its count of support, rail and
mating failures. A keep requires a tuning drop, no held-out growth, routed wire
length within +2%, and no increase in the gate-failure total.

## Electrical fitness and vision

`SYNTH_EXPORT_FITNESS=1` makes the exporter write `<stem>.fitness.json` for
the placement it used. The scorer reads it and prints `fit/gate/sup/mat` in the
per-board table. The full report is available to the agent through the MCP
`synth_describe_placement` tool.

The scorer renders the un-routed placement for `AR_VISION_BOARDS`
(default `secure_tracker,sensor_logger`) into `AR_VISION_DIR`. The driver keeps
the current best images in `logs/autoresearch-*/vision/best` and attaches them
to each `opencode run` prompt with `--file`, so the agent can see the dead space
as well as the numbers.

## Gates

Every board must export with 0 open nets, 0 KiCad DRC errors and 0 unconnected
pads. The router is fixed per board (KRT for the small corpus, FreeRouting for
`secure_tracker`; overridable with `SYNTH_KRT_*` / `SYNTH_FREEROUTING_*`). The export
may be marked untrusted by pre-existing ERC/footprint overrides; the routed and
DRC lines are the gate, not the override status. The scorer and corpus are
frozen.

## Corpus

- Tuning: `secure_tracker`, `sensor_logger`, `env_logger`, `decoupling_heavy`,
  `iot_sensor_board`.
- Held-out: `ldo_block`, `divider`, `mcu`, `placement_and_diff_pair`.

`secure_tracker` and `iot_sensor_board` are the hard cases: large interior dead
space and connector-bound outlines. `feather` is excluded: two pre-existing DRC
errors are unreachable from placement. The rest guard against regressions.

## Arms

- `density`: wire or improve the guarded repack/density work.
- `shape`: outline, group and passive packing.

## Run

```bash
cd arms/density
bash scripts/autoresearch-deadspace/loop.sh          # 40 rounds, 8 h
MAX_EXP=20 MAX_SECONDS=14400 bash scripts/autoresearch-deadspace/loop.sh
```

The driver builds, scores, prompts the agent, keeps or reverts, and commits.
One loop per arm. Do not run two drivers in one worktree. The baseline is the
current branch tip `0395c6b`.

## Files

- `program.md` — per-round agent brief.
- `score.py` — export, gates, dead/area/connector/label score.
- `loop.sh` — build/score/prompt/keep/revert.
- `corpus_tuning.txt`, `corpus_heldout.txt`, `extra_parts/`.

## Logs

- `results/<arm>.out` — console log from `run.sh` (start header, arm PID, final summary).
- `arms/<arm>/logs/autoresearch-replace/loop.log` — full driver log: phases, elapsed
  time, build/score durations, per-board tables, agent exit codes, keep/discard reasons.
- `arms/<arm>/logs/autoresearch-replace/agent-<n>.log` — the OpenCode transcript for round n.
- `arms/<arm>/logs/autoresearch-replace/tuning-<n>.log` / `heldout-<n>.log` — per-board JSON
  from the scorer.
- `arms/<arm>/results-replace.tsv` — one row per round (round, commit, scores, wire, status, note).
