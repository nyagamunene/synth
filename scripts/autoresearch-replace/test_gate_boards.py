import json
import pathlib
import tempfile

import gate_boards


def line(board, **values):
    return f"{board}.synth {json.dumps(values)}\n"


def log(*lines):
    path = pathlib.Path(tempfile.mkdtemp()) / "log"
    path.write_text("".join(lines) + "SCORE 1\nWIRE 1\nFITNESS 0\n")
    return path


def test_clean_candidate_passes():
    best = log(line("a", ok=True, fitness_failures=1, wire_length_mm=100, support_mean=4))
    cand = log(line("a", ok=True, fitness_failures=1, wire_length_mm=101, support_mean=4.2))
    assert gate_boards.compare(gate_boards.load(cand), gate_boards.load(best)) == []


def test_more_fitness_failures_fail():
    best = log(line("a", ok=True, fitness_gate_failures=0, wire_length_mm=100))
    cand = log(line("a", ok=True, fitness_gate_failures=1, wire_length_mm=100))
    assert gate_boards.compare(gate_boards.load(cand), gate_boards.load(best))


def test_more_wire_fails():
    best = log(line("a", ok=True, wire_length_mm=100, fitness_gate_failures=0))
    cand = log(line("a", ok=True, wire_length_mm=103, fitness_gate_failures=0))
    assert gate_boards.compare(gate_boards.load(cand), gate_boards.load(best))


def test_missing_or_failed_board_fails_closed():
    best = log(line("a", ok=True, wire_length_mm=100, fitness_gate_failures=0))
    cand = log(line("a", ok=False, wire_length_mm=None))
    assert gate_boards.compare(gate_boards.load(cand), gate_boards.load(best))


def test_zero_baseline_wire_is_not_gated():
    best = log(line("a", ok=True, wire_length_mm=0, fitness_gate_failures=0))
    cand = log(line("a", ok=True, wire_length_mm=50, fitness_gate_failures=0))
    assert gate_boards.compare(gate_boards.load(cand), gate_boards.load(best)) == []
