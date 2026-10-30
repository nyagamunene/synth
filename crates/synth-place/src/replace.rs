// SPDX-License-Identifier: Apache-2.0

//! Full RePlAce-style global placement: weighted-average wirelength plus
//! electrostatic density, optimised with Nesterov's accelerated gradient.
//!
//! The output is a continuous [`Placement`]; the caller legalises it with the
//! displacement-minimising legalizer in `lib.rs`. This module is deliberately
//! independent of the CEM seed and of the quadratic hint path; it only borrows
//! the quadratic initial placement and the PCB pad-offset lookups.

use std::collections::HashMap;

use synth_geometry::{mm_to_nm, nm_to_mm, Layer, Point, Rect, Rotation};
use synth_ir::{Board, ComponentId};

use crate::analytic::initial_positions;
use crate::floorplan::FloorplanTarget;
use crate::replace_params::RePlAceParams;
use crate::{ComponentPlacement, PadOffsetLookup, PinnedPlacements, Placement};

/// Continuous RePlAce output plus the frozen mask aligned with its components.
pub(crate) struct ReplacePlacement {
    pub placement: Placement,
    /// True for an anchored component.
    pub frozen: Vec<bool>,
}

struct Pin {
    component: usize,
    offset: (f64, f64),
}

#[allow(clippy::implicit_hasher, clippy::too_many_arguments)]
pub(crate) fn continuous_replace(
    board: &Board,
    outline: Rect,
    usable: Rect,
    baseline: &Placement,
    fp_targets: &HashMap<ComponentId, FloorplanTarget>,
    rotation_overrides: &HashMap<ComponentId, Rotation>,
    pinned: &PinnedPlacements,
    courtyard_lookup: &HashMap<ComponentId, (f64, f64)>,
    pad_offsets: &PadOffsetLookup,
    params: RePlAceParams,
) -> ReplacePlacement {
    let (ids, centres, rotations, free) = initial_positions(
        board,
        usable,
        baseline,
        fp_targets,
        rotation_overrides,
        pinned,
        courtyard_lookup,
        pad_offsets,
    );
    let index: HashMap<ComponentId, usize> = ids
        .iter()
        .enumerate()
        .map(|(position, id)| (*id, position))
        .collect();
    let nets = build_nets(board, &index, &rotations, pad_offsets);
    let sizes: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| courtyard_lookup.get(id).copied().unwrap_or((10.0, 10.0)))
        .collect();
    let frozen: Vec<bool> = free.iter().map(|is_free| !is_free).collect();
    let mut positions: Vec<(f64, f64)> = centres
        .iter()
        .map(|point| (nm_to_mm(point.x_nm), nm_to_mm(point.y_nm)))
        .collect();
    let edges: Vec<f64> = ids
        .iter()
        .map(|id| {
            board.component(*id).map_or(0.5, |component| {
                nm_to_mm(crate::courtyard_edge_inset_nm(component))
            })
        })
        .collect();
    let bounds = Bounds {
        min_x: nm_to_mm(usable.min.x_nm),
        min_y: nm_to_mm(usable.min.y_nm),
        max_x: nm_to_mm(usable.max.x_nm),
        max_y: nm_to_mm(usable.max.y_nm),
    };
    for position in 0..positions.len() {
        positions[position] = clamp_position(
            positions[position],
            position,
            &sizes,
            &rotations,
            &edges,
            &bounds,
            &frozen,
        );
    }

    let mut current = positions.clone();
    let mut previous = positions.clone();
    let mut t = 1.0_f64;
    let total_area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let grid = DensityGrid::new(bounds, params.bin_count);
    for stage in 0..params.stages {
        let stage_index = i32::try_from(stage).unwrap_or(0);
        let gamma =
            (params.gamma_start_mm * params.gamma_decay.powi(stage_index)).max(params.gamma_end_mm);
        let lambda = params.lambda_start * params.lambda_growth.powi(stage_index);
        let self_forces: Vec<(f64, f64)> = (0..current.len())
            .map(|index| grid.self_force(current[index], sizes[index], total_area))
            .collect();
        for _iteration in 0..params.iterations_per_stage {
            let t_next = 0.5 * (1.0 + (1.0 + 4.0 * t * t).sqrt());
            let momentum = ((t - 1.0) / t_next).max(0.0);
            let mut momentum_positions = current.clone();
            for position in 0..momentum_positions.len() {
                if frozen[position] {
                    continue;
                }
                let dx = (current[position].0 - previous[position].0) * momentum;
                let dy = (current[position].1 - previous[position].1) * momentum;
                momentum_positions[position] = (current[position].0 + dx, current[position].1 + dy);
                momentum_positions[position] = clamp_position(
                    momentum_positions[position],
                    position,
                    &sizes,
                    &rotations,
                    &edges,
                    &bounds,
                    &frozen,
                );
            }
            let mut gradient = vec![(0.0_f64, 0.0_f64); momentum_positions.len()];
            let _ = wa_gradient(
                &nets,
                &momentum_positions,
                &frozen,
                gamma,
                params.net_weight,
                &mut gradient,
            );
            let density = grid.density_gradient(
                &momentum_positions,
                &sizes,
                &frozen,
                total_area,
                &self_forces,
            );
            if std::env::var_os("SYNTH_DEBUG_REPLACE").is_some() && stage == 0 && _iteration == 0 {
                let max = density
                    .iter()
                    .zip(&frozen)
                    .filter(|(_, is_frozen)| !**is_frozen)
                    .map(|(force, _)| force.0.abs().max(force.1.abs()))
                    .fold(0.0_f64, f64::max);
                eprintln!(
                    "REPLACE density max free force {max:.4} first {:?}",
                    density.first()
                );
            }
            for position in 0..gradient.len() {
                if frozen[position] {
                    continue;
                }
                gradient[position].0 += lambda * density[position].0;
                gradient[position].1 += lambda * density[position].1;
            }
            let mut next = momentum_positions.clone();
            for position in 0..next.len() {
                if frozen[position] {
                    continue;
                }
                let dx = (gradient[position].0 * params.step_mm)
                    .clamp(-params.max_step_mm, params.max_step_mm);
                let dy = (gradient[position].1 * params.step_mm)
                    .clamp(-params.max_step_mm, params.max_step_mm);
                next[position] = (
                    momentum_positions[position].0 - dx,
                    momentum_positions[position].1 - dy,
                );
                next[position] = clamp_position(
                    next[position],
                    position,
                    &sizes,
                    &rotations,
                    &edges,
                    &bounds,
                    &frozen,
                );
            }
            previous = current;
            current = next;
            t = t_next;
        }
    }

    let layers: HashMap<ComponentId, Layer> = baseline
        .components
        .iter()
        .map(|placed| (placed.id, placed.layer))
        .collect();
    let components = ids
        .iter()
        .enumerate()
        .map(|(position, id)| ComponentPlacement {
            id: *id,
            center: Point::new(mm_to_nm(current[position].0), mm_to_nm(current[position].1)),
            rotation: rotations[position],
            layer: layers.get(id).copied().unwrap_or(Layer::Top),
        })
        .collect();
    ReplacePlacement {
        placement: Placement {
            board_outline: outline,
            components,
        },
        frozen,
    }
}

fn build_nets(
    board: &Board,
    index: &HashMap<ComponentId, usize>,
    rotations: &[Rotation],
    pad_offsets: &PadOffsetLookup,
) -> Vec<Vec<Pin>> {
    let mut nets = Vec::new();
    for net in &board.nets {
        if net.endpoints.len() < 2 {
            continue;
        }
        let pins: Vec<Pin> = net
            .endpoints
            .iter()
            .filter_map(|endpoint| {
                let &component = index.get(&endpoint.component)?;
                let (offset_x, offset_y) = pad_offsets
                    .lookup(endpoint.component, endpoint.pin.0 as usize)
                    .unwrap_or((0, 0));
                let (rotated_x, rotated_y) = rotations[component].rotate_offset(offset_x, offset_y);
                Some(Pin {
                    component,
                    offset: (nm_to_mm(rotated_x), nm_to_mm(rotated_y)),
                })
            })
            .collect();
        if pins.len() >= 2 {
            nets.push(pins);
        }
    }
    nets
}

#[derive(Clone, Copy)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

#[allow(clippy::too_many_arguments)]
fn clamp_position(
    position: (f64, f64),
    index: usize,
    sizes: &[(f64, f64)],
    rotations: &[Rotation],
    edges: &[f64],
    bounds: &Bounds,
    frozen: &[bool],
) -> (f64, f64) {
    if frozen[index] {
        return position;
    }
    let (width, height) = sizes[index];
    let (half_w, half_h) = if rotations[index].swaps_extents() {
        (height / 2.0, width / 2.0)
    } else {
        (width / 2.0, height / 2.0)
    };
    let inset = edges[index];
    let min_x = bounds.min_x + half_w + inset;
    let max_x = bounds.max_x - half_w - inset;
    let min_y = bounds.min_y + half_h + inset;
    let max_y = bounds.max_y - half_h - inset;
    let x = if min_x > max_x {
        (min_x + max_x) / 2.0
    } else {
        position.0.clamp(min_x, max_x)
    };
    let y = if min_y > max_y {
        (min_y + max_y) / 2.0
    } else {
        position.1.clamp(min_y, max_y)
    };
    (x, y)
}

/// Weighted-average wirelength and its gradient. Returns the objective value;
/// the gradient is accumulated into `gradient`.
fn wa_gradient(
    nets: &[Vec<Pin>],
    positions: &[(f64, f64)],
    frozen: &[bool],
    gamma: f64,
    net_weight: f64,
    gradient: &mut [(f64, f64)],
) -> f64 {
    let gamma = gamma.max(1.0e-9);
    let mut objective = 0.0;
    for net in nets {
        if net.len() < 2 {
            continue;
        }
        for axis in 0..2 {
            let values: Vec<f64> = if axis == 0 {
                net.iter()
                    .map(|pin| positions[pin.component].0 + pin.offset.0)
                    .collect()
            } else {
                net.iter()
                    .map(|pin| positions[pin.component].1 + pin.offset.1)
                    .collect()
            };
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let p: Vec<f64> = values
                .iter()
                .map(|value| ((value - max) / gamma).exp())
                .collect();
            let q: Vec<f64> = values
                .iter()
                .map(|value| ((min - value) / gamma).exp())
                .collect();
            let sum_p: f64 = p.iter().sum();
            let sum_q: f64 = q.iter().sum();
            if sum_p <= 0.0 || sum_q <= 0.0 {
                continue;
            }
            let weighted_p: f64 = p.iter().zip(&values).map(|(w, value)| w * value).sum();
            let weighted_q: f64 = q.iter().zip(&values).map(|(w, value)| w * value).sum();
            let average_p = weighted_p / sum_p;
            let average_q = weighted_q / sum_q;
            objective += net_weight * (average_p - average_q);
            for (pin_index, pin) in net.iter().enumerate() {
                if frozen[pin.component] {
                    continue;
                }
                let p_i = p[pin_index] / sum_p;
                let q_i = q[pin_index] / sum_q;
                let derivative = p_i * (1.0 + (values[pin_index] - average_p) / gamma)
                    - q_i * (1.0 - (values[pin_index] - average_q) / gamma);
                if axis == 0 {
                    gradient[pin.component].0 += net_weight * derivative;
                } else {
                    gradient[pin.component].1 += net_weight * derivative;
                }
            }
        }
    }
    objective
}

/// Electrostatic density on a square bin grid. The charge is the component
/// area spread by the piecewise-quadratic kernel; the potential solves the
/// Neumann Poisson equation with a direct deterministic DCT.
struct DensityGrid {
    m: usize,
    min_x: f64,
    min_y: f64,
    bin_w: f64,
    bin_h: f64,
    omega: Vec<f64>,
    cos: Vec<Vec<f64>>,
    sin: Vec<Vec<f64>>,
}

impl DensityGrid {
    fn new(bounds: Bounds, m: usize) -> Self {
        let m = m.max(8);
        let bin_w = (bounds.max_x - bounds.min_x) / m as f64;
        let bin_h = (bounds.max_y - bounds.min_y) / m as f64;
        let omega: Vec<f64> = (0..m)
            .map(|u| std::f64::consts::TAU * u as f64 / m as f64)
            .collect();
        let cos = (0..m)
            .map(|u| (0..m).map(|l| (omega[u] * l as f64).cos()).collect())
            .collect();
        let sin = (0..m)
            .map(|u| (0..m).map(|l| (omega[u] * l as f64).sin()).collect())
            .collect();
        Self {
            m,
            min_x: bounds.min_x,
            min_y: bounds.min_y,
            bin_w: bin_w.max(1.0e-6),
            bin_h: bin_h.max(1.0e-6),
            omega,
            cos,
            sin,
        }
    }

    /// Add one component's area to the charge grid, normalised so the kernel
    /// sums to the courtyard area.
    fn add_charge(&self, charge: &mut [f64], position: (f64, f64), size: (f64, f64)) {
        let m = self.m;
        let (width, height) = size;
        let area = width * height;
        if area <= 0.0 {
            return;
        }
        let cx = (position.0 - self.min_x) / self.bin_w - 0.5;
        let cy = (position.1 - self.min_y) / self.bin_h - 0.5;
        let kernel_w = width.max(self.bin_w);
        let kernel_h = height.max(self.bin_h);
        let span_x = (kernel_w / self.bin_w).ceil() as i64 + 1;
        let span_y = (kernel_h / self.bin_h).ceil() as i64 + 1;
        let mut weights = Vec::new();
        let mut sum = 0.0;
        for dx in -span_x..=span_x {
            for dy in -span_y..=span_y {
                let l = cx.round() as i64 + dx;
                let j = cy.round() as i64 + dy;
                if l < 0 || j < 0 || l >= m as i64 || j >= m as i64 {
                    continue;
                }
                let theta_x = kernel((l as f64 + 0.5 - cx).abs() * self.bin_w, kernel_w);
                let theta_y = kernel((j as f64 + 0.5 - cy).abs() * self.bin_h, kernel_h);
                let weight = theta_x * theta_y;
                if weight > 0.0 {
                    weights.push((l as usize, j as usize, weight));
                    sum += weight;
                }
            }
        }
        if sum <= 0.0 {
            let l = (cx.round().clamp(0.0, (m - 1) as f64)) as usize;
            let j = (cy.round().clamp(0.0, (m - 1) as f64)) as usize;
            charge[l * m + j] += area;
        } else {
            for (l, j, weight) in weights {
                charge[l * m + j] += area * weight / sum;
            }
        }
    }

    fn charge(&self, positions: &[(f64, f64)], sizes: &[(f64, f64)], total_area: f64) -> Vec<f64> {
        let m = self.m;
        let mut charge = vec![0.0_f64; m * m];
        for (position, size) in positions.iter().zip(sizes) {
            self.add_charge(&mut charge, *position, *size);
        }
        let uniform = total_area / (m * m) as f64;
        for value in &mut charge {
            *value -= uniform;
        }
        charge
    }

    /// Potential gradient grids from a zero-mean charge grid.
    fn field(&self, charge: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let m = self.m;
        let mut row = vec![0.0_f64; m * m];
        for u in 0..m {
            for j in 0..m {
                let mut sum = 0.0;
                for l in 0..m {
                    sum += self.cos[u][l] * charge[l * m + j];
                }
                row[u * m + j] = sum;
            }
        }
        let mut rho_hat = vec![0.0_f64; m * m];
        for v in 0..m {
            for u in 0..m {
                let mut sum = 0.0;
                for j in 0..m {
                    sum += self.cos[v][j] * row[u * m + j];
                }
                rho_hat[u * m + v] = sum / (m * m) as f64;
            }
        }
        let mut psi_hat = vec![0.0_f64; m * m];
        for u in 0..m {
            for v in 0..m {
                if u == 0 && v == 0 {
                    continue;
                }
                psi_hat[u * m + v] = rho_hat[u * m + v]
                    / (self.omega[u] * self.omega[u] + self.omega[v] * self.omega[v]);
            }
        }
        let mut for_x = vec![0.0_f64; m * m];
        let mut for_y = vec![0.0_f64; m * m];
        for u in 0..m {
            for j in 0..m {
                let mut inner_x = 0.0;
                let mut inner_y = 0.0;
                for v in 0..m {
                    let value = psi_hat[u * m + v];
                    inner_x += value * self.cos[v][j];
                    inner_y += value * -self.omega[v] * self.sin[v][j];
                }
                for_x[u * m + j] = inner_x;
                for_y[u * m + j] = inner_y;
            }
        }
        for l in 0..m {
            for j in 0..m {
                let mut dx = 0.0;
                let mut dy = 0.0;
                for u in 0..m {
                    dx += -self.omega[u] * self.sin[u][l] * for_x[u * m + j];
                    dy += self.cos[u][l] * for_y[u * m + j];
                }
                for_x[l * m + j] = dx / self.bin_w;
                for_y[l * m + j] = dy / self.bin_h;
            }
        }
        (for_x, for_y)
    }

    /// The force a component's own charge produces at its centre. It is
    /// subtracted from the total field so a symmetric kernel does not move
    /// itself (the discrete DCT leaves a small self-force).
    fn self_force(&self, position: (f64, f64), size: (f64, f64), total_area: f64) -> (f64, f64) {
        if total_area <= 0.0 {
            return (0.0, 0.0);
        }
        let m = self.m;
        let mut charge = vec![0.0_f64; m * m];
        self.add_charge(&mut charge, position, size);
        let (dx, dy) = self.field(&charge);
        let (gx, gy) = self.sample(&dx, &dy, position);
        let area = size.0 * size.1;
        (area * gx / total_area, area * gy / total_area)
    }

    fn density_gradient(
        &self,
        positions: &[(f64, f64)],
        sizes: &[(f64, f64)],
        frozen: &[bool],
        total_area: f64,
        self_forces: &[(f64, f64)],
    ) -> Vec<(f64, f64)> {
        let mut gradient = vec![(0.0_f64, 0.0_f64); positions.len()];
        if total_area <= 0.0 {
            return gradient;
        }
        let charge = self.charge(positions, sizes, total_area);
        let (dx, dy) = self.field(&charge);
        for index in 0..positions.len() {
            if frozen[index] {
                continue;
            }
            let (gx, gy) = self.sample(&dx, &dy, positions[index]);
            let area = sizes[index].0 * sizes[index].1;
            gradient[index] = (
                area * gx / total_area - self_forces[index].0,
                area * gy / total_area - self_forces[index].1,
            );
        }
        // The Neumann field must have no net force; a residual global drift
        // from the discrete solve is removed so it cannot translate the board.
        let free_count = frozen.iter().filter(|&&is_frozen| !is_frozen).count();
        if free_count > 0 {
            let mean_x: f64 = gradient
                .iter()
                .zip(frozen)
                .filter(|(_, is_frozen)| !**is_frozen)
                .map(|(force, _)| force.0)
                .sum::<f64>()
                / free_count as f64;
            let mean_y: f64 = gradient
                .iter()
                .zip(frozen)
                .filter(|(_, is_frozen)| !**is_frozen)
                .map(|(force, _)| force.1)
                .sum::<f64>()
                / free_count as f64;
            for (force, is_frozen) in gradient.iter_mut().zip(frozen) {
                if !*is_frozen {
                    force.0 -= mean_x;
                    force.1 -= mean_y;
                }
            }
        }
        // The DCT potential is in charge units; normalise the free-force
        // magnitude so lambda is a dimensionless balance against WA.
        let max_force = gradient
            .iter()
            .zip(frozen)
            .filter(|(_, is_frozen)| !**is_frozen)
            .map(|(force, _)| force.0.abs().max(force.1.abs()))
            .fold(0.0_f64, f64::max);
        if max_force > 1.0e-12 {
            for (force, is_frozen) in gradient.iter_mut().zip(frozen) {
                if !*is_frozen {
                    force.0 /= max_force;
                    force.1 /= max_force;
                }
            }
        }
        gradient
    }

    fn sample(&self, grid_x: &[f64], grid_y: &[f64], position: (f64, f64)) -> (f64, f64) {
        let m = self.m;
        let x = (position.0 - self.min_x) / self.bin_w - 0.5;
        let y = (position.1 - self.min_y) / self.bin_h - 0.5;
        let x = x.clamp(0.0, (m - 1) as f64);
        let y = y.clamp(0.0, (m - 1) as f64);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(m - 1);
        let y1 = (y0 + 1).min(m - 1);
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let lerp = |grid: &[f64]| {
            let a = grid[x0 * m + y0] * (1.0 - tx) + grid[x1 * m + y0] * tx;
            let b = grid[x0 * m + y1] * (1.0 - tx) + grid[x1 * m + y1] * tx;
            a * (1.0 - ty) + b * ty
        };
        (lerp(grid_x), lerp(grid_y))
    }
}

fn kernel(distance: f64, width: f64) -> f64 {
    let width = width.max(1.0e-9);
    if distance <= width / 2.0 {
        1.0 - 2.0 * distance * distance / (width * width)
    } else if distance <= width {
        2.0 * (distance - width) * (distance - width) / (width * width)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_pin_net() -> Vec<Pin> {
        vec![
            Pin {
                component: 0,
                offset: (0.0, 0.0),
            },
            Pin {
                component: 1,
                offset: (0.0, 0.0),
            },
        ]
    }

    #[test]
    fn wa_gradient_matches_finite_difference() {
        let nets = vec![two_pin_net()];
        let positions = [(10.0, 10.0), (20.0, 15.0)];
        let frozen = [false, false];
        let mut gradient = vec![(0.0, 0.0); 2];
        let baseline = wa_gradient(&nets, &positions, &frozen, 5.0, 1.0, &mut gradient);
        let step = 1.0e-4;
        for index in 0..2 {
            for axis in 0..2 {
                let mut moved = positions;
                if axis == 0 {
                    moved[index].0 += step;
                } else {
                    moved[index].1 += step;
                }
                let mut scratch = vec![(0.0, 0.0); 2];
                let value = wa_gradient(&nets, &moved, &frozen, 5.0, 1.0, &mut scratch);
                let finite = (value - baseline) / step;
                let analytic = if axis == 0 {
                    gradient[index].0
                } else {
                    gradient[index].1
                };
                assert!(
                    (finite - analytic).abs() < 1.0e-3,
                    "component {index} axis {axis}: finite {finite}, analytic {analytic}"
                );
            }
        }
    }

    #[test]
    fn density_has_no_self_force() {
        let bounds = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 20.0,
            max_y: 20.0,
        };
        let grid = DensityGrid::new(bounds, 32);
        let positions = [(10.0, 10.0)];
        let sizes = [(2.0, 2.0)];
        let frozen = [false];
        let self_forces = vec![grid.self_force(positions[0], sizes[0], 4.0)];
        let gradient = grid.density_gradient(&positions, &sizes, &frozen, 4.0, &self_forces);
        assert!(
            gradient[0].0.abs() < 1.0e-6 && gradient[0].1.abs() < 1.0e-6,
            "single part must have no self-force, got {:?}",
            gradient[0]
        );
    }

    #[test]
    fn density_pushes_two_parts_apart() {
        let bounds = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 20.0,
            max_y: 20.0,
        };
        let grid = DensityGrid::new(bounds, 32);
        let positions = [(9.0, 10.0), (11.0, 10.0)];
        let sizes = [(2.0, 2.0), (2.0, 2.0)];
        let frozen = [false, false];
        let total_area = 8.0;
        let self_forces: Vec<(f64, f64)> = positions
            .iter()
            .zip(&sizes)
            .map(|(position, size)| grid.self_force(*position, *size, total_area))
            .collect();
        let gradient = grid.density_gradient(&positions, &sizes, &frozen, total_area, &self_forces);
        assert!(
            gradient[0].0 < 0.0 && gradient[1].0 > 0.0,
            "parts must be pushed apart, got {gradient:?}"
        );
    }
}
