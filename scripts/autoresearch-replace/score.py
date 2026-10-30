"""Score one board for the shape/connector placement loop.

Primary metric: dead_mm2 = board cells not within NEAR_MM of any component courtyard.
Secondary:     area_mm2 (Edge.Cuts bbox), heavily discounted.
Penalty:       connector_violations (edge distance + mating-face rotation).
Hard gates:    0 open nets, 0 KiCad DRC errors, 0 unconnected pads after a real routed export.
"""
import json
import math
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time

_NUM = r"(-?\d+(?:\.\d+)?)"
_POINT = re.compile(rf"\((?:start|end|mid|center) {_NUM} {_NUM}\)")
_AT = re.compile(r"\(at ([\-\d.]+) ([\-\d.]+)(?: ([\-\d.]+))?")

NEAR_MM = 3.0
CELL_MM = 1.0
EDGE_TOL_MM = 2.0
VIOLATION_WEIGHT = 200.0
LABEL_OVERLAP_WEIGHT = 50.0
LABEL_OFF_WEIGHT = 100.0
AREA_WEIGHT = 0.25
CORNER_MM = 20.0

# Score hermetically: a per-user Tier-2 registry must not shadow the corpus parts.
os.environ["SYNTH_USER_REGISTRY_DIR"] = tempfile.mkdtemp(prefix="ar-shape-user-")
FREEROUTING_JAR = os.environ.get(
    "SYNTH_FREEROUTING_JAR",
    os.path.expanduser("~/work/synth/tools/freerouting/freerouting-2.4.1.jar"),
)
FREEROUTING_JAVA = os.environ.get(
    "SYNTH_FREEROUTING_JAVA",
    os.path.expanduser("~/work/synth/tools/jre25/bin/java"),
)
KRT_REPO = os.environ.get("SYNTH_KRT_REPO", os.path.expanduser("~/tools/KiCadRoutingTools"))
KRT_PYTHON = os.environ.get("SYNTH_KRT_PYTHON", os.path.expanduser("~/tools/krt-venv/bin/python"))
ROUTER_OVERRIDES = {
    "fixtures/designs/secure_tracker.synth": "free-routing",
}


def _gr_blocks(pcb):
    """Yield each top-level (gr_* ...) s-expression, found by paren matching."""
    for m in re.finditer(r"\(gr_\w+\b", pcb):
        depth = 0
        for i in range(m.start(), len(pcb)):
            depth += (pcb[i] == "(") - (pcb[i] == ")")
            if depth == 0:
                yield pcb[m.start() : i + 1]
                break


def board_bbox(pcb):
    pts = [
        (float(x), float(y))
        for block in _gr_blocks(pcb)
        if '(layer "Edge.Cuts")' in block
        for x, y in _POINT.findall(block)
    ]
    if not pts:
        return None
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    return min(xs), min(ys), max(xs), max(ys)


def area_mm2(pcb):
    """Bounding-box area of everything drawn on Edge.Cuts."""
    b = board_bbox(pcb)
    return 0.0 if b is None else (b[2] - b[0]) * (b[3] - b[1])


def parse_footprints(pcb):
    """Reference, centre, rotation and absolute courtyard bbox for every footprint."""
    out = []
    for raw in re.split(r"\n\t\(footprint", pcb)[1:]:
        ref = re.search(r'\(property\s+"Reference"\s+"([^"]+)"', raw, re.S) or re.search(
            r'\(fp_text reference "([^"]+)"', raw
        )
        at = _AT.search(raw)
        if not ref or not at:
            continue
        cx, cy = float(at.group(1)), float(at.group(2))
        rot = float(at.group(3) or 0) % 360.0
        pts = []
        for seg in re.findall(r'\(fp_(?:line|rect|arc|poly)\s*(.*?)\(layer "F\.CrtYd"\)', raw, re.S):
            for sx, sy in re.findall(r"\((?:start|end|center)\s+([\-\d.]+)\s+([\-\d.]+)\)", seg):
                pts.append((float(sx), float(sy)))
            for sx, sy in re.findall(r"\(xy\s+([\-\d.]+)\s+([\-\d.]+)\)", seg):
                pts.append((float(sx), float(sy)))
        bbox = None
        if pts:
            th = math.radians(rot)
            c, s = math.cos(th), math.sin(th)
            gx = [cx + x * c + y * s for x, y in pts]
            gy = [cy - x * s + y * c for x, y in pts]
            bbox = (min(gx), min(gy), max(gx), max(gy))
        out.append({"ref": ref.group(1), "cx": cx, "cy": cy, "rot": rot, "bbox": bbox})
    return out


def _grid(bbox):
    x0, y0, x1, y1 = bbox
    cols = max(0, int(math.floor((x1 - x0) / CELL_MM)))
    rows = max(0, int(math.floor((y1 - y0) / CELL_MM)))
    return cols, rows


def _reachable(bbox, footprints, near_mm):
    x0, y0, x1, y1 = bbox
    cols, rows = _grid(bbox)
    cells = set()
    for f in footprints:
        if not f["bbox"]:
            continue
        bx0, by0, bx1, by1 = f["bbox"]
        ix0 = int(math.floor((bx0 - near_mm - x0) / CELL_MM))
        ix1 = int(math.floor((bx1 + near_mm - x0) / CELL_MM))
        iy0 = int(math.floor((by0 - near_mm - y0) / CELL_MM))
        iy1 = int(math.floor((by1 + near_mm - y0) / CELL_MM))
        for i in range(max(0, ix0), min(cols - 1, ix1) + 1):
            px = x0 + (i + 0.5) * CELL_MM
            for j in range(max(0, iy0), min(rows - 1, iy1) + 1):
                py = y0 + (j + 0.5) * CELL_MM
                if bx0 - near_mm <= px <= bx1 + near_mm and by0 - near_mm <= py <= by1 + near_mm:
                    cells.add((i, j))
    return cells


def dead_area_mm2(pcb, footprints, near_mm=NEAR_MM):
    b = board_bbox(pcb)
    if b is None:
        return 0.0
    cols, rows = _grid(b)
    if cols <= 0 or rows <= 0:
        return 0.0
    return (cols * rows - len(_reachable(b, footprints, near_mm))) * CELL_MM * CELL_MM


def corner_dead_mm2(pcb, footprints, near_mm=NEAR_MM):
    """Dead cells inside the four CORNER_MM squares, de-duplicated."""
    b = board_bbox(pcb)
    if b is None:
        return 0.0
    x0, y0, x1, y1 = b
    reach = _reachable(b, footprints, near_mm)
    cols, rows = _grid(b)
    corner_cols = max(1, int(CORNER_MM / CELL_MM))
    corner_rows = max(1, int(CORNER_MM / CELL_MM))
    dead = set()
    for i in range(cols):
        for j in range(rows):
            if (i, j) in reach:
                continue
            near_x = i < corner_cols or i >= cols - corner_cols
            near_y = j < corner_rows or j >= rows - corner_rows
            if near_x and near_y:
                dead.add((i, j))
    return len(dead) * CELL_MM * CELL_MM


def label_violations(pcb, margin_mm=0.0):
    """Visible reference-designator labels that overlap each other or leave the board."""
    b = board_bbox(pcb)
    if b is None:
        return {"overlaps": [], "off_board": [], "hidden": []}
    labels = []
    for raw in re.split(r"\n\t\(footprint", pcb)[1:]:
        at = _AT.search(raw)
        ref = re.search(r'\(property\s+"Reference"\s+"([^"]+)"', raw, re.S)
        if not at or not ref:
            continue
        depth, end = 0, ref.start()
        for end in range(ref.start(), len(raw)):
            depth += (raw[end] == "(") - (raw[end] == ")")
            if depth == 0:
                break
        block = raw[ref.start() : end + 1]
        local = re.search(r"\(at\s+([\-\d.]+)\s+([\-\d.]+)(?:\s+([\-\d.]+))?\)", block)
        if not local:
            continue
        cx, cy = float(at.group(1)), float(at.group(2))
        rot = float(at.group(3) or 0)
        lx, ly = float(local.group(1)), float(local.group(2))
        lang = float(local.group(3) or 0)
        fs = re.search(r"\(size ([\d.]+) ([\d.]+)\)", block)
        size = float(fs.group(1)) if fs else 1.0
        th = math.radians(rot)
        c, s = math.cos(th), math.sin(th)
        ax, ay = cx + lx * c + ly * s, cy - lx * s + ly * c
        ang = math.radians((rot + lang) % 180)
        tw, tht = len(ref.group(1)) * size * 0.62, size
        labels.append(
            {
                "ref": ref.group(1),
                "x": ax,
                "y": ay,
                "w": abs(tw * math.cos(ang)) + abs(tht * math.sin(ang)),
                "h": abs(tw * math.sin(ang)) + abs(tht * math.cos(ang)),
                "hidden": bool(re.search(r"\(hide\s+yes\)", block)),
            }
        )
    visible = [l for l in labels if not l["hidden"]]
    overlaps = []
    for i in range(len(visible)):
        for j in range(i + 1, len(visible)):
            a, c = visible[i], visible[j]
            if abs(a["x"] - c["x"]) < (a["w"] + c["w"]) / 2 + margin_mm and abs(
                a["y"] - c["y"]
            ) < (a["h"] + c["h"]) / 2 + margin_mm:
                overlaps.append([a["ref"], c["ref"]])
    off = [
        l["ref"]
        for l in visible
        if l["x"] - l["w"] / 2 < b[0] - margin_mm
        or l["x"] + l["w"] / 2 > b[2] + margin_mm
        or l["y"] - l["h"] / 2 < b[1] - margin_mm
        or l["y"] + l["h"] / 2 > b[3] + margin_mm
    ]
    return {
        "overlaps": overlaps,
        "off_board": off,
        "hidden": [l["ref"] for l in labels if l["hidden"]],
    }


def connector_hints(design_path):
    """refdes -> {part, edge} for connector components that declare placement hints."""
    src = pathlib.Path(design_path).read_text()
    hints = {}
    for m in re.finditer(r'component\s+(J\d+):\s*connector\s+"([^"]+)"', src):
        ref, part = m.group(1), m.group(2)
        brace = src.find("{", m.end())
        if brace == -1:
            continue
        depth = 0
        end = brace
        for end in range(brace, len(src)):
            depth += (src[end] == "{") - (src[end] == "}")
            if depth == 0:
                break
        block = src[brace : end + 1]
        hint = re.search(r"placement_hint\s*\{([^}]*)\}", block, re.S)
        if not hint:
            continue
        edge = re.search(r"edge:\s*(\w+)", hint.group(1))
        if edge:
            hints[ref] = {"part": part, "edge": edge.group(1).lower()}
    return hints


_EXPECTED = {"left": 270, "right": 90, "bottom": 0, "top": 180}


def connector_violations(pcb, hints, tol_mm=EDGE_TOL_MM):
    """Connectors far from their declared edge, or with the plug facing the wrong way."""
    b = board_bbox(pcb)
    if b is None:
        return []
    x0, y0, x1, y1 = b
    found = []
    for f in parse_footprints(pcb):
        h = hints.get(f["ref"])
        if not h or not f["bbox"]:
            continue
        bx0, by0, bx1, by1 = f["bbox"]
        edge = h["edge"]
        gap = {
            "left": bx0 - x0,
            "right": x1 - bx1,
            "top": by0 - y0,
            "bottom": y1 - by1,
        }.get(edge)
        if gap is None:
            continue
        issues = []
        if gap > tol_mm:
            issues.append(f"gap {gap:.1f}mm from {edge}")
        if not h["part"].startswith("header"):
            expected = _EXPECTED[edge]
            if int(round(f["rot"])) % 360 != expected:
                issues.append(f"rot {int(round(f['rot'])) % 360} != {expected}")
        if issues:
            found.append({"ref": f["ref"], "edge": edge, "issues": issues})
    return found


def router_for(design):
    """A fixed router per board avoids comparing wire lengths from two engines."""
    return ROUTER_OVERRIDES.get(design, "kicad-routing-tools")


def gates(stdout, exit_code):
    """Hard gates from `synth export-kicad` output. A missing report line fails closed.

    The corpus boards carry pre-existing ERC/footprint overrides, so the export
    can exit non-zero while the routed board is physically clean. The routing
    and DRC lines are the gate; the override status is reported by the exporter
    and is not a placement failure.
    """
    open_nets = re.search(r"(\d+) open,", stdout)
    drc = re.search(r"drc: errors (\d+), unconnected pads (\d+)", stdout)
    if not open_nets or not drc:
        return {"open": None, "errors": None, "unconnected": None, "ok": False}
    o, e, u = int(open_nets[1]), int(drc[1]), int(drc[2])
    return {"open": o, "errors": e, "unconnected": u, "ok": o == e == u == 0}


def keep(tune, held, best_tune, best_held):
    if "FAIL" in (tune, held):
        return False
    return tune < best_tune - 0.01 and held <= best_held


def should_keep(
    tune,
    held,
    best_tune,
    best_held,
    wire_t=None,
    wire_h=None,
    best_wire_t=None,
    best_wire_h=None,
    growth=0.02,
):
    """Keep only on a score win, with neither corpus's routed wire growing > growth."""
    fields = (tune, held, best_tune, best_held)
    if "" in fields or "FAIL" in fields:
        return False
    try:
        if not keep(float(tune), float(held), float(best_tune), float(best_held)):
            return False
    except ValueError:
        return False
    if wire_t is None and wire_h is None:
        return True  # metric-only callers
    wires = (wire_t, wire_h, best_wire_t, best_wire_h)
    if any(v is None or v == "" or v == "FAIL" for v in wires):
        return False
    try:
        return float(wire_t) <= float(best_wire_t) * (1 + growth) and float(
            wire_h
        ) <= float(best_wire_h) * (1 + growth)
    except ValueError:
        return False


def merged_registry(dest):
    """registry/parts plus the parts the corpus needs; --registry replaces the whole registry."""
    shutil.copytree("registry/parts", dest, dirs_exist_ok=True)
    shutil.copytree(pathlib.Path(__file__).parent / "extra_parts", dest, dirs_exist_ok=True)
    return dest


def score_board(synth_bin, design, out_dir, krt_repo, krt_python, registry=None):
    start = time.monotonic()
    try:
        router = router_for(design)
        router_args = (
            ["--freerouting-jar", FREEROUTING_JAR, "--freerouting-java", FREEROUTING_JAVA]
            if router == "free-routing"
            else ["--kicad-routing-tools-repo", KRT_REPO,
                  "--kicad-routing-tools-python", KRT_PYTHON]
        )
        run = subprocess.run(
            [
                synth_bin, "export-kicad", design, "--out", out_dir, "--force",
                "--router", router,
                *router_args,
                *(["--registry", registry] if registry else []),
            ],
            capture_output=True, text=True, timeout=900,
            env={**os.environ, "SYNTH_PLACE_REPLACE": "1"},
        )
    except subprocess.TimeoutExpired:
        return {"open": None, "errors": None, "unconnected": None, "ok": False,
                "why": "timeout after 900s", "area_mm2": None, "dead_mm2": None,
                "corner_dead_mm2": None, "connector_violations": None, "score": None}
    text = run.stdout + run.stderr
    g = gates(text, run.returncode)
    if not g["ok"]:
        lines = [line for line in text.strip().splitlines() if line.strip()]
        g["why"] = " | ".join(lines[-5:])[:400] or "no output"
    pcbs = sorted(pathlib.Path(out_dir).glob("*.kicad_pcb"))
    if g["ok"] and pcbs:
        pcb = pcbs[0].read_text()
        fps = parse_footprints(pcb)
        hints = connector_hints(design)
        violations = connector_violations(pcb, hints)
        labels = label_violations(pcb)
        g["area_mm2"] = round(area_mm2(pcb), 2)
        g["dead_mm2"] = round(dead_area_mm2(pcb, fps), 2)
        g["corner_dead_mm2"] = round(corner_dead_mm2(pcb, fps), 2)
        g["connector_violations"] = violations
        g["label_overlaps"] = len(labels["overlaps"])
        g["labels_off_board"] = len(labels["off_board"])
        g["labels_hidden"] = len(labels["hidden"])
        g["score"] = round(
            g["dead_mm2"]
            + AREA_WEIGHT * g["area_mm2"]
            + VIOLATION_WEIGHT * len(violations)
            + LABEL_OVERLAP_WEIGHT * g["label_overlaps"]
            + LABEL_OFF_WEIGHT * g["labels_off_board"],
            2,
        )
    else:
        g["area_mm2"] = g["dead_mm2"] = g["corner_dead_mm2"] = g["score"] = None
        g["connector_violations"] = None
        g["label_overlaps"] = g["labels_off_board"] = g["labels_hidden"] = None
    # The exporter names the record after the board, not the source file, so a
    # design whose board name differs from its filename has no `<stem>.json`.
    records = sorted(pathlib.Path(out_dir).glob("*.routing.json"))
    record_path = records[0] if records else None
    if record_path is not None:
        try:
            record = json.loads(record_path.read_text())
        except (OSError, ValueError):
            record = {}
        provenance = record.get("provenance") or {}
        statistics = record.get("statistics") or {}
        g["engine"] = provenance.get("engine")
        g["router_ms"] = provenance.get("duration_ms")
        g["segments"] = statistics.get("segments")
        g["vias"] = statistics.get("vias")
        g["wire_length_mm"] = statistics.get("wire_length_mm")
        g["open_nets"] = statistics.get("open_nets")
    if g["ok"] and g.get("wire_length_mm") is None:
        g["ok"] = False
        g["why"] = "routing record missing wire_length_mm"
    g["wall_s"] = round(time.monotonic() - start, 1)
    return g


if __name__ == "__main__":
    import sys

    home = os.path.expanduser("~/tools")
    synth = os.environ.get("SYNTH_BIN", "target/release/synth")
    total, wire_total, failed = 0.0, 0.0, []
    reg_dir = tempfile.TemporaryDirectory(prefix="ar-shape-reg-")
    reg = merged_registry(reg_dir.name)
    for design in sys.argv[1:]:
        with tempfile.TemporaryDirectory(prefix="ar-shape-") as out:
            s = score_board(
                synth, design, out, f"{home}/KiCadRoutingTools", f"{home}/krt-venv/bin/python", reg
            )
        print(design, json.dumps(s), flush=True)
        if s["ok"]:
            total += s["score"]
            wire_total += s.get("wire_length_mm") or 0
        else:
            failed.append(design)
    print(f"SCORE {'FAIL' if failed else round(total, 2)} failed={len(failed)}")
    print(f"WIRE {round(wire_total, 1)}")
