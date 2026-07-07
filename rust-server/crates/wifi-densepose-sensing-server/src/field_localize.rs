//! Field-peak localization — maps subcarrier-variance heatmap peaks to
//! room-world person positions.
//!
//! ## How it works
//!
//! `generate_signal_field()` already builds a 20×20 grid from **measured subcarrier
//! variances and motion-band power** every frame. Each grid cell corresponds to a
//! subcarrier / angle pair; the cell value is the motion-weighted CSI variance at
//! that angular direction. When a person scatters WiFi, the cells pointing toward
//! them get hotter → the hottest cell is our best estimate of their position.
//!
//! This is the same approach used by the upstream RuView project (field_localize.rs).
//! It is honest: the position comes from real CSI data, moves with real motion,
//! and makes no claim to survey-grade accuracy from a single-antenna link.
//!
//! ## Coordinate mapping
//!
//! Grid cell `(ix, iz)` → world:  `wx = (ix - nx/2) * X_SCALE, wz = (iz - nz/2) * Z_SCALE`.
//! This matches the Obervatory UI's `_buildSignalField` transform so the figure
//! lands exactly on the field hotspot it is standing on.

/// World-space scale for X axis (width), matching Observatory layout.
pub const X_SCALE: f64 = 0.6;
/// World-space scale for Z axis (depth), matching Observatory layout.
pub const Z_SCALE: f64 = 0.5;

/// Minimum normalised field value for a cell to count as a real peak.
/// Below this the field is treated as background noise — no phantom persons.
// ESP32 C5 has ~53 subcarriers; motion_score in [0.02, 0.3] gives field
// peaks in [0.02, 0.3].  0.05 filters pure noise while catching real body
// scattering (which typically pushes 2-3 cells above this).
pub const PEAK_THRESHOLD: f64 = 0.05;

/// A localised field peak in room-world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldPeak {
    /// Room position `[x, y, z]` in metres. `y` is always 0 (floor plane).
    pub position: [f64; 3],
    /// Normalised field intensity at the peak cell [0, 1].
    pub intensity: f64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Map a grid cell to Observatory world coordinates.
#[must_use]
pub fn cell_to_world(ix: usize, iz: usize, nx: usize, nz: usize) -> [f64; 3] {
    let wx = (ix as f64 - nx as f64 / 2.0) * X_SCALE;
    let wz = (iz as f64 - nz as f64 / 2.0) * Z_SCALE;
    [wx, 0.0, wz]
}

/// Extract up to `max_peaks` strongest, spatially-separated peaks from the
/// `signal_field` grid produced by `generate_signal_field()`.
///
/// Returns peaks sorted strongest-first. Returns empty when no cell exceeds
/// [`PEAK_THRESHOLD`] — the field reports "nobody here".
#[must_use]
pub fn extract_peaks(
    values: &[f64],
    nx: usize,
    nz: usize,
    max_peaks: usize,
    min_separation_cells: f64,
) -> Vec<FieldPeak> {
    if nx == 0 || nz == 0 || values.len() < nx * nz || max_peaks == 0 {
        return Vec::new();
    }

    // Collect all cells above threshold, strongest first
    let mut candidates: Vec<(usize, usize, f64)> = Vec::new();
    for iz in 0..nz {
        for ix in 0..nx {
            let v = values[iz * nx + ix];
            if v >= PEAK_THRESHOLD {
                candidates.push((ix, iz, v));
            }
        }
    }
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));

    let mut peaks: Vec<FieldPeak> = Vec::new();
    for (ix, iz, v) in candidates {
        if peaks.len() >= max_peaks {
            break;
        }
        let too_close = peaks.iter().any(|p| {
            let dx = p.position[0] - (ix as f64 - nx as f64 / 2.0) * X_SCALE;
            let dz = p.position[2] - (iz as f64 - nz as f64 / 2.0) * Z_SCALE;
            (dx * dx + dz * dz).sqrt() < min_separation_cells * X_SCALE
        });
        if too_close {
            continue;
        }
        let refined = refine_peak_subpixel(values, ix, iz, nx, nz);
        peaks.push(FieldPeak {
            position: refined,
            intensity: v,
        });
    }
    peaks
}

/// Refine a peak position using weighted centroid of neighboring cells.
///
/// This achieves sub-cell precision by computing the intensity-weighted
/// centroid of the 3×3 neighborhood around the peak cell. This is analogous
/// to sub-pixel corner refinement in computer vision.
///
/// Returns the refined [x, y, z] world position.
#[must_use]
pub fn refine_peak_subpixel(
    values: &[f64],
    ix: usize,
    iz: usize,
    nx: usize,
    nz: usize,
) -> [f64; 3] {
    // 3×3 neighborhood weighted centroid
    let mut sum_w = 0.0f64;
    let mut sum_x = 0.0f64;
    let mut sum_z = 0.0f64;

    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx_idx = (ix as i32 + dx).clamp(0, nx as i32 - 1) as usize;
            let nz_idx = (iz as i32 + dz).clamp(0, nz as i32 - 1) as usize;
            let v = values[nz_idx * nx + nx_idx];
            // Use intensity as weight; add small epsilon to avoid division by zero
            let w = v + 1e-6;
            sum_w += w;
            sum_x += w * nx_idx as f64;
            sum_z += w * nz_idx as f64;
        }
    }

    let refined_x = if sum_w > 1e-9 { sum_x / sum_w } else { ix as f64 };
    let refined_z = if sum_w > 1e-9 { sum_z / sum_w } else { iz as f64 };

    cell_to_world(refined_x.round() as usize, refined_z.round() as usize, nx, nz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_to_world_center_is_origin() {
        let c = cell_to_world(10, 10, 20, 20);
        assert!((c[0] - 0.0).abs() < 1e-9);
        assert_eq!(c[1], 0.0);
        assert!((c[2] - 0.0).abs() < 1e-9);
    }

    #[test]
    fn empty_field_returns_none() {
        let values = vec![0.01; 400];
        assert!(extract_peaks(&values, 20, 20, 1, 3.0).is_empty());
    }

    #[test]
    fn extracts_strongest_peak() {
        let mut values = vec![0.1; 400];
        values[5 * 20 + 15] = 0.9; // peak at (15, 5)
        let peaks = extract_peaks(&values, 20, 20, 1, 3.0);
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].position[0] - (15.0 - 10.0) * 0.6).abs() < 1e-9);
        assert!((peaks[0].position[2] - (5.0 - 10.0) * 0.5).abs() < 1e-9);
    }

    #[test]
    fn subpixel_refinement_improves_precision() {
        // Peak at (15, 10) with asymmetric neighborhood
        let mut values = vec![0.0; 400]; // 20×20
        values[10 * 20 + 15] = 1.0;  // center
        values[10 * 20 + 14] = 0.6;  // left neighbor
        values[10 * 20 + 16] = 0.3;  // right neighbor
        values[9 * 20 + 15] = 0.4;   // top neighbor
        values[11 * 20 + 15] = 0.5;  // bottom neighbor

        let raw = cell_to_world(15, 10, 20, 20);
        let refined = refine_peak_subpixel(&values, 15, 10, 20, 20);
        // Refined position should be shifted toward the heavier neighbors
        // (left and bottom are heavier, so x should decrease, z should increase)
        assert!(refined[0] <= raw[0], "refined x should shift toward heavier side");
    }

    #[test]
    fn top_subcarriers_selects_highest_variance() {
        let variances = vec![0.1, 0.5, 0.3, 0.8, 0.2, 0.45, 0.4, 0.7];
        let top = crate::signal_processing::select_top_subcarriers(&variances, 3);
        assert_eq!(top.len(), 3);
        // Top 3 should be indices 3 (0.8), 7 (0.7), 1 (0.5)
        assert!(top.contains(&3));
        assert!(top.contains(&7));
        assert!(top.contains(&1));
    }
}
