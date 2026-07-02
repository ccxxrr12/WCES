//! Field model calibration bridge — physics-grounded signal field generation.
//!
//! Uses empty-room electromagnetic baseline subtraction (ADR-143 simplified).
//! During auto-calibration (~30s at 20Hz), the system learns the room's
//! static eigenstructure from CSI amplitude observations. After calibration,
//! each frame's perturbation energy is extracted to drive the signal field.
//!
//! # Pipeline
//! CSI Frame → FieldModel::feed_calibration (calibration phase)
//! CSI Frame → FieldModel::extract_perturbation → energy → signal field

use std::collections::VecDeque;
use wifi_densepose_signal::ruvsense::field_model::{FieldModel, FieldModelConfig, FieldModelError};

/// Number of frames to collect during auto-calibration (~30s at 20Hz).
pub const AUTO_CALIBRATION_FRAMES: usize = 600;

/// Number of recent perturbation values for EMA smoothing.
const SMOOTHING_WINDOW: usize = 50;

pub struct FieldBridge {
    model: FieldModel,
    calibration_count: usize,
    pub calibrated: bool,
    energy_history: VecDeque<f64>,
    pub smoothed_energy: f64,
}

impl FieldBridge {
    pub fn new() -> Result<Self, FieldModelError> {
        let config = FieldModelConfig {
            n_links: 1,
            n_subcarriers: 64,
            min_calibration_frames: AUTO_CALIBRATION_FRAMES,
            ..FieldModelConfig::default()
        };
        let model = FieldModel::new(config)?;
        Ok(Self {
            model,
            calibration_count: 0,
            calibrated: false,
            energy_history: VecDeque::with_capacity(SMOOTHING_WINDOW),
            smoothed_energy: 0.0,
        })
    }

    /// Feed a CSI amplitude vector. Returns perturbation energy if post-calibration.
    pub fn feed(&mut self, amplitudes: &[f64]) -> Option<f64> {
        if amplitudes.is_empty() {
            return None;
        }

        if !self.calibrated {
            // Feed one link's observation vector — only count on success
            if self.model.feed_calibration(&[amplitudes.to_vec()]).is_ok() {
                self.calibration_count += 1;
            }
            if self.calibration_count >= AUTO_CALIBRATION_FRAMES {
                let now_us = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as u64;
                if self.model.finalize_calibration(now_us, 0).is_ok() {
                    self.calibrated = true;
                }
            }
            return None;
        }

        // Post-calibration: extract perturbation
        match self.model.extract_perturbation(&[amplitudes.to_vec()]) {
            Ok(perturbation) => {
                let raw = perturbation.total_energy;
                self.energy_history.push_back(raw);
                if self.energy_history.len() > SMOOTHING_WINDOW {
                    self.energy_history.pop_front();
                }
                let sum: f64 = self.energy_history.iter().sum();
                let count = self.energy_history.len() as f64;
                self.smoothed_energy = if count > 0.0 { sum / count } else { raw };
                Some(self.smoothed_energy)
            }
            Err(_) => None,
        }
    }

    pub fn calibration_pct(&self) -> f64 {
        (self.calibration_count as f64 / AUTO_CALIBRATION_FRAMES as f64 * 100.0).min(100.0)
    }
}
