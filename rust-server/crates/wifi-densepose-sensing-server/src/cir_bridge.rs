//! CIR (Channel Impulse Response) bridge — ISTA-based sparse CIR estimation.
//!
//! Wraps `CirEstimator` from `wifi_densepose_signal::ruvsense::cir` for
//! physics-based time-of-flight ranging from ESP32 CSI frames.
//!
//! # Pipeline
//! ESP32 raw CSI amplitudes + phases → CsiFrame → CirEstimator → Cir
//!   → dominant_distance_m / rms_delay_spread / active_tap_count
//!
//! # Design note
//! We do NOT use `HardwareNormalizer` here because its phase-sanitisation step
//! removes the linear phase trend that encodes the channel delay.  Instead we
//! construct the `CsiFrame` directly from raw I/Q data.  The `CirEstimator`
//! applies its own phase-variance ghost-tap guard internally.
//!
//! # Config selection
//! ESP32-C5 provides 64 subcarriers (HT20 waveform).  We use `CirConfig::ht20()`
//! which expects 64-point FFT, 52 active tones, 156 delay taps.

use wifi_densepose_signal::ruvsense::cir::{CirEstimator, CirConfig, Cir};
use wifi_densepose_core::types::{CsiFrame, CsiMetadata, DeviceId, FrequencyBand};
use ndarray::Array2;
use num_complex::Complex64;

/// Thin wrapper around CirEstimator for per-node use.
pub struct CirBridge {
    estimator: CirEstimator,
    /// Latest successful CIR estimate, if any.
    pub latest_cir: Option<Cir>,
    /// Consecutive estimation failures (for degradation tracking).
    failure_count: u32,
    /// Whether the last call produced a valid ranging estimate.
    pub ranging_valid: bool,
    /// Subcarrier count this bridge was configured for (detected from first frame).
    configured_subcarriers: usize,
}

impl CirBridge {
    /// Create a new CIR bridge with HT20 config (64-point FFT, 52 active tones).
    ///
    /// Matches ESP32-C5 raw CSI output (64 subcarriers, HT20 waveform).
    /// For other subcarrier counts the config is upgraded on first frame.
    pub fn new() -> Self {
        let config = CirConfig::ht20();
        let estimator = CirEstimator::new(config);
        Self {
            estimator,
            latest_cir: None,
            failure_count: 0,
            ranging_valid: false,
            configured_subcarriers: 64,
        }
    }

    /// Ensure the estimator config matches the given subcarrier count.
    fn ensure_config(&mut self, n_subcarriers: usize) {
        if n_subcarriers == self.configured_subcarriers {
            return;
        }
        let config = match n_subcarriers {
            64 => CirConfig::ht20(),
            128 => CirConfig::ht40(),
            256 => CirConfig::he20(),
            512 => CirConfig::he40(),
            // Fallback: use canonical-56 for unknown counts.
            _ => CirConfig::canonical56(),
        };
        self.estimator = CirEstimator::new(config);
        self.configured_subcarriers = n_subcarriers;
    }

    /// Process one ESP32 CSI frame. Returns `Some(&Cir)` on success.
    ///
    /// Raw amplitudes and phases are converted to a complex `CsiFrame` and
    /// passed through the ISTA solver.  On failure the previous CIR is retained.
    pub fn process(&mut self, amplitudes: &[f64], phases: &[f64]) -> Option<&Cir> {
        if amplitudes.is_empty() || phases.is_empty() || amplitudes.len() != phases.len() {
            self.failure_count = self.failure_count.saturating_add(1);
            return self.latest_cir.as_ref();
        }

        let n = amplitudes.len();
        self.ensure_config(n);

        // Build a CsiFrame from raw I/Q data.
        let mut data = Array2::<Complex64>::zeros((1, n));
        for i in 0..n {
            let amp = amplitudes[i];
            let phase = phases[i];
            data[[0, i]] = Complex64::new(amp * phase.cos(), amp * phase.sin());
        }
        let meta = CsiMetadata::new(
            DeviceId::new("esp32-c5"),
            FrequencyBand::Band2_4GHz,
            6,
        );
        let csi_frame = CsiFrame::new(meta, data);

        // Run ISTA sparse CIR estimation.
        match self.estimator.estimate(&csi_frame) {
            Ok(cir) => {
                self.ranging_valid = cir.ranging_valid;
                self.failure_count = 0;
                self.latest_cir = Some(cir);
            }
            Err(_) => {
                self.failure_count = self.failure_count.saturating_add(1);
            }
        }

        self.latest_cir.as_ref()
    }

    /// Dominant-path distance in metres (c × dominant_tap_delay).
    pub fn dominant_distance_m(&self) -> Option<f64> {
        self.latest_cir.as_ref().map(|c| c.dominant_distance_m())
    }

    /// Safe ToF-based distance, gated by `ranging_valid`.
    pub fn ranging_distance_m(&self) -> Option<f64> {
        self.latest_cir.as_ref()
            .and_then(|c| c.dominant_tap_tof_s())
            .map(|tof| tof * 3e8)
    }

    /// RMS delay spread in seconds — multipath richness indicator.
    pub fn rms_delay_spread_s(&self) -> Option<f64> {
        self.latest_cir.as_ref().map(|c| c.rms_delay_spread_s)
    }

    /// Number of active taps (magnitude ≥ 1% of dominant).
    pub fn active_tap_count(&self) -> Option<usize> {
        self.latest_cir.as_ref().map(|c| c.active_tap_count)
    }

    /// Dominant-tap ratio (0–1).
    pub fn dominant_tap_ratio(&self) -> Option<f32> {
        self.latest_cir.as_ref().map(|c| c.dominant_tap_ratio)
    }

    /// Number of consecutive estimation failures.
    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }

    /// Whether the CIR estimator is producing healthy output.
    pub fn healthy(&self) -> bool {
        self.failure_count < 5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn cir_bridge_constructs() {
        let bridge = CirBridge::new();
        assert!(bridge.latest_cir.is_none());
        assert!(!bridge.ranging_valid);
    }

    #[test]
    fn cir_bridge_rejects_empty_input() {
        let mut bridge = CirBridge::new();
        let result = bridge.process(&[], &[]);
        assert!(result.is_none());
        assert_eq!(bridge.failure_count(), 1);
    }

    #[test]
    fn cir_bridge_processes_valid_frame() {
        let mut bridge = CirBridge::new();
        // Generate a clean single-path HT20 channel: 64 subcarriers, 30 ns delay.
        let n = 64;
        let tau = 30e-9_f64;
        let delta_f = 312_500.0; // 312.5 kHz spacing
        let amps: Vec<f64> = vec![0.8; n];
        let phases: Vec<f64> = (0..n)
            .map(|i| {
                let sc = if i <= n / 2 { i as f64 } else { i as f64 - n as f64 };
                -TAU * sc * delta_f * tau
            })
            .collect();

        let result = bridge.process(&amps, &phases);
        assert!(result.is_some(), "CIR should succeed on clean synthetic HT20 frame");
        let cir = result.unwrap();
        assert_eq!(cir.taps.len(), 156, "HT20 → 3×52 = 156 delay taps");
        // With a clean 30ns single-path channel, the dominant tap should concentrate energy.
        assert!(cir.dominant_tap_ratio > 0.0, "dominant_tap_ratio={} should be > 0", cir.dominant_tap_ratio);
    }
}
