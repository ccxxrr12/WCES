//! Detection bridge — MAT crate vital sign detection + movement classification.
//!
//! Wraps the `wifi_densepose_mat::detection` modules as an alternative/additional
//! vital sign detection path alongside the existing Goertzel-based `vital_signs.rs`.
//!
//! # Pipeline
//! CSI amplitudes → BreathingDetector (FFT + harmonic analysis) → BreathingResult
//! CSI phases     → HeartbeatDetector (FFT + notch filters + HRV) → HeartbeatResult
//! CSI amplitudes → MovementClassifier (window statistics) → MovementResult
//!
//! # Relationship to existing vital_signs.rs
//! This bridge provides an INDEPENDENT detection path. Results can be:
//! - Used as a second opinion (compare with Goertzel output)
//! - Merged (average the two estimates for higher confidence)
//! - Used as fallback (if one detector has low confidence, use the other)

use wifi_densepose_mat::detection::{
    BreathingDetector, BreathingDetectorConfig,
    HeartbeatDetector, HeartbeatDetectorConfig,
    MovementClassifier, MovementClassifierConfig,
};
use wifi_densepose_mat::domain::vital_signs::{
    BreathingPattern, HeartbeatSignature, MovementProfile,
    BreathingType, SignalStrength, MovementType,
};

/// Breathing detection result (flattened for easy consumption).
#[derive(Debug, Clone)]
pub struct BreathingResult {
    pub rate_bpm: f32,
    pub amplitude: f32,
    pub regularity: f32,
    pub pattern: String,
    pub confidence: f64,
}

/// Heartbeat detection result.
#[derive(Debug, Clone)]
pub struct HeartbeatResult {
    pub rate_bpm: f32,
    pub variability: f32,
    pub strength: String,
    pub confidence: f64,
}

/// Movement classification result.
#[derive(Debug, Clone)]
pub struct MovementResult {
    pub movement_type: String,
    pub intensity: f32,
    pub frequency: f32,
    pub is_voluntary: bool,
}

/// Bridge wrapping MAT detection modules.
pub struct DetectionBridge {
    breathing: BreathingDetector,
    heartbeat: HeartbeatDetector,
    movement: MovementClassifier,
    /// Running sample rate estimate (Hz).
    sample_rate: f64,
}

impl DetectionBridge {
    /// Create with competition-appropriate defaults.
    pub fn new() -> Self {
        let br_config = BreathingDetectorConfig {
            min_rate_bpm: 4.0,
            max_rate_bpm: 40.0,
            min_amplitude: 0.1,
            window_size: 512,
            window_overlap: 0.5,
            confidence_threshold: 0.3,
        };

        let hr_config = HeartbeatDetectorConfig {
            min_rate_bpm: 30.0,
            max_rate_bpm: 200.0,
            min_signal_strength: 0.05,
            window_size: 1024,
            enhanced_processing: true,
            confidence_threshold: 0.4,
        };

        let mv_config = MovementClassifierConfig {
            movement_threshold: 0.1,
            gross_movement_threshold: 0.5,
            window_size: 100,
            periodicity_threshold: 0.3,
        };

        Self {
            breathing: BreathingDetector::new(br_config),
            heartbeat: HeartbeatDetector::new(hr_config),
            movement: MovementClassifier::new(mv_config),
            sample_rate: 30.0,
        }
    }

    /// Update the sample rate estimate (Hz).
    pub fn set_sample_rate(&mut self, rate: f64) {
        self.sample_rate = rate;
    }

    /// Detect breathing pattern from CSI amplitudes.
    ///
    /// Returns `None` if no breathing pattern is detected with sufficient confidence.
    pub fn detect_breathing(&self, amplitudes: &[f64]) -> Option<BreathingResult> {
        self.breathing.detect(amplitudes, self.sample_rate)
            .map(|p| BreathingResult {
                rate_bpm: p.rate_bpm,
                amplitude: p.amplitude,
                regularity: p.regularity,
                pattern: format!("{:?}", p.pattern_type),
                confidence: p.confidence(),
            })
    }

    /// Detect heartbeat from CSI phases, optionally using known breathing rate
    /// for notch filtering of breathing harmonics.
    ///
    /// Returns `None` if no heartbeat is detected with sufficient confidence.
    pub fn detect_heartbeat(
        &self,
        phases: &[f64],
        breathing_rate_bpm: Option<f64>,
    ) -> Option<HeartbeatResult> {
        self.heartbeat.detect(phases, self.sample_rate, breathing_rate_bpm)
            .map(|h| HeartbeatResult {
                rate_bpm: h.rate_bpm,
                variability: h.variability,
                strength: format!("{:?}", h.strength),
                confidence: h.confidence(),
            })
    }

    /// Classify movement pattern from CSI amplitudes.
    pub fn classify_movement(&self, amplitudes: &[f64]) -> MovementResult {
        let profile = self.movement.classify(amplitudes, self.sample_rate);
        MovementResult {
            movement_type: format!("{:?}", profile.movement_type),
            intensity: profile.intensity,
            frequency: profile.frequency,
            is_voluntary: profile.is_voluntary,
        }
    }

    /// Convenience: run all three detectors at once.
    pub fn detect_all(
        &self,
        amplitudes: &[f64],
        phases: &[f64],
    ) -> (Option<BreathingResult>, Option<HeartbeatResult>, MovementResult) {
        let br = self.detect_breathing(amplitudes);
        let hr = self.detect_heartbeat(phases, br.as_ref().map(|b| b.rate_bpm as f64));
        let mv = self.classify_movement(amplitudes);
        (br, hr, mv)
    }
}

impl Default for DetectionBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_bridge_constructs() {
        let bridge = DetectionBridge::new();
        assert!((bridge.sample_rate - 30.0).abs() < 0.01);
    }

    #[test]
    fn detection_bridge_runs_all_detectors() {
        let bridge = DetectionBridge::new();
        let n = 256;
        // Generate plausible-looking CSI: low-freq oscillation on amplitude.
        let amps: Vec<f64> = (0..n)
            .map(|i| 0.6 + 0.15 * (i as f64 * 0.05).sin())
            .collect();
        let phases: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.03).sin() * 0.3)
            .collect();

        let (br, hr, mv) = bridge.detect_all(&amps, &phases);
        // At minimum, movement classification always returns a result.
        assert!(mv.intensity >= 0.0 && mv.intensity <= 1.0);
        // Breathing/heartbeat may or may not detect (depends on signal quality).
        // We just verify they don't panic.
        let _ = (br, hr);
    }
}
