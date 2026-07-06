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
        peaks.push(FieldPeak {
            position: cell_to_world(ix, iz, nx, nz),
            intensity: v,
        });
    }
    peaks
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
        let values = vec![0.1; 400];
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
}
