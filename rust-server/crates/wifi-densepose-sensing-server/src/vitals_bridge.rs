//! Bridge connecting the wifi-densepose-vitals crate into the sensing-server pipeline.
//!
//! Runs the vitals crate's Butterworth-based extractors alongside the existing
//! Goertzel FFT detector. When both produce results, uses the vitals crate output
//! (better filtering); falls back to existing detector when vitals crate returns None.
//!
//! # Pipeline
//! Esp32Frame → vitals::CsiFrame → CsiVitalPreprocessor → BreathingExtractor + HeartRateExtractor
//!                                                                          ↓
//!                                                              VitalSigns (merged)

use wifi_densepose_vitals::breathing::BreathingExtractor;
use wifi_densepose_vitals::heartrate::HeartRateExtractor;
use wifi_densepose_vitals::preprocessor::CsiVitalPreprocessor;
use wifi_densepose_vitals::types::{self as vitals_types, VitalStatus};

use crate::vital_signs::VitalSigns;

/// Managed vitals crate extraction pipeline.
pub struct VitalsBridge {
    preprocessor: CsiVitalPreprocessor,
    breathing: BreathingExtractor,
    heartrate: HeartRateExtractor,
    sample_rate: f64,
}

impl VitalsBridge {
    /// Create a new vitals pipeline for the given subcarrier count and sample rate.
    /// `alpha` controls EMA responsiveness (0.05 = slow tracking, better static suppression).
    pub fn new(n_subcarriers: usize, sample_rate: f64) -> Self {
        Self {
            preprocessor: CsiVitalPreprocessor::new(n_subcarriers, 0.05),
            breathing: BreathingExtractor::new(
                n_subcarriers.min(64),
                sample_rate.max(1.0),
                30.0, // 30-second window
            ),
            heartrate: HeartRateExtractor::new(
                n_subcarriers.min(64),
                sample_rate.max(1.0),
                15.0, // 15-second window
            ),
            sample_rate,
        }
    }

    /// Update sample rate (called when EMA-measured rate changes).
    pub fn set_sample_rate(&mut self, rate: f64) {
        self.sample_rate = rate.max(1.0);
    }

    /// Run the vitals crate pipeline on a CSI frame.
    /// Returns breathing rate and heart rate estimates if available.
    pub fn extract(
        &mut self,
        amplitudes: &[f64],
        phases: &[f64],
        sample_index: u64,
    ) -> (Option<f64>, Option<f64>, f64, f64) {
        // Build vitals crate CsiFrame (field-compatible with Esp32Frame)
        let n = amplitudes.len().min(phases.len());
        let frame = match vitals_types::CsiFrame::new(
            amplitudes[..n].to_vec(),
            phases[..n].to_vec(),
            n,
            sample_index,
            self.sample_rate,
        ) {
            Some(f) => f,
            None => return (None, None, 0.0, 0.0),
        };

        // Preprocess: extract body-modulated residuals (EMA baseline subtraction)
        let residuals = match self.preprocessor.process(&frame) {
            Some(r) => r,
            None => return (None, None, 0.0, 0.0),
        };

        // Uniform weights for breathing extraction
        let rn = residuals.len().min(64);
        let uniform_weights: Vec<f64> = vec![1.0 / rn as f64; rn];

        // Breathing extraction
        let br_est = self.breathing.extract(&residuals, &uniform_weights);
        let hr_est = self.heartrate.extract(&residuals, phases);

        let br = br_est.as_ref().filter(|e| e.status == VitalStatus::Valid).map(|e| e.value_bpm);
        let hr = hr_est.as_ref().filter(|e| e.status == VitalStatus::Valid).map(|e| e.value_bpm);
        let br_conf = br_est.as_ref().map(|e| e.confidence).unwrap_or(0.0);
        let hr_conf = hr_est.as_ref().map(|e| e.confidence).unwrap_or(0.0);

        (br, hr, br_conf, hr_conf)
    }
}
