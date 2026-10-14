// SPDX-License-Identifier: Apache-2.0

//! `kicad-cli pcb drc` as the independent physical check.
//!
//! Synth's own DRC engine reasons over the in-memory routing IR, which is
//! the right tool for the router it was written alongside but useless for
//! copper it did not produce. For an externally routed board the authority
//! is KiCad: it knows the board's real geometry, refills zones, and reports
//! shorts, clearances, and unrouted connections against the design's own
//! rules.
//!
//! The important behaviour here is what happens when KiCad is missing.
//! [`run`] returns `None` rather than an empty [`synth_drc::DrcCounts`], because "we
//! could not check" and "we checked and found nothing" must never look
//! the same to the release gate.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::contract::RouteRequest;

/// Budget for a DRC run. Zone refill on a dense board is not fast.
pub(crate) const DRC_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

/// Cap on the captured DRC output, in bytes.
pub(crate) const DRC_OUTPUT_CAP: usize = 1024 * 1024;

/// A machine-readable DRC finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrcFinding {
    /// KiCad's violation type, e.g. `clearance`.
    pub violation_type: String,
    /// KiCad's severity, `error` or `warning`.
    pub severity: String,
    pub description: String,
    /// Net numbers KiCad attributed the finding to, when it named any.
    #[serde(default)]
    pub nets: Vec<i64>,
    /// Component reference designators KiCad attributed it to.
    #[serde(default)]
    pub references: Vec<String>,
}

impl DrcFinding {
    /// Whether this finding blocks fabrication.
    ///
    /// Errors block; warnings and informational severities do not.
    /// `unconnected` is deliberately excluded: it is counted once, in
    /// [`DrcReport::unconnected_items`], and counting it here as well
    /// would double every unconnected pad in the blocking total.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        !matches!(self.severity.as_str(), "warning" | "ignore" | "unconnected")
    }
}

/// The result of running DRC over a candidate board.
///
/// [`Default`] is deliberately *not* clean: a report that was never
/// produced has to read as unavailable, or constructing one by accident
/// would certify a board nobody checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrcReport {
    pub findings: Vec<DrcFinding>,
    /// Pads KiCad reported as not connected to the rest of their net.
    pub unconnected_items: usize,
    /// Why DRC could not be run, when it could not.
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

impl Default for DrcReport {
    fn default() -> Self {
        Self {
            findings: Vec::new(),
            unconnected_items: 0,
            unavailable_reason: Some("DRC has not been run".to_string()),
        }
    }
}

impl DrcReport {
    /// Whether DRC ran and found nothing blocking.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unavailable_reason.is_none()
            && self.unconnected_items == 0
            && !self.findings.iter().any(DrcFinding::is_blocking)
    }

    /// Findings that block, plus every unconnected item.
    #[must_use]
    pub fn blocking_reasons(&self) -> Vec<String> {
        if let Some(reason) = &self.unavailable_reason {
            return vec![reason.clone()];
        }
        let mut reasons: Vec<String> = self
            .findings
            .iter()
            .filter(|f| f.is_blocking())
            .map(|f| format!("{}: {}", f.violation_type, f.description))
            .collect();
        if self.unconnected_items > 0 {
            reasons.push(format!(
                "kicad reported {} unconnected item(s)",
                self.unconnected_items
            ));
        }
        reasons
    }

    /// Counts in the shape the rest of Synth's gates already consume.
    #[must_use]
    pub fn counts(&self) -> synth_drc::DrcCounts {
        let errors = self.findings.iter().filter(|f| f.is_blocking()).count();
        // Unconnected items are reported in `unconnected_items` and already
        // carried in `findings`; adding them here as warnings too would make
        // one pad count twice in the same line.
        let warnings = self
            .findings
            .iter()
            .filter(|f| !f.is_blocking() && f.severity != "unconnected")
            .count();
        synth_drc::DrcCounts {
            errors,
            unconnected: self.unconnected_items,
            warnings,
        }
    }
}

/// Run DRC over `board_path`, writing the JSON report to the request's
/// declared location.
///
/// Returns `None` — never an empty report — when KiCad could not be run.
/// A caller that cannot distinguish those two cases would treat a missing
/// KiCad install as a clean board.
pub fn run(board_path: &Path, request: &RouteRequest) -> Option<synth_drc::DrcCounts> {
    let report = run_detailed(board_path, request);
    if report.unavailable_reason.is_some() {
        None
    } else {
        Some(report.counts())
    }
}

/// Run DRC and keep the findings, not just the counts.
#[must_use]
pub fn run_detailed(board_path: &Path, request: &RouteRequest) -> DrcReport {
    let cli = synth_drc::kicad_cli::binary();
    let report_path = request.drc_report_path();
    if let Some(parent) = report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let args = vec![
        "pcb".to_string(),
        "drc".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--severity-error".to_string(),
        "--severity-warning".to_string(),
        // Refill first: DRC against a board whose zones are still as
        // exported would report clearances to copper that is not there.
        "--refill-zones".to_string(),
        "--output".to_string(),
        report_path.display().to_string(),
        board_path.display().to_string(),
    ];

    let result = {
        let _staged = StagedProject::stage(&request.out_dir, &request.stem, board_path);
        crate::process::run(Path::new(&cli), &args, DRC_BUDGET, DRC_OUTPUT_CAP)
    };
    let invocation = match result {
        Ok(invocation) => invocation,
        Err(error) => {
            return DrcReport {
                unavailable_reason: Some(format!(
                    "kicad-cli pcb drc could not be run ({}); an unperformed DRC is \
                     not a clean DRC",
                    error.detail
                )),
                ..DrcReport::default()
            }
        }
    };

    if !invocation.succeeded() {
        return DrcReport {
            unavailable_reason: Some(format!(
                "kicad-cli pcb drc did not complete: {}",
                first_line(&invocation.combined_output())
            )),
            ..DrcReport::default()
        };
    }

    parse_report(&report_path).unwrap_or_else(|| DrcReport {
        unavailable_reason: Some(format!(
            "kicad-cli pcb drc wrote no readable report at {}",
            report_path.display()
        )),
        ..DrcReport::default()
    })
}

/// The export's project file and custom rules, copied next to a candidate board
/// for one DRC run.
///
/// `kicad-cli` reads both from files that share the board's name, and the
/// routed candidate is named differently from the export, so without this the
/// DRC runs on KiCad's own defaults, not the rules the export wrote. A file
/// already beside the board (left by an earlier run, or written by KiCad) is
/// overwritten for the run, so it cannot decide the rules, and is put back when
/// the guard is dropped; a file that was not there is removed.
struct StagedProject {
    /// Each staged path with what was there before, if anything.
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl StagedProject {
    fn stage(export_dir: &Path, export_stem: &str, board_path: &Path) -> Self {
        let files = ["kicad_pro", "kicad_dru"]
            .into_iter()
            .filter_map(|extension| {
                let source = export_dir.join(format!("{export_stem}.{extension}"));
                let staged = board_path.with_extension(extension);
                let rules = std::fs::read(&source).ok()?;
                let before = std::fs::read(&staged).ok();
                std::fs::write(&staged, rules).ok()?;
                Some((staged, before))
            })
            .collect();
        Self { files }
    }
}

impl Drop for StagedProject {
    fn drop(&mut self) {
        for (path, before) in &self.files {
            let _ = match before {
                Some(content) => std::fs::write(path, content),
                None => std::fs::remove_file(path),
            };
        }
    }
}

pub(crate) fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output");
    line.trim().to_string()
}

/// Parse KiCad's DRC JSON.
///
/// Deliberately tolerant of a missing or unexpected shape: a report that
/// cannot be understood is an unavailable check, not a clean one, so every
/// failure path here produces a reason rather than empty findings.
#[must_use]
pub fn parse_report(path: &Path) -> Option<DrcReport> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_report_text(&text)
}

/// Parse DRC JSON from text. Split out so it is testable without KiCad.
#[must_use]
pub fn parse_report_text(text: &str) -> Option<DrcReport> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    // A JSON document that is not an object is not a DRC report, even
    // though it parses. Reading it as "no findings" would turn an
    // unreadable report into a clean board.
    if !value.is_object() {
        return None;
    }

    // KiCad keys its sections by category name. Which section an entry
    // came from is what identifies it, because the section is the only
    // reliable discriminator: an unconnected item carries severity `error`
    // in current releases, so severity alone would report every
    // unconnected pad as a rule violation and lose the distinction the
    // counts depend on.
    let mut findings = Vec::new();
    let mut unconnected_items = 0usize;
    let mut unconnected_in_violations = 0usize;

    for key in ["violations", "unconnected_items"] {
        let is_unconnected_section = key == "unconnected_items";
        let Some(items) = value.get(key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        if is_unconnected_section {
            unconnected_items = items.len();
        }
        for item in items {
            let violation_type = item
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let severity = item
                .get("severity")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("error")
                .to_string();
            let description = item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let nets = item
                .get("nets")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_i64)
                        .collect()
                })
                .unwrap_or_default();
            let references = item
                .get("items")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|v| {
                            v.get("description")
                                .and_then(serde_json::Value::as_str)
                                .and_then(|d| d.split_once(", "))
                                .map(|(reference, _)| reference.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Mark it by section, not by the severity KiCad happened to
            // write, so `is_blocking` and the counts agree with each other
            // and with what the section actually holds.
            let severity = if is_unconnected_section {
                "unconnected".to_string()
            } else {
                if severity == "unconnected" {
                    unconnected_in_violations += 1;
                }
                severity
            };
            findings.push(DrcFinding {
                violation_type,
                severity,
                description,
                nets,
                references,
            });
        }
    }

    // An older release may report unconnected pads inside `violations`
    // with severity `unconnected` rather than in their own section; count
    // those too, so the total is right either way.
    unconnected_items += unconnected_in_violations;

    Some(DrcReport {
        findings,
        unconnected_items,
        unavailable_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staging_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("synth-drc-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("b.kicad_dru"), "rules").expect("dru");
        std::fs::write(dir.join("b.kicad_pro"), "project").expect("pro");
        dir
    }

    /// `kicad-cli` reads a board's project and custom rules from files that
    /// share the board's name. The candidate board has another name than the
    /// exported one, so the export's rules are staged beside it for the run.
    #[test]
    fn the_exports_rules_are_staged_beside_a_candidate_board() {
        let dir = staging_dir("stage");
        let board = dir.join("b.freerouting.kicad_pcb");
        let staged = StagedProject::stage(&dir, "b", &board);
        assert_eq!(
            std::fs::read_to_string(dir.join("b.freerouting.kicad_dru")).unwrap(),
            "rules"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("b.freerouting.kicad_pro")).unwrap(),
            "project"
        );
        drop(staged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sidecar left beside the candidate by an earlier run (or by KiCad) must
    /// not decide the rules: this run's export overwrites it, and whatever was
    /// there is put back afterwards. Files that were not there are removed.
    #[test]
    fn a_stale_sidecar_is_overwritten_for_the_run_and_restored_after() {
        let dir = staging_dir("stale");
        std::fs::write(dir.join("b.freerouting.kicad_dru"), "stale rules").unwrap();
        let board = dir.join("b.freerouting.kicad_pcb");

        let staged = StagedProject::stage(&dir, "b", &board);
        assert_eq!(
            std::fs::read_to_string(dir.join("b.freerouting.kicad_dru")).unwrap(),
            "rules",
            "the run must see this export's rules, not the stale file"
        );
        drop(staged);

        assert_eq!(
            std::fs::read_to_string(dir.join("b.freerouting.kicad_dru")).unwrap(),
            "stale rules",
            "a file that was there before is put back"
        );
        assert!(
            !dir.join("b.freerouting.kicad_pro").exists(),
            "a file that was not there before is removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_report_is_clean() {
        let report =
            parse_report_text(r#"{"violations": [], "unconnected_items": []}"#).expect("parses");
        assert!(report.is_clean());
        assert_eq!(report.counts(), synth_drc::DrcCounts::default());
    }

    #[test]
    fn an_error_blocks_and_a_warning_does_not() {
        let report = parse_report_text(
            r#"{"violations": [
                {"type": "clearance", "severity": "error", "description": "too close"},
                {"type": "silk_over_copper", "severity": "warning", "description": "overlap"}
            ]}"#,
        )
        .expect("parses");
        assert!(!report.is_clean());
        assert_eq!(report.counts().errors, 1);
        assert_eq!(report.counts().warnings, 1);
        assert_eq!(report.blocking_reasons().len(), 1);
        assert!(report.blocking_reasons()[0].contains("too close"));
    }

    #[test]
    fn an_unconnected_section_is_counted_as_unconnected_not_as_a_violation() {
        // KiCad gives an unconnected pad severity `error` inside its own
        // section. Reading severity alone would count each one as a rule
        // violation, so the two counts disagree with the report and the
        // operator is told to fix a clearance problem they do not have.
        let report = parse_report_text(
            r#"{"violations": [],
               "unconnected_items": [
                 {"type": "unconnected_items", "severity": "error", "description": "Missing connection"},
                 {"type": "unconnected_items", "severity": "error", "description": "Missing connection"}
               ]}"#,
        )
        .expect("parses");
        assert_eq!(report.unconnected_items, 2);
        assert_eq!(report.counts().errors, 0, "not rule violations");
        assert_eq!(report.counts().unconnected, 2);
        assert_eq!(report.counts().blocking_count(), 2);
        assert!(!report.is_clean(), "an unconnected pad still blocks");
    }

    #[test]
    fn an_unconnected_item_is_not_also_counted_as_a_warning() {
        // It is reported once, in the unconnected column. Counting it in the
        // warning column too makes one pad appear in the same summary line
        // twice, and the total no longer matches the number of pads.
        let report = parse_report_text(
            r#"{"violations": [
                 {"type": "silk_over_copper", "severity": "warning", "description": "overlap"}
               ],
               "unconnected_items": [
                 {"type": "unconnected_items", "severity": "error", "description": "Missing connection"},
                 {"type": "unconnected_items", "severity": "error", "description": "Missing connection"}
               ]}"#,
        )
        .expect("parses");
        assert_eq!(report.counts().warnings, 1, "just the real warning");
        assert_eq!(report.counts().unconnected, 2);
    }

    #[test]
    fn a_violation_and_unconnected_items_are_counted_separately() {
        let report = parse_report_text(
            r#"{"violations": [{"type": "clearance", "severity": "error",
                                "description": "too close"}],
               "unconnected_items": [
                 {"type": "unconnected_items", "severity": "error", "description": "Missing connection"}
               ]}"#,
        )
        .expect("parses");
        assert_eq!(report.counts().errors, 1);
        assert_eq!(report.counts().unconnected, 1);
        assert_eq!(report.blocking_reasons().len(), 2);
    }

    #[test]
    fn an_unconnected_item_blocks() {
        let report = parse_report_text(
            r#"{"violations": [{"type": "unconnected_items", "severity": "unconnected",
               "description": "Pad not connected"}]}"#,
        )
        .expect("parses");
        assert_eq!(report.unconnected_items, 1);
        assert_eq!(report.counts().unconnected, 1);
        assert_eq!(report.counts().blocking_count(), 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn nets_and_references_are_carried_through() {
        // Without attribution a routed-board finding cannot be traced back
        // to a net or a part, which is what makes it actionable.
        let report = parse_report_text(
            r#"{"violations": [{"type": "clearance", "severity": "error",
                "description": "Net conflict", "nets": [3, 4],
                "items": [{"description": "U1, pin 3"}, {"description": "R2, pin 1"}]}]}"#,
        )
        .expect("parses");
        assert_eq!(report.findings[0].nets, vec![3, 4]);
        assert_eq!(report.findings[0].references, vec!["U1", "R2"]);
    }

    #[test]
    fn a_report_that_cannot_be_read_is_unavailable_rather_than_clean() {
        // The distinction the whole module exists to preserve.
        assert!(parse_report_text("not json").is_none());
        assert!(parse_report_text("[]").is_none());
        assert!(!DrcReport::default().is_clean());
    }

    #[test]
    fn an_unavailable_check_reports_its_reason_as_a_blocker() {
        let report = DrcReport {
            unavailable_reason: Some("kicad-cli pcb drc could not be run".to_string()),
            ..DrcReport::default()
        };
        assert!(!report.is_clean());
        assert_eq!(report.blocking_reasons().len(), 1);
        assert!(report.blocking_reasons()[0].contains("could not be run"));
    }

    #[test]
    fn a_missing_severity_is_treated_as_blocking() {
        // Defaulting to "error" is the fail-closed choice: an unlabelled
        // finding must not be assumed harmless.
        let report = parse_report_text(
            r#"{"violations": [{"type": "mystery", "description": "no severity"}]}"#,
        )
        .expect("parses");
        assert!(report.findings[0].is_blocking());
    }
}
