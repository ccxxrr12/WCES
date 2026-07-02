//! Tracking bridge — Kalman-filtered survivor tracking using the MAT crate.
//!
//! Wraps `wifi_densepose_mat::tracking::SurvivorTracker` (Kalman 6-D CV model,
//! CSI fingerprint biometric re-ID, Tentative→Active→Lost→Terminated lifecycle).
//!
//! # Relationship to mat_pipeline
//! `mat_pipeline::TriageEngine` handles START triage classification and UI output
//! formatting.  This bridge adds Kalman smoothing, robust re-identification, and
//! track lifecycle management *on top* — it receives the triage engine's survivor
//! snapshots, runs them through the MAT tracker, and returns enriched positions
//! (Kalman-smoothed) and track IDs (stable across re-ID).
//!
//! # Pipeline
//! TriageEngine output → DetectionObservation → SurvivorTracker.update()
//!   → AssociationResult → enriched SurvivorSnapshot (smoothed position, track_id)

use std::collections::HashMap;
use wifi_densepose_mat::tracking::{
    SurvivorTracker, TrackerConfig, TrackId,
    DetectionObservation, AssociationResult,
};
use wifi_densepose_mat::domain::{
    coordinates::Coordinates3D,
    vital_signs::{
        VitalSignsReading, BreathingPattern, HeartbeatSignature,
        MovementProfile, BreathingType, MovementType,
    },
    scan_zone::{ScanZone, ZoneBounds},
};
use wifi_densepose_mat::domain::vital_signs::{ConfidenceScore, SignalStrength};

/// A simplified observation from the sensing pipeline, compatible with
/// the data available in `mat_pipeline::VitalSignsInput`.
#[derive(Debug, Clone)]
pub struct TrackObservation {
    /// Estimated 3-D position (e.g. from LocalizationBridge or RSSI).
    pub position: Option<[f64; 3]>,
    /// Breathing rate in BPM.
    pub breathing_rate_bpm: Option<f64>,
    /// Heart rate in BPM.
    pub heart_rate_bpm: Option<f64>,
    /// Signal quality [0, 1].
    pub signal_quality: f64,
    /// Motion score [0, 1].
    pub motion_score: f64,
    /// Detection confidence [0, 1].
    pub confidence: f64,
    /// Associated node ID.
    pub node_id: u8,
    /// Stable person identifier from the sensing pipeline.
    pub person_id: Option<u32>,
}

/// Bridge wrapping the MAT crate's SurvivorTracker.
pub struct TrackingBridge {
    tracker: SurvivorTracker,
    zone: ScanZone,
    /// Map from display ID → MAT TrackId.
    id_map: HashMap<String, TrackId>,
    /// Reverse map: TrackId string → display ID (SURV-XXXXXXXX).
    reverse_id_map: HashMap<String, String>,
    /// Last association result for diagnostics.
    pub last_result: Option<AssociationResult>,
    /// Time of last update for dt calculation.
    last_update_secs: Option<f64>,
}

impl TrackingBridge {
    /// Create a new tracking bridge with competition-appropriate defaults.
    pub fn new() -> Self {
        let config = TrackerConfig {
            birth_hits_required: 2,     // Need 2 consecutive detections to confirm
            max_active_misses: 5,       // 5 misses → Lost (2.5s at 2Hz)
            max_lost_age_secs: 300.0,   // Re-ID window: 5 minutes
            reid_threshold: 0.75,       // Cosine similarity threshold for re-ID
            gate_mahalanobis_sq: 9.0,   // 3-σ gate for association
            obs_noise_var: 0.5,         // Observation noise (metres²)
            process_noise_var: 0.1,     // Process noise (metres²/s²)
        };

        let zone = ScanZone::new(
            "competition-room",
            ZoneBounds::Rectangle {
                min_x: -5.0,
                min_y: -5.0,
                max_x: 5.0,
                max_y: 5.0,
            },
        );

        Self {
            tracker: SurvivorTracker::new(config),
            zone,
            id_map: HashMap::new(),
            reverse_id_map: HashMap::new(),
            last_result: None,
            last_update_secs: None,
        }
    }

    /// Update the tracker with a batch of observations.
    ///
    /// Returns the association result (matches, births, losses, re-IDs).
    /// Call this once per processing cycle with all current detections.
    pub fn update(&mut self, observations: &[TrackObservation]) -> &AssociationResult {
        let now = now_secs();
        let dt = self.last_update_secs
            .map(|prev| (now - prev).max(0.05).min(2.0)) // clamp dt [50ms, 2s]
            .unwrap_or(0.5);
        self.last_update_secs = Some(now);

        let detections: Vec<DetectionObservation> = observations.iter()
            .map(|obs| self.to_detection(obs))
            .collect();

        self.last_result = Some(self.tracker.update(detections, dt));

        // Update ID maps with any new tracks.
        for track in self.tracker.active_tracks() {
            let track_id_str = track.id.to_string();
            self.reverse_id_map
                .entry(track_id_str.clone())
                .or_insert_with(|| format!("SURV-{:08x}", track.id.as_uuid().as_u64_pair().0 as u32));
        }

        self.last_result.as_ref().unwrap()
    }

    /// Get Kalman-smoothed position for a display survivor ID.
    pub fn smoothed_position(&self, display_id: &str) -> Option<[f64; 3]> {
        let track_id = self.id_map.get(display_id)?;
        self.tracker.get_track(track_id)
            .map(|t| t.kalman.position())
    }

    /// Get the display ID for a track, creating one if needed.
    pub fn display_id_for(&mut self, track_id: &TrackId) -> String {
        let key = track_id.to_string();
        self.reverse_id_map
            .entry(key)
            .or_insert_with(|| format!("SURV-{:08x}", track_id.as_uuid().as_u64_pair().0 as u32))
            .clone()
    }

    /// Check if a display ID was re-identified in the last update.
    pub fn was_reidentified(&self, display_id: &str) -> bool {
        self.last_result.as_ref()
            .map(|r| {
                self.id_map.get(display_id)
                    .map(|tid| r.reidentified_track_ids.contains(tid))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Number of active (confirmed) tracks.
    pub fn active_count(&self) -> usize {
        self.tracker.active_count()
    }

    /// Total number of tracks (including tentative, lost).
    pub fn track_count(&self) -> usize {
        self.tracker.track_count()
    }

    /// Convert a TrackObservation into the MAT crate's DetectionObservation.
    fn to_detection(&self, obs: &TrackObservation) -> DetectionObservation {
        let vitals = VitalSignsReading {
            breathing: obs.breathing_rate_bpm.map(|br| BreathingPattern {
                rate_bpm: br as f32,
                amplitude: 0.5,
                regularity: 0.8,
                pattern_type: BreathingType::Normal,
            }),
            heartbeat: obs.heart_rate_bpm.map(|hr| HeartbeatSignature {
                rate_bpm: hr as f32,
                variability: 50.0,
                strength: SignalStrength::Moderate,
            }),
            movement: MovementProfile {
                movement_type: if obs.motion_score > 0.6 {
                    MovementType::Gross
                } else if obs.motion_score > 0.2 {
                    MovementType::Fine
                } else {
                    MovementType::None
                },
                intensity: obs.motion_score as f32,
                frequency: 0.0,
                is_voluntary: obs.motion_score > 0.3,
            },
            timestamp: chrono::Utc::now(),
            confidence: ConfidenceScore::new(obs.confidence),
        };

        let position = obs.position.map(|[x, y, z]| {
            Coordinates3D::with_default_uncertainty(x, y, z)
        });

        DetectionObservation {
            position,
            vital_signs: vitals,
            confidence: obs.confidence,
            zone_id: self.zone.id().clone(),
        }
    }

    /// Get all active tracks with their display IDs and smoothed positions.
    pub fn active_track_snapshots(&self) -> Vec<TrackSnapshot> {
        self.tracker.active_tracks().map(|track| {
            let pos = track.kalman.position();
            let vel = track.kalman.velocity();
            let display_id = self.reverse_id_map
                .get(&track.id.to_string())
                .cloned()
                .unwrap_or_else(|| track.id.to_string());
            TrackSnapshot {
                display_id,
                position: pos,
                velocity: vel,
                state: format!("{:?}", track.lifecycle.state()),
                fingerprint_age: 0,
            }
        }).collect()
    }
}

/// Lightweight snapshot of a tracked survivor for use in UI updates.
#[derive(Debug, Clone)]
pub struct TrackSnapshot {
    pub display_id: String,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub state: String,
    pub fingerprint_age: u64,
}

impl std::fmt::Display for TrackSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} @ [{:.1}, {:.1}, {:.1}] {}",
            self.display_id, self.position[0], self.position[1], self.position[2], self.state)
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
    fn tracking_bridge_constructs() {
        let bridge = TrackingBridge::new();
        assert_eq!(bridge.active_count(), 0);
        assert_eq!(bridge.track_count(), 0);
    }

    #[test]
    fn single_observation_creates_tentative_track() {
        let mut bridge = TrackingBridge::new();
        let obs = TrackObservation {
            position: Some([1.0, 2.0, 0.0]),
            breathing_rate_bpm: Some(15.0),
            heart_rate_bpm: Some(72.0),
            signal_quality: 0.8,
            motion_score: 0.3,
            confidence: 0.7,
            node_id: 1,
            person_id: Some(1),
        };
        bridge.update(&[obs]);
        // First hit creates a tentative track (not yet confirmed).
        assert!(bridge.track_count() > 0);
    }
}
