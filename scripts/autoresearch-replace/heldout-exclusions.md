# Corpus exclusions

## feather_m4_express (2026-10-30)

Excluded from tuning. Both routers leave two pre-existing DRC errors on the
routed board (a `bh1750_lux` pad-clearance violation and a self-referential
GND-zone unconnected item). They are board/footprint issues, not placement, and
none of them is reachable from the loop's allowed crates. `secure_tracker` is
the clean large-board target instead.

The board can still be scored manually after the corpus run.
