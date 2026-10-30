"""Per-board gates between a candidate round and the current best logs.

The corpus totals in `should_keep` let a win on one board pay for a regression
on another. This helper compares the same board on both sides and rejects a
rise in fitness failures, routed wire, or support distance.

Usage:
    gate_boards.py CAND_TUNING BEST_TUNING CAND_HELD BEST_HELD

Prints one line per failed board and exits 1. A missing or failed board fails
closed.
"""
import json
import sys

WIRE_GROWTH = 0.02
SUPPORT_GROWTH_MM = 0.5
SUPPORT_GROWTH_FRACTION = 0.15


def load(path):
    boards = {}
    for line in open(path):
        line = line.strip()
        if not line or line.startswith(("SCORE", "WIRE", "FITNESS", "TEST")):
            continue
        parts = line.split(" ", 1)
        if len(parts) != 2:
            continue
        try:
            boards[parts[0]] = json.loads(parts[1])
        except ValueError:
            continue
    return boards


def compare(candidate, best):
    problems = []
    for board, before in best.items():
        after = candidate.get(board)
        if after is None:
            problems.append(f"{board}: missing")
            continue
        if not after.get("ok"):
            problems.append(f"{board}: not clean ({after.get('why', 'no reason')})")
            continue

        failures_before = before.get("fitness_failures") or 0
        failures_after = after.get("fitness_failures") or 0
        if failures_after > failures_before:
            problems.append(
                f"{board}: fitness failures {failures_before} -> {failures_after}"
            )

        wire_before = before.get("wire_length_mm") or 0
        wire_after = after.get("wire_length_mm") or 0
        if wire_before > 0 and wire_after > wire_before * (1 + WIRE_GROWTH):
            problems.append(
                f"{board}: wire {wire_before:.1f} -> {wire_after:.1f} mm"
            )

        for key, tolerance in (
            ("support_mean", SUPPORT_GROWTH_MM),
            ("support_worst", SUPPORT_GROWTH_MM),
        ):
            before_value = before.get(key)
            after_value = after.get(key)
            if before_value is None or after_value is None:
                continue
            allowed = before_value + max(
                tolerance, SUPPORT_GROWTH_FRACTION * before_value
            )
            if after_value > allowed:
                problems.append(
                    f"{board}: {key} {before_value:.2f} -> {after_value:.2f} mm"
                )
    return problems


def main(argv):
    if len(argv) != 5:
        print("usage: gate_boards.py CAND_TUNING BEST_TUNING CAND_HELD BEST_HELD")
        return 2
    problems = compare(load(argv[1]), load(argv[2])) + compare(
        load(argv[3]), load(argv[4])
    )
    if problems:
        print("\n".join(problems))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
