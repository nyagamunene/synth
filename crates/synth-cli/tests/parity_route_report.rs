//! Ignored route-loop parity A/B: export winner vs the existing candidate set.

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

#[test]
#[ignore = "release-mode parity A/B; run locally"]
fn parity_route_report() {
    for design in [
        "../../fixtures/designs/feather_m4_express.synth",
        "../../fixtures/designs/secure_tracker.synth",
    ] {
        let board = load_board(design);
        let sidecars = synth_kicad::Sidecars {
            schematic: None,
            placement: None,
        };
        let candidates = synth_kicad::placement_candidates(&board, &sidecars).expect("candidates");
        println!("--- {design}: {} candidates", candidates.len());
        for (index, placement) in candidates.iter().enumerate() {
            let hpwl = synth_place::score::score_placement(&board, placement).hpwl_total_nm;
            let routing = synth_route::route(&board, placement);
            let track: f64 = routing
                .segments
                .iter()
                .map(|s| {
                    let dx = synth_geometry::nm_to_mm(s.end.x_nm - s.start.x_nm);
                    let dy = synth_geometry::nm_to_mm(s.end.y_nm - s.start.y_nm);
                    dx.hypot(dy)
                })
                .sum();
            println!(
                "  {index}: {:.2}x{:.2} mm ({:.0} mm2) hpwl {:.1} | unrouted {} | copper {} | wire {:.1}",
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
    }
}
