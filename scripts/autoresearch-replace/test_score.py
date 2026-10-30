import pathlib
import tempfile

import score

EDGE = """\t(gr_line (start 0 0) (end 20 0) (layer "Edge.Cuts"))
\t(gr_line (start 20 0) (end 20 10) (layer "Edge.Cuts"))
\t(gr_line (start 20 10) (end 0 10) (layer "Edge.Cuts"))
\t(gr_line (start 0 10) (end 0 0) (layer "Edge.Cuts"))"""


def pcb(ref="J1", part="jst_xh_2pin", x=18, y=5, rot=90):
    return f"""(kicad_pcb
\t(footprint "lib:{part}"
\t\t(at {x} {y} {rot})
\t\t(property
\t\t\t"Reference"
\t\t\t"{ref}"
\t\t)
\t\t(fp_rect
\t\t\t(start -1 -1)
\t\t\t(end 1 1)
\t\t\t(layer "F.CrtYd")
\t\t)
\t)
{EDGE}
)"""


def design(part="jst_xh_2pin", edge="right"):
    d = pathlib.Path(tempfile.mkdtemp()) / "t.synth"
    d.write_text(
        'board "t" {\n'
        f'  component J1: connector "{part}" value "X" {{\n'
        f"    placement_hint {{ edge: {edge} priority: hard }}\n"
        "  }\n"
        "}\n"
    )
    return d


def label_pcb(x2=2.5, lx=0):
    return f"""(kicad_pcb
\t(footprint "a"
\t\t(at 2 2 0)
\t\t(property "Reference" "J1"
\t\t\t(at 0 0 0)
\t\t\t(layer "F.SilkS")
\t\t\t(effects (font (size 1 1)))
\t\t)
\t\t(fp_rect (start -1 -1) (end 1 1) (layer "F.CrtYd"))
\t)
\t(footprint "b"
\t\t(at {x2} 2 0)
\t\t(property "Reference" "J2"
\t\t\t(at {lx} 0 0)
\t\t\t(layer "F.SilkS")
\t\t\t(effects (font (size 1 1)))
\t\t)
\t\t(fp_rect (start -1 -1) (end 1 1) (layer "F.CrtYd"))
\t)
{EDGE}
)"""


def test_area_is_the_edge_cuts_bounding_box_only():
    assert score.area_mm2(pcb()) == 200.0


def test_dead_area_excludes_a_3mm_ring_round_the_courtyard():
    p = pcb()
    fps = score.parse_footprints(p)
    dead = score.dead_area_mm2(p, fps)
    assert 0 < dead < 200
    assert score.corner_dead_mm2(p, fps) > 0


def test_connector_on_its_edge_with_the_plug_outward_is_clean():
    p = pcb()
    assert score.connector_violations(p, score.connector_hints(design())) == []


def test_connector_far_from_its_edge_is_a_violation():
    p = pcb(x=15)
    v = score.connector_violations(p, score.connector_hints(design()))
    assert len(v) == 1 and "gap" in v[0]["issues"][0]


def test_connector_facing_the_wrong_way_is_a_violation():
    p = pcb(rot=0)
    v = score.connector_violations(p, score.connector_hints(design()))
    assert len(v) == 1 and "rot" in v[0]["issues"][0]


def test_a_header_is_not_rotation_checked_but_is_still_distance_checked():
    p = pcb(part="header_1x4", rot=0)
    assert score.connector_violations(p, score.connector_hints(design("header_1x4"))) == []
    p = pcb(part="header_1x4", x=10, rot=0)
    assert len(score.connector_violations(p, score.connector_hints(design("header_1x4")))) == 1


def test_overlapping_reference_labels_are_reported():
    assert score.label_violations(label_pcb(2.5))["overlaps"] == [["J1", "J2"]]
    assert score.label_violations(label_pcb(6))["overlaps"] == []


def test_a_label_off_the_board_is_reported():
    assert score.label_violations(label_pcb(6, lx=20))["off_board"] == ["J2"]
    assert score.label_violations(label_pcb(6))["off_board"] == []


def test_keep_needs_a_tuning_win_and_no_held_out_loss():
    best = (100.0, 50.0)
    assert score.keep(99.0, 50.0, *best) is True
    assert score.keep(100.0, 50.0, *best) is False
    assert score.keep(99.0, 51.0, *best) is False


def test_a_failed_or_missing_total_never_keeps():
    assert score.should_keep("FAIL", "50", "100", "50") is False
    assert score.should_keep("99", "50", "", "50") is False
    assert score.should_keep("99", "50", "100", "50") is True


def test_wire_growth_is_capped_even_on_a_score_win():
    ok = ("99", "50", "100", "50")
    assert score.should_keep(*ok, "1010", "505", "1000", "500") is True
    assert score.should_keep(*ok, "1030", "505", "1000", "500") is False
    assert score.should_keep(*ok, "1010", "520", "1000", "500") is False
    assert score.should_keep(*ok, "", "505", "1000", "500") is False


def test_fitness_gate_rejects_a_new_electrical_failure_per_corpus():
    ok = ("99", "50", "100", "50", "1010", "505", "1000", "500")
    assert score.should_keep(
        *ok, fitness_t=2, fitness_h=1, best_fitness_t=2, best_fitness_h=1
    ) is True
    assert score.should_keep(
        *ok, fitness_t=3, fitness_h=1, best_fitness_t=2, best_fitness_h=1
    ) is False
    assert score.should_keep(
        *ok, fitness_t=2, fitness_h=2, best_fitness_t=2, best_fitness_h=1
    ) is False
    assert score.should_keep(
        *ok, fitness_t="", fitness_h=1, best_fitness_t=2, best_fitness_h=1
    ) is False


def test_fitness_gate_applies_without_wire_metrics():
    assert score.should_keep(
        "99", "50", "100", "50", fitness_t=1, fitness_h=1, best_fitness_t=1, best_fitness_h=1
    ) is True
    assert score.should_keep(
        "99", "50", "100", "50", fitness_t=2, fitness_h=1, best_fitness_t=1, best_fitness_h=1
    ) is False


def test_the_driver_positional_contract_includes_growth_before_fitness():
    # This is the exact order loop.sh builds: tune, held, best_t, best_h,
    # wire_t, wire_h, best_w, best_hw, growth, fitness_t, fitness_h,
    # best_fitness_t, best_fitness_h.
    assert score.should_keep(
        "99", "50", "100", "50", "1010", "505", "1000", "500",
        "0.02", "1", "1", "1", "1",
    ) is True
    assert score.should_keep(
        "99", "50", "100", "50", "1010", "505", "1000", "500",
        "0.02", "2", "1", "1", "1",
    ) is False


def test_negative_fitness_counts_are_rejected():
    assert score.should_keep(
        "99", "50", "100", "50", "1010", "505", "1000", "500",
        "0.02", "-1", "0", "1", "0",
    ) is False
