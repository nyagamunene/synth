// SPDX-License-Identifier: Apache-2.0

//! Tunable parameters for the RePlAce global-placement experiment.
//!
//! This file is intentionally the only place the autoresearch loop edits for
//! equation tuning. The solver reads [`RePlAceParams::default`]; the scorer and
//! corpus are frozen.
//!
//! Units are millimetres unless stated otherwise.

/// Which density force the Nesterov step uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DensityModel {
    /// Bin-overlap density (research note Option A), the stable first model.
    Bin,
    /// Poisson/electrostatic density on a DCT grid (research note Option B).
    Poisson,
}

/// One RePlAce run's equation knobs.
#[derive(Clone, Copy, Debug)]
pub struct RePlAceParams {
    /// Density force model.
    pub density_model: DensityModel,
    /// Density grid is `bin_count x bin_count` over the usable rectangle.
    pub bin_count: usize,
    /// Number of gamma/lambda stages.
    pub stages: usize,
    /// Nesterov iterations per stage.
    pub iterations_per_stage: usize,
    /// Initial weighted-average smoothing, in millimetres.
    pub gamma_start_mm: f64,
    /// Smallest smoothing value.
    pub gamma_end_mm: f64,
    /// Gamma is multiplied by this after every stage.
    pub gamma_decay: f64,
    /// Initial density weight.
    pub lambda_start: f64,
    /// Lambda is multiplied by this after every stage.
    pub lambda_growth: f64,
    /// Nesterov step per unit gradient, in millimetres.
    pub step_mm: f64,
    /// Largest single-coordinate movement per iteration, in millimetres.
    pub max_step_mm: f64,
    /// Weight of one net in the wirelength objective.
    pub net_weight: f64,
    /// Extra clearance the legalizer keeps around free parts, in millimetres.
    pub legalizer_gap_mm: f64,
}

impl Default for RePlAceParams {
    fn default() -> Self {
        Self {
            density_model: DensityModel::Bin,
            bin_count: 64,
            stages: 6,
            iterations_per_stage: 40,
            gamma_start_mm: 20.0,
            gamma_end_mm: 0.5,
            gamma_decay: 0.5,
            lambda_start: 1.0,
            lambda_growth: 2.0,
            step_mm: 0.2,
            max_step_mm: 2.0,
            net_weight: 1.0,
            legalizer_gap_mm: 0.0,
        }
    }
}
