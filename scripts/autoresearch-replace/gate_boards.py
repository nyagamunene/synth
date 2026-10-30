"""Per-board gates between a candidate round and the current best logs.

The corpus totals in `should_keep` let a win on one board pay for a regression
on another. This helper compares the same board on both sides and rejects a
rise in fitness failures, routed wire, or support distance.

Usage:
    gate_boards.py CAND_TUNING BEST_TUNING CAND_HELD BEST_HELD

Prints one line per failed board and exits 1. A missing or failed board fails
closed. Both the post-fix field names (`fitness_gate_failures`,
`support_mean_mm`) and the older ones are accepted.
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
        if not line or line.startswith(("SCORE", "WIRE", "GATES", "FITNESS", "TEST")):
            continue
        parts = line.split(" ", 1)
        if len(parts) != 2:
            continue
        try:
            boards[parts[0]] = json.loads(parts[1])
        except ValueError:
            continue
    return boards


def field(board, *names):
    for name in names:
        if name in board:
            return board[name]
    return None


def failures(board):
    value = field(board, "fitness_gate_failures", "fitness_failures")
    if value is None:
        value = (board.get("fitness") or {}).get("gate_failures")
    return value


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

        before_failures = failures(before)
        after_failures = failures(after)
        if before_failures is None or after_failures is None:
            problems.append(f"{board}: fitness failures missing")
        elif after_failures > before_failures:
            problems.append(
                f"{board}: fitness failures {before_failures} -> {after_failures}"
            )

        before_wire = before.get("wire_length_mm") or 0
        after_wire = after.get("wire_length_mm") or 0
        if before_wire > 0 and after_wire > before_wire * (1 + WIRE_GROWTH):
            problems.append(f"{board}: wire {before_wire:.1f} -> {after_wire:.1f} mm")

        for names, key in (
            (("support_mean", "support_mean_mm"), "support mean"),
            (("support_worst", "support_worst_mm"), "support worst"),
        ):
            before_support = field(before, *names)
            after_support = field(after, *names)
            if before_support is None or after_support is None:
                continue
            allowed = before_support + max(
                SUPPORT_GROWTH_MM, SUPPORT_GROWTH_FRACTION * before_support
            )
            if after_support > allowed:
                problems.append(
                    f"{board}: {key} {before_support:.2f} -> {after_support:.2f} mm"
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
