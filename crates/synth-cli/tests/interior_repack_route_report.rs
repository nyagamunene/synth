//! Ignored interior-repack route A/B: base vs repacked candidates. Local.

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

fn route_one(label: &str, board: &synth_ir::Board, placement: &synth_place::Placement) {
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
}

#[test]
#[ignore = "release-mode repack A/B; run locally"]
fn interior_repack_route_report() {
    for design in [
        "../../fixtures/designs/feather_m4_express.synth",
        "../../fixtures/designs/secure_tracker.synth",
    ] {
        let board = load_board(design);
        let sidecars = synth_kicad::Sidecars {
            schematic: None,
            placement: None,
        };
        let base = synth_kicad::placement_candidates(&board, &sidecars).expect("base");
        let repacked =
            synth_kicad::repacked_placement_candidates(&board, &sidecars).expect("repacked");
        let smallest = |candidates: &[synth_place::Placement]| {
            candidates
                .iter()
                .enumerate()
                .min_by_key(|(_, candidate)| {
                    candidate.board_outline.width_nm() * candidate.board_outline.height_nm()
                })
                .map(|(index, _)| index)
                .expect("candidate")
        };
        println!("--- {design}");
        route_one("base first", &board, &base[0]);
        let base_smallest = smallest(&base);
        route_one(
            &format!("base smallest [{base_smallest}]"),
            &board,
            &base[base_smallest],
        );
        route_one("repacked first", &board, &repacked[0]);
        let repacked_smallest = smallest(&repacked);
        route_one(
            &format!("repacked smallest [{repacked_smallest}]"),
            &board,
            &repacked[repacked_smallest],
        );
    }
}
