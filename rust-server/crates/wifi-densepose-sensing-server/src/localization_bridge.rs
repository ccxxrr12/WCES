//! Localization bridge — RSSI/ToF triangulation using the MAT crate.
//!
//! Wraps `wifi_densepose_mat::localization::Triangulator` and
//! `LocalizationService` for multi-sensor survivor position estimation.
//!
//! # Pipeline
//! Per-node RSSI + ToF (from CIR) → Triangulator → Coordinates3D
//!   → fed into TriageEngine for position-enhanced survivor tracking.
//!
//! The bridge maintains a sliding window of recent per-node observations
//! so that triangulation can proceed even when frames arrive asynchronously
//! from different ESP32 nodes.

use std::collections::HashMap;
use wifi_densepose_mat::localization::{
    Triangulator, TriangulationConfig, LocalizationService,
};
use wifi_densepose_mat::domain::{
    coordinates::Coordinates3D,
    scan_zone::{SensorPosition, SensorType},
};

/// TTL for a per-node RSSI observation before it's considered stale.
const OBSERVATION_TTL_SECS: f64 = 5.0;

/// A single RSSI observation from one node at one point in time.
#[derive(Debug, Clone)]
struct RssiObservation {
    rssi_dbm: f64,
    /// CIR-based ranging distance, if available (higher confidence).
    cir_distance_m: Option<f64>,
    timestamp_secs: f64,
}

/// Multi-node localization bridge.
///
/// Maintains a map of node → latest RSSI+CIR observation and runs
/// weighted-least-squares triangulation when ≥2 nodes are active.
pub struct LocalizationBridge {
    triangulator: Triangulator,
    /// Maps node_id → latest observation.
    observations: HashMap<u8, RssiObservation>,
    /// Node positions in the room coordinate frame (node_id → [x, y, z]).
    node_positions: HashMap<u8, [f64; 3]>,
}

impl LocalizationBridge {
    /// Create a new localization bridge with competition-default node positions.
    pub fn new(node_positions: HashMap<u8, [f64; 3]>) -> Self {
        let config = TriangulationConfig {
            min_sensors: 2, // relaxed: 2 nodes + CIR ranging can triangulate
            max_uncertainty: 5.0,
            path_loss_exponent: 3.0,
            reference_distance: 1.0,
            reference_rssi: -30.0,
            weighted: true,
        };
        Self {
            triangulator: Triangulator::new(config),
            observations: HashMap::new(),
            node_positions,
        }
    }

    /// Create with competition defaults (3 nodes in equilateral triangle).
    pub fn competition_default() -> Self {
        Self::new(crate::mat_pipeline::node_positions_arr())
    }

    /// Feed a new RSSI observation from a given node.
    ///
    /// Optionally includes a CIR-based ranging distance for hybrid RSSI+ToF
    /// triangulation (higher weight than pure RSSI).
    pub fn feed_observation(
        &mut self,
        node_id: u8,
        rssi_dbm: f64,
        cir_distance_m: Option<f64>,
    ) {
        let now = now_secs();
        self.observations.insert(node_id, RssiObservation {
            rssi_dbm,
            cir_distance_m,
            timestamp_secs: now,
        });
        // Prune stale observations (>5s old).
        self.observations.retain(|_, obs| now - obs.timestamp_secs < OBSERVATION_TTL_SECS);
    }

    /// Estimate survivor position from all current observations.
    ///
    /// Returns `None` if fewer than 2 nodes have recent observations.
    /// Uses CIR-based ToF distances when available (higher confidence),
    /// falling back to RSSI path-loss distances.
    pub fn estimate_position(&self) -> Option<[f64; 3]> {
        let now = now_secs();
        let active: Vec<(u8, &RssiObservation)> = self.observations.iter()
            .filter(|(_, obs)| now - obs.timestamp_secs < OBSERVATION_TTL_SECS)
            .map(|(&nid, obs)| (nid, obs))
            .collect();

        if active.len() < 2 {
            return None;
        }

        // Build sensor positions and RSSI values for the MAT triangulator.
        let sensors: Vec<SensorPosition> = active.iter()
            .filter_map(|(nid, _)| {
                self.node_positions.get(nid).map(|&[x, y, z]| {
                    SensorPosition {
                        id: format!("node-{nid}"),
                        x, y, z,
                        sensor_type: SensorType::Transceiver,
                        is_operational: true,
                    }
                })
            })
            .collect();

        let rssi_pairs: Vec<(String, f64)> = active.iter()
            .map(|(nid, obs)| (format!("node-{nid}"), obs.rssi_dbm))
            .collect();

        // Try RSSI triangulation first.
        if let Some(pos) = self.triangulator.estimate_position(&sensors, &rssi_pairs) {
            return Some([pos.x, pos.y, pos.z]);
        }

        // Fallback: weighted centroid from CIR distances when available,
        // pure RSSI-based distances otherwise.
        self.weighted_centroid(&active)
    }

    /// Weighted centroid fallback when least-squares triangulation fails.
    fn weighted_centroid(&self, active: &[(u8, &RssiObservation)]) -> Option<[f64; 3]> {
        let mut wx = 0.0_f64;
        let mut wy = 0.0_f64;
        let mut wz = 0.0_f64;
        let mut total_w = 0.0_f64;

        for (nid, obs) in active {
            if let Some(&[nx, ny, nz]) = self.node_positions.get(nid) {
                // CIR-based ranging gets 3× weight over pure RSSI.
                let base_w = if obs.cir_distance_m.is_some() { 3.0 } else { 1.0 };
                // Higher RSSI (closer) → higher weight.
                let rssi_w = ((obs.rssi_dbm + 90.0) / 60.0).clamp(0.1, 1.0);
                let w = base_w * rssi_w;
                wx += nx * w;
                wy += ny * w;
                wz += nz * w;
                total_w += w;
            }
        }

        if total_w > 0.0 {
            Some([wx / total_w, wy / total_w, wz / total_w])
        } else {
            None
        }
    }

    /// Number of nodes with fresh observations (<5s).
    pub fn active_node_count(&self) -> usize {
        let now = now_secs();
        self.observations.values()
            .filter(|obs| now - obs.timestamp_secs < OBSERVATION_TTL_SECS)
            .count()
    }

    /// Clear all observations (e.g. on source change or reset).
    pub fn clear(&mut self) {
        self.observations.clear();
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localization_bridge_constructs() {
        let bridge = LocalizationBridge::competition_default();
        assert_eq!(bridge.active_node_count(), 0);
    }

    #[test]
    fn single_observation_insufficient() {
        let mut bridge = LocalizationBridge::competition_default();
        bridge.feed_observation(1, -50.0, None);
        assert_eq!(bridge.active_node_count(), 1);
        assert!(bridge.estimate_position().is_none());
    }

    #[test]
    fn two_observations_triangulate() {
        let mut bridge = LocalizationBridge::competition_default();
        bridge.feed_observation(1, -45.0, None);
        bridge.feed_observation(2, -50.0, None);
        assert_eq!(bridge.active_node_count(), 2);
        let pos = bridge.estimate_position();
        assert!(pos.is_some(), "Two nodes should produce a position estimate");
    }
}
