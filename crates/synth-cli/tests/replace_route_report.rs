//! Ignored RePlAce route A/B: default vs place_with_replace. Local.

use std::path::Path;

fn load_board(path: &str) -> synth_ir::Board {
    let source = std::fs::read_to_string(path).expect("read board");
    let file = path.to_string();
    let parse = synth_parser::parse(&source, file.clone());
    let ast = parse.ast.as_ref().expect("parse");
    let registry_dir = Path::new("../..").join("registry").join("parts");
    let registry = synth_registry::load_dir(&registry_dir).expect("registry");
    let loader = synth_ir::FsImportLoader {
        root: std::path::PathBuf::from("../.."),
    };
    let resolved = synth_ir::resolve_imports(ast, &loader, &file);
    let lowered = synth_ir::lower(&resolved.program, &registry, &file);
    lowered.board.expect("board")
}

fn report(label: &str, board: &synth_ir::Board, placement: &synth_place::Placement) {
    let hpwl = synth_place::score::score_placement(board, placement).hpwl_total_nm;
    let routing = synth_route::route(board, placement);
    let track: f64 = routing
        .segments
        .iter()
        .map(|segment| {
            let dx = synth_geometry::nm_to_mm(segment.end.x_nm - segment.start.x_nm);
            let dy = synth_geometry::nm_to_mm(segment.end.y_nm - segment.start.y_nm);
            dx.hypot(dy)
        })
        .sum();
    println!(
        "  {label}: {:.2}x{:.2} mm ({:.0} mm2) hpwl {:.1} | unrouted {} | copper {} | wire {:.1}",
        synth_geometry::nm_to_mm(placement.board_outline.width_nm()),
        synth_geometry::nm_to_mm(placement.board_outline.height_nm()),
        synth_geometry::nm_to_mm(placement.board_outline.width_nm())
            * synth_geometry::nm_to_mm(placement.board_outline.height_nm()),
        synth_geometry::nm_to_mm(hpwl),
        routing.unrouted_nets.len(),
        routing.segments.len() + routing.vias.len(),
        track
    );
    for net in &routing.unrouted_nets {
        println!("    UNROUTED {}", net.net_name);
    }
}

#[test]
#[ignore = "release-mode RePlAce A/B; run locally"]
fn replace_route_report() {
    for design in [
        "../../fixtures/designs/secure_tracker.synth",
        "../../examples/sensor_logger/sensor_logger.synth",
        "../../examples/env_logger/env_logger.synth",
        "../../fixtures/designs/decoupling_heavy.synth",
        "../../fixtures/designs/iot_sensor_board.synth",
    ] {
        let board = load_board(design);
        let default = synth_place::place(&board).expect("default");
        let replace = synth_place::place_with_replace(&board).expect("replace");
        println!("--- {design}");
        report("default", &board, &default);
        report("replace", &board, &replace);
    }
}
