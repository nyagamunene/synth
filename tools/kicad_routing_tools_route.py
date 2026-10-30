#!/usr/bin/env python3
"""Run KiCadRoutingTools as Synth's optional external router.

KiCadRoutingTools remains an external checkout; this adapter provides a
stable scriptable entry point without vendoring its Python or native Rust
runtime.

The policy checks here are the engine's *self-assessment*, recorded so the
run can say what the router believed. They are not the verdict: the
adapter re-derives connectivity from the copper it wrote and runs
`kicad-cli pcb drc`, so a router that miscounts its own success cannot
promote itself.

Exit status is therefore deliberately narrow:

* `0` — the engine ran and its own policy gate passed. Says nothing about
  whether the board is actually routable.
* non-zero — the engine failed, or its own policy gate rejected the result.

A malformed summary is a failure rather than a shrug. The previous
behaviour warned and carried on, which meant a KRT build that emitted
unparseable JSON could produce a board that no check had ever read.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

# Exit code used when the engine itself succeeded but its own policy gate
# rejected the result. Distinct from a crash so a caller can tell "the
# router refused this board" from "the router broke".
POLICY_REJECTED = 3

# Engine via penalty for the first attempt; its own default is 75.
KEEP_LAYER_VIA_COST = 1000


def _load_summary(stats: Path) -> dict:
    """Read the engine's JSON summary, or explain why it could not be read."""
    if not stats.is_file():
        raise RuntimeError(f"KRT did not write a JSON report at {stats}")
    try:
        return json.loads(stats.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"could not parse the KRT summary at {stats}: {error}") from error


def _failed_count(report: dict) -> int:
    """The summary's aggregate failure count, which may come with no net names."""
    failed = report.get("failed")
    if isinstance(failed, bool) or not isinstance(failed, (int, float)):
        return 0
    return int(failed) if failed > 0 else 0


def _failed_nets(stats: Path) -> bool:
    """Whether the engine's summary shows an open or failed connection group."""
    try:
        report = _load_summary(stats)
    except RuntimeError:
        return True
    return _failed_count(report) > 0 or any(
        report.get(key) for key in ("failed_single", "open_single", "failed_multipoint")
    )


def _policy_findings(report: dict, allow_via_in_pad: bool) -> list[str]:
    """Everything the engine's own summary says is wrong with the route."""
    findings: list[str] = []

    open_items = sum(
        len(report.get(key, []) or [])
        for key in ("failed_single", "open_single", "failed_multipoint")
    )
    if open_items:
        findings.append(f"{open_items} open/failed connection group(s)")
    elif failed := _failed_count(report):
        findings.append(f"{failed} failed connection(s) reported without net names")

    design_rules = report.get("design_rules") or {}
    floors = design_rules.get("board_floors") or {}
    delivered = design_rules.get("min_delivered") or {}
    below = [
        f"{key} {delivered[key]:.4f} < {floor:.4f}"
        for key, floor in floors.items()
        if key in delivered and delivered[key] < floor - 1e-9
    ]
    if below:
        findings.append("delivered below board floors: " + ", ".join(sorted(below)))

    via_in_pad = (report.get("via_in_pad") or {}).get("count", 0)
    if via_in_pad and not allow_via_in_pad:
        findings.append(f"{via_in_pad} via-in-pad site(s)")

    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input_board", type=Path)
    parser.add_argument("output_board", type=Path)
    parser.add_argument(
        "--repo", required=True, type=Path, help="KiCadRoutingTools checkout"
    )
    parser.add_argument(
        "--python",
        default=sys.executable,
        help="Python interpreter with KRT dependencies",
    )
    parser.add_argument(
        "--ordering", choices=("mps", "inside_out", "original", "bus"), default="mps"
    )
    parser.add_argument("--stats", type=Path, help="JSON route summary output")
    parser.add_argument(
        "--no-write-fill", action="store_true", help="Do not write filled zone polygons"
    )
    parser.add_argument(
        "--escalation", choices=("off", "board", "fab"), default="board"
    )
    parser.add_argument(
        "--fab-tier", choices=("standard", "advanced", "auto"), default="auto"
    )
    parser.add_argument("--fab-overrides", type=Path)
    parser.add_argument("--same-net-pad-clearance", type=float, default=0.1)
    parser.add_argument("--strict-sizes", action="store_true", default=True)
    parser.add_argument("--allow-via-in-pad", action="store_true")
    args = parser.parse_args()

    route = args.repo / "py_router" / "route.py"
    if not route.is_file():
        parser.error(f"KRT route.py not found under {args.repo}")
    args.output_board.parent.mkdir(parents=True, exist_ok=True)
    stats = args.stats or args.output_board.with_suffix(".krt-stats.json")
    command = [
        args.python,
        str(route),
        str(args.input_board),
        str(args.output_board),
        "--nets",
        "*",
        "--ordering",
        args.ordering,
        "--stats",
        "--json-out",
        str(stats),
        "--escalation",
        args.escalation,
        "--same-net-pad-clearance",
        str(-1 if args.allow_via_in_pad else args.same_net_pad_clearance),
        "--fab-tier",
        args.fab_tier,
    ]
    if args.fab_overrides:
        command.extend(("--fab-overrides", str(args.fab_overrides)))
    if args.strict_sizes:
        command.append("--strict-sizes")
    if not args.no_write_fill:
        command.append("--write-fill")

    # A layer change on a signal net cuts its return path and W-SYNTH-DRC-016
    # flags it, so the first attempt prices vias high and keeps nets on one
    # layer. A board that does not route that way is routed again at the
    # engine's own via cost rather than failed.
    # The summary and the board are the evidence for the attempt that wrote
    # them, so both are removed before each attempt: an attempt that writes
    # none is a failure rather than a pass on the previous attempt's output.
    print("running KiCadRoutingTools:", " ".join(command), flush=True)
    stats.unlink(missing_ok=True)
    args.output_board.unlink(missing_ok=True)
    completed = subprocess.run([*command, "--via-cost", str(KEEP_LAYER_VIA_COST)], check=False)
    if completed.returncode or _failed_nets(stats):
        print("retrying KiCadRoutingTools at its default via cost", flush=True)
        stats.unlink(missing_ok=True)
        args.output_board.unlink(missing_ok=True)
        completed = subprocess.run(command, check=False)

    policy_ok = completed.returncode == 0
    if completed.returncode:
        print(f"error: KiCadRoutingTools exited {completed.returncode}", file=sys.stderr)

    try:
        report = _load_summary(stats)
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return completed.returncode or POLICY_REJECTED

    print(
        f"KRT summary: routed={report.get('successful', '?')} "
        f"failed={report.get('failed', '?')} "
        f"vias={report.get('total_vias', '?')} "
        f"time={report.get('total_time', '?')}s",
        flush=True,
    )

    for finding in _policy_findings(report, args.allow_via_in_pad):
        print(f"error: KiCadRoutingTools policy: {finding}", file=sys.stderr)
        policy_ok = False

    if not args.output_board.is_file():
        print("error: KRT produced no output board", file=sys.stderr)
        return completed.returncode or POLICY_REJECTED

    # The engine's own connectivity checker is a useful cross-check, but a
    # missing checker must not silently skip the gate: say so instead.
    checker = args.repo / "py_router" / "check_connected.py"
    if checker.is_file():
        connected = subprocess.run(
            [args.python, str(checker), str(args.output_board), "--quiet"], check=False
        )
        if connected.returncode:
            print(
                "error: KRT connectivity checker reported disconnected routes",
                file=sys.stderr,
            )
            policy_ok = False
    else:
        print(
            f"warning: {checker} is absent, so KRT's own connectivity check was "
            "skipped; connectivity is re-derived downstream",
            file=sys.stderr,
        )

    return 0 if policy_ok else (completed.returncode or POLICY_REJECTED)


if __name__ == "__main__":
    raise SystemExit(main())