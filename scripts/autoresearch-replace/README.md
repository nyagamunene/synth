# Dead-space / routability placement research

An autoresearch loop on top of the analytical-placement branch. It reuses the
`003-shape-connector` scorer: dead cells, area, connector and label penalties,
with hard routing gates and a 2% wire cap.

## Metric

    dead_mm2 + 0.25 * area_mm2 + 200 * connector_violations
            + 50 * label_overlaps + 100 * labels_off_board

Lower is better. A keep requires a tuning drop, no held-out growth, and routed
wire length within +2% on both corpora.

## Gates

Every board must export with 0 open nets, 0 KiCad DRC errors and 0 unconnected
pads. The router is FreeRouting (the bundled JAR/Java under the main checkout,
overridable with `SYNTH_FREEROUTING_JAR` / `SYNTH_FREEROUTING_JAVA`). The export
may be marked untrusted by pre-existing ERC/footprint overrides; the routed and
DRC lines are the gate, not the override status. The scorer and corpus are
frozen.

The driver passes `--fitness-json`, so the export also writes a placement
fitness report. `FITNESS` is the total `gate_failures` over the corpus: support
distance (3 mm decoupling, 5 mm rail/load; a part's declared `max_distance_mm`
overrides the default), rail-band passives (one failure when any passive is in
the lane, excluding parts within 2 mm of a connector), and connector mating
inset (3 mm, the nearest edge when no mating face is declared; a body outside
the outline fails). A keep requires `FITNESS` not to rise above the current
best on either corpus. The 0–100 composite is a diagnostic, not an objective.

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
