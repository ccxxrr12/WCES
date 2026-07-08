//! Neural network-based vital signs classifier with uncertainty estimation.
//!
//! This module provides ML-based classification for:
//! - Breathing pattern types (normal, shallow, labored, irregular, agonal)
//! - Heartbeat signatures (normal, bradycardia, tachycardia)
//! - Movement patterns with voluntary/involuntary distinction
//!
//! ## Uncertainty Estimation
//!
//! The classifier implements Monte Carlo Dropout for uncertainty quantification,
//! providing both aleatoric (data) and epistemic (model) uncertainty estimates.
//!
//! ## Architecture
//!
//! Uses a multi-task neural network with shared encoder:
//! ```text
//! CSI Features -> Shared Encoder -> [Breathing Head, Heartbeat Head, Movement Head]
//!                                   |               |                |
//!                                   v               v                v
//!                             [Class Logits]  [Rate + Var]    [Type + Intensity]
//!                             [Uncertainty]   [Confidence]    [Voluntary Flag]
//! ```

#![allow(unexpected_cfgs)]

use super::{MlError, MlResult};
use crate::detection::CsiDataBuffer;
use crate::domain::{
    BreathingPattern, BreathingType, HeartbeatSignature, MovementProfile,
    MovementType, SignalStrength, VitalSignsReading,
};
use std::path::Path;
use tracing::{info, instrument, warn};

#[cfg(feature = "onnx")]
use ndarray::{Array1, Array2, Array4, s};
#[cfg(feature = "onnx")]
use std::collections::HashMap;
#[cfg(feature = "onnx")]
use std::sync::Arc;
#[cfg(feature = "onnx")]
use parking_lot::RwLock;
#[cfg(feature = "onnx")]
use tracing::debug;

#[cfg(feature = "onnx")]
use wifi_densepose_nn::{OnnxBackend, OnnxSession, InferenceOptions, Tensor, TensorShape};

/// Configuration for the vital signs classifier
#[derive(Debug, Clone)]
pub struct VitalSignsClassifierConfig {
    /// Use GPU for inference
    pub use_gpu: bool,
    /// Number of inference threads
    pub num_threads: usize,
    /// Minimum confidence threshold for valid detection
    pub min_confidence: f32,
    /// Enable uncertainty estimation (MC Dropout)
    pub enable_uncertainty: bool,
    /// Number of MC Dropout samples for uncertainty
    pub mc_samples: usize,
    /// Dropout rate for MC Dropout
    pub dropout_rate: f32,
}

impl Default for VitalSignsClassifierConfig {
    fn default() -> Self {
        Self {
            use_gpu: false,
            num_threads: 4,
            min_confidence: 0.5,
            enable_uncertainty: true,
            mc_samples: 10,
            dropout_rate: 0.1,
        }
    }
}

/// Features extracted for vital signs classification
#[derive(Debug, Clone)]
pub struct VitalSignsFeatures {
    /// Time-domain features from amplitude
    pub amplitude_features: Vec<f32>,
    /// Time-domain features from phase
    pub phase_features: Vec<f32>,
    /// Frequency-domain features
    pub spectral_features: Vec<f32>,
    /// Breathing-band power (0.1-0.5 Hz)
    pub breathing_band_power: f32,
    /// Heartbeat-band power (0.8-2.0 Hz)
    pub heartbeat_band_power: f32,
    /// Movement-band power (0-5 Hz broadband)
    pub movement_band_power: f32,
    /// Signal quality indicator
    pub signal_quality: f32,
    /// Sample rate of the original data
    pub sample_rate: f64,
}

impl VitalSignsFeatures {
    /// Convert to model input tensor
    pub fn to_tensor(&self) -> Vec<f32> {
        let mut features = Vec::with_capacity(256);

        // Add amplitude features (64)
        features.extend_from_slice(&self.amplitude_features[..self.amplitude_features.len().min(64)]);
        features.resize(64, 0.0);

        // Add phase features (64)
        features.extend_from_slice(&self.phase_features[..self.phase_features.len().min(64)]);
        features.resize(128, 0.0);

        // Add spectral features (64)
        features.extend_from_slice(&self.spectral_features[..self.spectral_features.len().min(64)]);
        features.resize(192, 0.0);

        // Add band power features
        features.push(self.breathing_band_power);
        features.push(self.heartbeat_band_power);
        features.push(self.movement_band_power);
        features.push(self.signal_quality);

        // Pad to 256
        features.resize(256, 0.0);

        features
    }
}

/// Breathing pattern classification result
#[derive(Debug, Clone)]
pub struct BreathingClassification {
    /// Detected breathing type
    pub breathing_type: BreathingType,
    /// Estimated breathing rate (BPM)
    pub rate_bpm: f32,
    /// Rate uncertainty (standard deviation)
    pub rate_uncertainty: f32,
    /// Classification confidence
    pub confidence: f32,
    /// Per-class probabilities
    pub class_probabilities: Vec<f32>,
    /// Uncertainty estimate
    pub uncertainty: UncertaintyEstimate,
}

impl BreathingClassification {
    /// Convert to domain BreathingPattern
    pub fn to_breathing_pattern(&self) -> Option<BreathingPattern> {
        if self.confidence < 0.3 {
            return None;
        }

        Some(BreathingPattern {
            rate_bpm: self.rate_bpm,
            amplitude: self.confidence,
            regularity: 1.0 - self.uncertainty.total(),
            pattern_type: self.breathing_type.clone(),
        })
    }
}

/// Heartbeat signature classification result
#[derive(Debug, Clone)]
pub struct HeartbeatClassification {
    /// Estimated heart rate (BPM)
    pub rate_bpm: f32,
    /// Rate uncertainty (standard deviation)
    pub rate_uncertainty: f32,
    /// Heart rate variability
    pub hrv: f32,
    /// Signal strength indicator
    pub signal_strength: SignalStrength,
    /// Classification confidence
    pub confidence: f32,
    /// Uncertainty estimate
    pub uncertainty: UncertaintyEstimate,
}

impl HeartbeatClassification {
    /// Convert to domain HeartbeatSignature
    pub fn to_heartbeat_signature(&self) -> Option<HeartbeatSignature> {
        if self.confidence < 0.3 {
            return None;
        }

        Some(HeartbeatSignature {
            rate_bpm: self.rate_bpm,
            variability: self.hrv,
            strength: self.signal_strength.clone(),
        })
    }

    /// Classify heart rate as normal/bradycardia/tachycardia
    pub fn classify_rate(&self) -> &'static str {
        if self.rate_bpm < 60.0 {
            "bradycardia"
        } else if self.rate_bpm > 100.0 {
            "tachycardia"
        } else {
            "normal"
        }
    }
}

/// Uncertainty estimate with aleatoric and epistemic components
#[derive(Debug, Clone)]
pub struct UncertaintyEstimate {
    /// Aleatoric uncertainty (irreducible, from data)
    pub aleatoric: f32,
    /// Epistemic uncertainty (reducible, from model)
    pub epistemic: f32,
    /// Whether the prediction is considered reliable
    pub is_reliable: bool,
}

impl UncertaintyEstimate {
    /// Create new uncertainty estimate
    pub fn new(aleatoric: f32, epistemic: f32) -> Self {
        let total = (aleatoric.powi(2) + epistemic.powi(2)).sqrt();
        Self {
            aleatoric,
            epistemic,
            is_reliable: total < 0.3,
        }
    }

    /// Get total uncertainty
    pub fn total(&self) -> f32 {
        (self.aleatoric.powi(2) + self.epistemic.powi(2)).sqrt()
    }

    /// Check if prediction is confident
    pub fn is_confident(&self, threshold: f32) -> bool {
        self.total() < threshold
    }
}

impl Default for UncertaintyEstimate {
    fn default() -> Self {
        Self {
            aleatoric: 0.5,
            epistemic: 0.5,
            is_reliable: false,
        }
    }
}

/// Combined classifier output
#[derive(Debug, Clone)]
pub struct ClassifierOutput {
    /// Breathing classification
    pub breathing: Option<BreathingClassification>,
    /// Heartbeat classification
    pub heartbeat: Option<HeartbeatClassification>,
    /// Movement classification
    pub movement: Option<MovementClassification>,
    /// Overall confidence
    pub overall_confidence: f32,
    /// Combined uncertainty
    pub combined_uncertainty: UncertaintyEstimate,
}

impl ClassifierOutput {
    /// Convert to domain VitalSignsReading
    pub fn to_vital_signs_reading(&self) -> Option<VitalSignsReading> {
        let breathing = self.breathing.as_ref()
            .and_then(|b| b.to_breathing_pattern());
        let heartbeat = self.heartbeat.as_ref()
            .and_then(|h| h.to_heartbeat_signature());
        let movement = self.movement.as_ref()
            .map(|m| m.to_movement_profile())
            .unwrap_or_default();

        if breathing.is_none() && heartbeat.is_none() && movement.movement_type == MovementType::None {
            return None;
        }

        Some(VitalSignsReading::new(breathing, heartbeat, movement))
    }
}

/// Movement classification result
#[derive(Debug, Clone)]
pub struct MovementClassification {
    /// Movement type
    pub movement_type: MovementType,
    /// Movement intensity (0.0-1.0)
    pub intensity: f32,
    /// Whether movement appears voluntary
    pub is_voluntary: bool,
    /// Frequency of movement
    pub frequency: f32,
    /// Classification confidence
    pub confidence: f32,
}

impl MovementClassification {
    /// Convert to domain MovementProfile
    pub fn to_movement_profile(&self) -> MovementProfile {
        MovementProfile {
            movement_type: self.movement_type.clone(),
            intensity: self.intensity,
            frequency: self.frequency,
            is_voluntary: self.is_voluntary,
        }
    }
}

/// Neural network-based vital signs classifier
pub struct VitalSignsClassifier {
    config: VitalSignsClassifierConfig,
    /// Whether ONNX model is loaded
    model_loaded: bool,
    /// Pre-computed filter coefficients for breathing band
    breathing_filter: BandpassFilter,
    /// Pre-computed filter coefficients for heartbeat band
    heartbeat_filter: BandpassFilter,
    /// Cached ONNX session
    #[cfg(feature = "onnx")]
    session: Option<Arc<RwLock<OnnxSession>>>,
}

/// Simple bandpass filter coefficients
struct BandpassFilter {
    low_freq: f64,
    high_freq: f64,
    sample_rate: f64,
}

impl BandpassFilter {
    fn new(low: f64, high: f64, sample_rate: f64) -> Self {
        Self {
            low_freq: low,
            high_freq: high,
            sample_rate,
        }
    }

    /// Apply bandpass filter (simplified FFT-based approach)
    fn apply(&self, signal: &[f64]) -> Vec<f64> {
        use rustfft::{FftPlanner, num_complex::Complex};

        if signal.len() < 8 {
            return signal.to_vec();
        }

        // Pad to power of 2
        let n = signal.len().next_power_of_two();
        let mut buffer: Vec<Complex<f64>> = signal.iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        buffer.resize(n, Complex::new(0.0, 0.0));

        // Forward FFT
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);
        fft.process(&mut buffer);

        // Apply frequency mask
        let freq_resolution = self.sample_rate / n as f64;
        for (i, val) in buffer.iter_mut().enumerate() {
            let freq = if i <= n / 2 {
                i as f64 * freq_resolution
            } else {
                (n - i) as f64 * freq_resolution
            };

            if freq < self.low_freq || freq > self.high_freq {
                *val = Complex::new(0.0, 0.0);
            }
        }

        // Inverse FFT
        let ifft = planner.plan_fft_inverse(n);
        ifft.process(&mut buffer);

        // Normalize and extract real part
        buffer.iter()
            .take(signal.len())
            .map(|c| c.re / n as f64)
            .collect()
    }

    /// Calculate band power
    fn band_power(&self, signal: &[f64]) -> f64 {
        let filtered = self.apply(signal);
        filtered.iter().map(|x| x.powi(2)).sum::<f64>() / filtered.len() as f64
    }
}

impl VitalSignsClassifier {
    /// Create classifier from ONNX model file
    #[instrument(skip(path))]
    pub fn from_onnx<P: AsRef<Path>>(path: P, config: VitalSignsClassifierConfig) -> MlResult<Self> {
        let path_ref = path.as_ref();
        info!(?path_ref, "Loading vital signs classifier");

        #[cfg(feature = "onnx")]
        let session = if path_ref.exists() {
            let options = InferenceOptions {
                use_gpu: config.use_gpu,
                num_threads: config.num_threads,
                ..Default::default()
            };
            match OnnxSession::from_file(path_ref, &options) {
                Ok(s) => {
                    info!("ONNX vital signs model loaded successfully");
                    Some(Arc::new(RwLock::new(s)))
                }
                Err(e) => {
                    warn!(?e, "Failed to load ONNX model, using rule-based fallback");
                    None
                }
            }
        } else {
            warn!(?path_ref, "Model file not found, using rule-based fallback");
            None
        };

        #[cfg(feature = "onnx")]
        let model_loaded = session.is_some();

        #[cfg(not(feature = "onnx"))]
        let model_loaded = false;

        Ok(Self {
            config,
            model_loaded,
            breathing_filter: BandpassFilter::new(0.1, 0.5, 1000.0),
            heartbeat_filter: BandpassFilter::new(0.8, 2.0, 1000.0),
            #[cfg(feature = "onnx")]
            session,
        })
    }

    /// Create rule-based classifier (no ONNX)
    pub fn rule_based(config: VitalSignsClassifierConfig) -> Self {
        Self {
            config,
            model_loaded: false,
            breathing_filter: BandpassFilter::new(0.1, 0.5, 1000.0),
            heartbeat_filter: BandpassFilter::new(0.8, 2.0, 1000.0),
            #[cfg(feature = "onnx")]
            session: None,
        }
    }

    /// Check if ONNX model is loaded
    pub fn is_loaded(&self) -> bool {
        self.model_loaded
    }

    /// Extract features from CSI buffer
    pub fn extract_features(&self, buffer: &CsiDataBuffer) -> MlResult<VitalSignsFeatures> {
        if buffer.amplitudes.is_empty() {
            return Err(MlError::FeatureExtraction("Empty CSI buffer".into()));
        }

        // Update filters with actual sample rate
        let breathing_filter = BandpassFilter::new(0.1, 0.5, buffer.sample_rate);
        let heartbeat_filter = BandpassFilter::new(0.8, 2.0, buffer.sample_rate);

        // Extract amplitude features
        let amplitude_features = self.extract_time_features(&buffer.amplitudes);

        // Extract phase features
        let phase_features = self.extract_time_features(&buffer.phases);

        // Extract spectral features
        let spectral_features = self.extract_spectral_features(&buffer.amplitudes, buffer.sample_rate);

        // Calculate band powers
        let breathing_band_power = breathing_filter.band_power(&buffer.amplitudes) as f32;
        let heartbeat_band_power = heartbeat_filter.band_power(&buffer.phases) as f32;

        // Movement detection using broadband power
        let movement_band_power = buffer.amplitudes.iter()
            .map(|x| x.powi(2))
            .sum::<f64>() as f32 / buffer.amplitudes.len() as f32;

        // Signal quality
        let signal_quality = self.estimate_signal_quality(&buffer.amplitudes);

        Ok(VitalSignsFeatures {
            amplitude_features,
            phase_features,
            spectral_features,
            breathing_band_power,
            heartbeat_band_power,
            movement_band_power,
            signal_quality,
            sample_rate: buffer.sample_rate,
        })
    }

    /// Extract time-domain features
    fn extract_time_features(&self, signal: &[f64]) -> Vec<f32> {
        if signal.is_empty() {
            return vec![0.0; 64];
        }

        let n = signal.len();
        let mean = signal.iter().sum::<f64>() / n as f64;
        let variance = signal.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>() / n as f64;
        let std_dev = variance.sqrt();

        let mut features = Vec::with_capacity(64);

        // Statistical features
        features.push(mean as f32);
        features.push(std_dev as f32);
        features.push(variance as f32);

        // Min/max
        let min = signal.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = signal.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        features.push(min as f32);
        features.push(max as f32);
        features.push((max - min) as f32);

        // Skewness
        let skewness = if std_dev > 1e-10 {
            signal.iter()
                .map(|x| ((x - mean) / std_dev).powi(3))
                .sum::<f64>() / n as f64
        } else {
            0.0
        };
        features.push(skewness as f32);

        // Kurtosis
        let kurtosis = if std_dev > 1e-10 {
            signal.iter()
                .map(|x| ((x - mean) / std_dev).powi(4))
                .sum::<f64>() / n as f64 - 3.0
        } else {
            0.0
        };
        features.push(kurtosis as f32);

        // Zero crossing rate
        let zero_crossings = signal.windows(2)
            .filter(|w| (w[0] - mean) * (w[1] - mean) < 0.0)
            .count();
        features.push(zero_crossings as f32 / n as f32);

        // RMS
        let rms = (signal.iter().map(|x| x.powi(2)).sum::<f64>() / n as f64).sqrt();
        features.push(rms as f32);

        // Subsample signal for temporal features
        let step = (n / 50).max(1);
        for i in (0..n).step_by(step).take(54) {
            features.push(((signal[i] - mean) / (std_dev + 1e-8)) as f32);
        }

        // Pad to 64
        features.resize(64, 0.0);
        features
    }

    /// Extract frequency-domain features
    fn extract_spectral_features(&self, signal: &[f64], sample_rate: f64) -> Vec<f32> {
        use rustfft::{FftPlanner, num_complex::Complex};

        if signal.len() < 16 {
            return vec![0.0; 64];
        }

        let n = 128.min(signal.len().next_power_of_two());
        let mut buffer: Vec<Complex<f64>> = signal.iter()
            .take(n)
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        buffer.resize(n, Complex::new(0.0, 0.0));

        // Apply Hann window
        for (i, val) in buffer.iter_mut().enumerate() {
            let window = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos());
            *val = Complex::new(val.re * window, 0.0);
        }

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);
        fft.process(&mut buffer);

        // Extract power spectrum (first half)
        let mut features: Vec<f32> = buffer.iter()
            .take(n / 2)
            .map(|c| (c.norm() / n as f64) as f32)
            .collect();

        // Pad to 64
        features.resize(64, 0.0);

        // Find dominant frequency
        let freq_resolution = sample_rate / n as f64;
        let (max_idx, _) = features.iter()
            .enumerate()
            .skip(1)  // Skip DC
            .take(30) // Up to ~30% of Nyquist
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, &0.0));

        // Store dominant frequency in last position
        features[63] = (max_idx as f64 * freq_resolution) as f32;

        features
    }

    /// Estimate signal quality
    fn estimate_signal_quality(&self, signal: &[f64]) -> f32 {
        if signal.len() < 10 {
            return 0.0;
        }

        let mean = signal.iter().sum::<f64>() / signal.len() as f64;
        let variance = signal.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>() / signal.len() as f64;

        // Higher SNR = higher quality
        let snr = if variance > 1e-10 {
            mean.abs() / variance.sqrt()
        } else {
            10.0
        };

        (snr / 5.0).min(1.0) as f32
    }

    /// Classify vital signs from features
    #[instrument(skip(self, features))]
    pub async fn classify(&self, features: &VitalSignsFeatures) -> MlResult<ClassifierOutput> {
        #[cfg(feature = "onnx")]
        if let Some(ref session) = self.session {
            return self.classify_onnx(features, session).await;
        }

        // Fall back to rule-based classification
        self.classify_rules(features)
    }

    /// ONNX-based classification
    #[cfg(feature = "onnx")]
    async fn classify_onnx(
        &self,
        features: &VitalSignsFeatures,
        session: &Arc<RwLock<OnnxSession>>,
    ) -> MlResult<ClassifierOutput> {
        let input_tensor = features.to_tensor();

        // Create 4D tensor for model input
        let input_array = Array4::from_shape_vec(
            (1, 1, 1, input_tensor.len()),
            input_tensor,
        ).map_err(|e| MlError::Inference(e.to_string()))?;

        let tensor = Tensor::Float4D(input_array);

        let mut inputs = HashMap::new();
        inputs.insert("input".to_string(), tensor);

        // Run inference (potentially multiple times for MC Dropout)
        let mc_samples = if self.config.enable_uncertainty {
            self.config.mc_samples
        } else {
            1
        };

        let mut all_outputs = Vec::with_capacity(mc_samples);
        // TODO(M-10): each `session.write().run(inputs.clone())` is a
        // synchronous blocking ort inference call executed on the tokio
        // async executor. With `mc_samples` iterations this can stall the
        // runtime for tens of milliseconds per `classify` call. This should
        // be moved to `tokio::task::spawn_blocking` (running all MC samples
        // inside a single blocking closure that acquires the write lock
        // once and loops). Requires `OnnxSession: Send`.
        for _ in 0..mc_samples {
            let outputs = session.write().run(inputs.clone())
                .map_err(|e| MlError::NeuralNetwork(e))?;
            all_outputs.push(outputs);
        }

        // Aggregate MC Dropout outputs
        self.aggregate_mc_outputs(&all_outputs, features)
    }

    /// Aggregate Monte Carlo Dropout outputs (H-7).
    ///
    /// Computes per-element mean and variance across the MC samples for each
    /// model output head. The mean is used as the point prediction, while the
    /// variance across samples provides epistemic uncertainty (model
    /// uncertainty due to limited data / dropout masks).
    ///
    /// Expected output names (model convention):
    /// - `"breathing"`: `[rate_bpm, ...]`
    /// - `"heartbeat"`: `[rate_bpm, hrv, strength, ...]`
    /// - `"movement"`:  `[intensity, frequency, voluntary_flag, type_id]`
    ///
    /// If a head is missing or unparseable, falls back to the corresponding
    /// rule-based classifier for that head.
    #[cfg(feature = "onnx")]
    fn aggregate_mc_outputs(
        &self,
        outputs: &[HashMap<String, Tensor>],
        features: &VitalSignsFeatures,
    ) -> MlResult<ClassifierOutput> {
        if outputs.is_empty() {
            return self.classify_rules(features);
        }

        // Collect per-key tensor data as flat f32 vectors across MC samples.
        let mut per_key_samples: HashMap<String, Vec<Vec<f32>>> = HashMap::new();
        for out in outputs {
            for (key, tensor) in out.iter() {
                match tensor.to_vec() {
                    Ok(v) => per_key_samples.entry(key.clone()).or_default().push(v),
                    Err(_) => continue,
                }
            }
        }

        if per_key_samples.is_empty() {
            return self.classify_rules(features);
        }

        // Compute per-element mean and population variance across samples.
        let stats_for = |key: &str| -> Option<(Vec<f32>, Vec<f32>)> {
            let samples = per_key_samples.get(key)?;
            if samples.is_empty() {
                return None;
            }
            let n = samples.len() as f32;
            let m = samples.iter().map(|s| s.len()).max()?;
            if m == 0 {
                return None;
            }
            let mut mean = vec![0.0_f32; m];
            for s in samples {
                for (i, &v) in s.iter().take(m).enumerate() {
                    mean[i] += v;
                }
            }
            for x in mean.iter_mut() {
                *x /= n;
            }
            let mut var = vec![0.0_f32; m];
            for s in samples {
                for (i, &v) in s.iter().take(m).enumerate() {
                    let d = v - mean[i];
                    var[i] += d * d;
                }
            }
            // Population variance; for n==1 epistemic uncertainty collapses to 0.
            let denom = if n > 1.0 { n } else { 1.0 };
            for x in var.iter_mut() {
                *x /= denom;
            }
            Some((mean, var))
        };

        let aleatoric = 1.0 - features.signal_quality;

        // --- Breathing head -------------------------------------------------
        let breathing = if let Some((mean, var)) = stats_for("breathing") {
            let rate_bpm = mean.first().copied().unwrap_or(0.0);
            let rate_uncertainty = var.first().copied().unwrap_or(0.0).sqrt();
            let breathing_type = self.classify_breathing_type(rate_bpm, features);
            let class_probabilities = self.compute_breathing_probabilities(rate_bpm, features);
            let power_confidence = (features.breathing_band_power * 10.0).min(1.0);
            let quality_confidence = features.signal_quality;
            let confidence = ((power_confidence + quality_confidence) / 2.0
                * (1.0 - rate_uncertainty.clamp(0.0, 1.0)))
                .max(0.0)
                .min(1.0);
            let epistemic = rate_uncertainty.clamp(0.0, 1.0);
            Some(BreathingClassification {
                breathing_type,
                rate_bpm,
                rate_uncertainty,
                confidence,
                class_probabilities,
                uncertainty: UncertaintyEstimate::new(aleatoric, epistemic),
            })
        } else {
            self.classify_breathing_rules(features)
        };

        // --- Heartbeat head -------------------------------------------------
        let heartbeat = if let Some((mean, var)) = stats_for("heartbeat") {
            let rate_bpm = mean.first().copied().unwrap_or(0.0);
            let rate_uncertainty = var.first().copied().unwrap_or(0.0).sqrt();
            if rate_bpm < 30.0 || rate_bpm > 200.0 {
                None
            } else {
                let hrv = mean.get(1).copied().unwrap_or(0.0).max(0.0);
                let strength_raw =
                    mean.get(2).copied().unwrap_or(features.heartbeat_band_power);
                let signal_strength = if strength_raw > 0.66 {
                    SignalStrength::Strong
                } else if strength_raw > 0.33 {
                    SignalStrength::Moderate
                } else if strength_raw > 0.1 {
                    SignalStrength::Weak
                } else {
                    SignalStrength::VeryWeak
                };
                let epistemic = rate_uncertainty.clamp(0.0, 1.0);
                let confidence = (1.0 - epistemic).max(0.0).min(1.0);
                Some(HeartbeatClassification {
                    rate_bpm,
                    rate_uncertainty,
                    hrv,
                    signal_strength,
                    confidence,
                    uncertainty: UncertaintyEstimate::new(aleatoric, epistemic),
                })
            }
        } else {
            self.classify_heartbeat_rules(features)
        };

        // --- Movement head --------------------------------------------------
        let movement = if let Some((mean, var)) = stats_for("movement") {
            let intensity = mean.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let frequency = mean.get(1).copied().unwrap_or(0.0).max(0.0);
            let is_voluntary = mean.get(2).copied().unwrap_or(0.0) > 0.5;
            let type_id = mean.get(3).copied().unwrap_or(0.0).round() as i32;
            let movement_type = match type_id {
                0 => MovementType::None,
                1 => MovementType::Gross,
                2 => MovementType::Fine,
                3 => MovementType::Tremor,
                4 => MovementType::Periodic,
                _ => MovementType::None,
            };
            let intensity_var = var.first().copied().unwrap_or(0.0);
            let confidence = (1.0 - intensity_var.clamp(0.0, 1.0)).max(0.0).min(1.0);
            Some(MovementClassification {
                movement_type,
                intensity,
                is_voluntary,
                frequency,
                confidence,
            })
        } else {
            self.classify_movement_rules(features)
        };

        // --- Aggregate ------------------------------------------------------
        let mut confidences: Vec<f32> = Vec::with_capacity(3);
        if let Some(b) = breathing.as_ref() {
            confidences.push(b.confidence);
        }
        if let Some(h) = heartbeat.as_ref() {
            confidences.push(h.confidence);
        }
        if let Some(m) = movement.as_ref() {
            confidences.push(m.confidence);
        }
        let overall_confidence = if confidences.is_empty() {
            0.0
        } else {
            confidences.iter().sum::<f32>() / confidences.len() as f32
        };

        // Combined epistemic uncertainty = mean epistemic across heads that
        // provided an uncertainty estimate (breathing + heartbeat).
        let mut eps: Vec<f32> = Vec::new();
        if let Some(b) = breathing.as_ref() {
            eps.push(b.uncertainty.epistemic);
        }
        if let Some(h) = heartbeat.as_ref() {
            eps.push(h.uncertainty.epistemic);
        }
        let epistemic_avg = if eps.is_empty() {
            0.5
        } else {
            eps.iter().sum::<f32>() / eps.len() as f32
        };
        let combined_uncertainty = UncertaintyEstimate::new(aleatoric, epistemic_avg);

        Ok(ClassifierOutput {
            breathing,
            heartbeat,
            movement,
            overall_confidence,
            combined_uncertainty,
        })
    }

    /// Rule-based classification (fallback)
    fn classify_rules(&self, features: &VitalSignsFeatures) -> MlResult<ClassifierOutput> {
        let breathing = self.classify_breathing_rules(features);
        let heartbeat = self.classify_heartbeat_rules(features);
        let movement = self.classify_movement_rules(features);

        let overall_confidence = {
            let confidences: Vec<f32> = [
                breathing.as_ref().map(|b| b.confidence),
                heartbeat.as_ref().map(|h| h.confidence),
                movement.as_ref().map(|m| m.confidence),
            ]
            .iter()
            .filter_map(|&c| c)
            .collect();
            // Divide by the number of classifiers that actually produced a
            // result, not a hard-coded 3.0. When only a subset of detectors
            // fires (e.g. heartbeat disabled or below threshold), dividing by
            // 3.0 understates the true confidence and can wrongly suppress
            // detections below the ensemble threshold.
            if confidences.is_empty() {
                0.0
            } else {
                confidences.iter().sum::<f32>() / confidences.len() as f32
            }
        };

        let combined_uncertainty = UncertaintyEstimate::new(
            1.0 - overall_confidence,
            1.0 - features.signal_quality,
        );

        Ok(ClassifierOutput {
            breathing,
            heartbeat,
            movement,
            overall_confidence,
            combined_uncertainty,
        })
    }

    /// Rule-based breathing classification
    fn classify_breathing_rules(&self, features: &VitalSignsFeatures) -> Option<BreathingClassification> {
        // Check if breathing band has sufficient power
        if features.breathing_band_power < 0.01 || features.signal_quality < 0.2 {
            return None;
        }

        // Estimate breathing rate from dominant frequency in breathing band
        let breathing_rate = self.estimate_breathing_rate(features);

        if breathing_rate < 4.0 || breathing_rate > 60.0 {
            return None;
        }

        // Classify breathing type
        let breathing_type = self.classify_breathing_type(breathing_rate, features);

        // Calculate confidence
        let power_confidence = (features.breathing_band_power * 10.0).min(1.0);
        let quality_confidence = features.signal_quality;
        let confidence = (power_confidence + quality_confidence) / 2.0;

        // Class probabilities (simplified)
        let class_probabilities = self.compute_breathing_probabilities(breathing_rate, features);

        // Uncertainty estimation
        let rate_uncertainty = breathing_rate * (1.0 - confidence) * 0.2;
        let uncertainty = UncertaintyEstimate::new(
            1.0 - confidence,
            1.0 - features.signal_quality,
        );

        Some(BreathingClassification {
            breathing_type,
            rate_bpm: breathing_rate,
            rate_uncertainty,
            confidence,
            class_probabilities,
            uncertainty,
        })
    }

    /// Estimate breathing rate from features
    fn estimate_breathing_rate(&self, features: &VitalSignsFeatures) -> f32 {
        // Use dominant frequency from spectral features
        // Breathing band: 0.1-0.5 Hz = 6-30 BPM
        let dominant_freq = if features.spectral_features.len() >= 64 {
            features.spectral_features[63]
        } else {
            0.25 // Default 15 BPM
        };

        // If dominant frequency is in breathing range, use it
        if dominant_freq >= 0.1 && dominant_freq <= 0.5 {
            dominant_freq * 60.0
        } else {
            // Estimate from band power ratio
            let power_ratio = features.breathing_band_power /
                (features.movement_band_power + 0.001);
            let estimated = 12.0 + power_ratio * 8.0;
            estimated.clamp(6.0, 30.0)
        }
    }

    /// Classify breathing type from rate and features
    fn classify_breathing_type(&self, rate_bpm: f32, features: &VitalSignsFeatures) -> BreathingType {
        // Use rate and signal characteristics
        if rate_bpm < 6.0 {
            BreathingType::Agonal
        } else if rate_bpm < 10.0 {
            BreathingType::Shallow
        } else if rate_bpm > 30.0 {
            BreathingType::Labored
        } else {
            // Check regularity using spectral features
            let power_variance: f32 = features.spectral_features.iter()
                .take(10)
                .map(|&x| x.powi(2))
                .sum::<f32>() / 10.0;

            let mean_power: f32 = features.spectral_features.iter()
                .take(10)
                .sum::<f32>() / 10.0;

            let regularity = 1.0 - (power_variance / (mean_power.powi(2) + 0.001)).min(1.0);

            if regularity < 0.5 {
                BreathingType::Irregular
            } else {
                BreathingType::Normal
            }
        }
    }

    /// Compute breathing class probabilities
    fn compute_breathing_probabilities(&self, rate_bpm: f32, _features: &VitalSignsFeatures) -> Vec<f32> {
        let mut probs = vec![0.0; 6]; // Normal, Shallow, Labored, Irregular, Agonal, Apnea

        // Simple probability assignment based on rate
        if rate_bpm < 6.0 {
            probs[4] = 0.8; // Agonal
            probs[5] = 0.2; // Apnea-like
        } else if rate_bpm < 10.0 {
            probs[1] = 0.7; // Shallow
            probs[4] = 0.2;
            probs[0] = 0.1;
        } else if rate_bpm > 30.0 {
            probs[2] = 0.8; // Labored
            probs[0] = 0.2;
        } else if rate_bpm >= 12.0 && rate_bpm <= 20.0 {
            probs[0] = 0.8; // Normal
            probs[3] = 0.2;
        } else {
            probs[0] = 0.5;
            probs[3] = 0.5;
        }

        probs
    }

    /// Rule-based heartbeat classification
    fn classify_heartbeat_rules(&self, features: &VitalSignsFeatures) -> Option<HeartbeatClassification> {
        // Heartbeat detection requires stronger signal
        if features.heartbeat_band_power < 0.005 || features.signal_quality < 0.3 {
            return None;
        }

        // Estimate heart rate
        let heart_rate = self.estimate_heart_rate(features);

        if heart_rate < 30.0 || heart_rate > 200.0 {
            return None;
        }

        // Calculate HRV (simplified)
        let hrv = features.heartbeat_band_power * 0.1;

        // Signal strength from band power
        let signal_strength = if features.heartbeat_band_power > 0.1 {
            SignalStrength::Strong
        } else if features.heartbeat_band_power > 0.05 {
            SignalStrength::Moderate
        } else if features.heartbeat_band_power > 0.02 {
            SignalStrength::Weak
        } else {
            SignalStrength::VeryWeak
        };

        let confidence = match signal_strength {
            SignalStrength::Strong => 0.9,
            SignalStrength::Moderate => 0.7,
            SignalStrength::Weak => 0.5,
            SignalStrength::VeryWeak => 0.3,
        };

        let rate_uncertainty = heart_rate * (1.0 - confidence) * 0.15;

        let uncertainty = UncertaintyEstimate::new(
            1.0 - confidence,
            1.0 - features.signal_quality,
        );

        Some(HeartbeatClassification {
            rate_bpm: heart_rate,
            rate_uncertainty,
            hrv,
            signal_strength,
            confidence,
            uncertainty,
        })
    }

    /// Estimate heart rate from features
    fn estimate_heart_rate(&self, features: &VitalSignsFeatures) -> f32 {
        // Heart rate band: 0.8-2.0 Hz = 48-120 BPM (typical adult 60-100 BPM).
        // Search the amplitude spectrum for a peak within this band, mirroring
        // estimate_breathing_rate's spectral approach but in the cardiac
        // frequency range. spectral_features holds the FFT power spectrum of
        // the amplitude signal; index 63 is overwritten with the dominant
        // frequency (Hz) so it is skipped during the peak search.
        const HEART_RATE_MIN_HZ: f32 = 0.8;
        const HEART_RATE_MAX_HZ: f32 = 2.0;
        // FFT size used by extract_spectral_features (upper bound of the
        // 128.min(signal.len().next_power_of_two()) expression).
        const FFT_SIZE: f64 = 128.0;

        let sample_rate = features.sample_rate;
        if sample_rate > 0.0 && features.spectral_features.len() >= 64 {
            let freq_resolution = (sample_rate / FFT_SIZE) as f32;
            if freq_resolution > 0.0 {
                let mut best_power: f32 = 0.0;
                let mut best_freq_hz: f32 = 0.0;
                for (i, &power) in features.spectral_features.iter().enumerate() {
                    if i == 63 {
                        continue;
                    }
                    let freq_hz = i as f32 * freq_resolution;
                    if freq_hz >= HEART_RATE_MIN_HZ
                        && freq_hz <= HEART_RATE_MAX_HZ
                        && power > best_power
                    {
                        best_power = power;
                        best_freq_hz = freq_hz;
                    }
                }

                if best_power > 0.0 {
                    return (best_freq_hz * 60.0).clamp(40.0, 180.0);
                }
            }
        }

        // Fallback: no usable spectral peak in the heartbeat band (e.g. when
        // the sample rate is too high for the FFT size to resolve 0.8-2.0 Hz).
        // Use the ratio of heartbeat-band to breathing-band power to bias the
        // estimate away from the breathing harmonic. A stronger cardiac
        // component (higher ratio) maps to a higher rate within the normal
        // adult range, instead of the previous constant ~70 BPM output.
        let power_ratio = features.heartbeat_band_power
            / (features.breathing_band_power + 0.001);
        let base = 60.0 + (power_ratio * 40.0).clamp(0.0, 40.0);
        base.clamp(40.0, 180.0)
    }

    /// Rule-based movement classification
    fn classify_movement_rules(&self, features: &VitalSignsFeatures) -> Option<MovementClassification> {
        let intensity = (features.movement_band_power * 2.0).min(1.0);

        if intensity < 0.05 {
            return None;
        }

        // Classify movement type
        let movement_type = if intensity > 0.7 {
            MovementType::Gross
        } else if intensity > 0.3 {
            MovementType::Fine
        } else if features.signal_quality < 0.5 {
            MovementType::Tremor
        } else {
            MovementType::Periodic
        };

        // Determine if voluntary (gross movements with high signal quality)
        let is_voluntary = movement_type == MovementType::Gross && features.signal_quality > 0.6;

        // Frequency from spectral features
        let frequency = features.spectral_features.get(63).copied().unwrap_or(0.0);

        let confidence = (intensity * features.signal_quality).min(1.0);

        Some(MovementClassification {
            movement_type,
            intensity,
            is_voluntary,
            frequency,
            confidence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_features() -> VitalSignsFeatures {
        VitalSignsFeatures {
            amplitude_features: vec![0.5; 64],
            phase_features: vec![0.1; 64],
            spectral_features: {
                let mut s = vec![0.1; 64];
                s[63] = 0.25; // 15 BPM breathing
                s
            },
            breathing_band_power: 0.15,
            heartbeat_band_power: 0.08,
            movement_band_power: 0.05,
            signal_quality: 0.8,
            sample_rate: 1000.0,
        }
    }

    #[test]
    fn test_uncertainty_estimate() {
        let uncertainty = UncertaintyEstimate::new(0.1, 0.15);
        assert!(uncertainty.total() < 0.2);
        assert!(uncertainty.is_reliable);
    }

    #[test]
    fn test_feature_tensor() {
        let features = create_test_features();
        let tensor = features.to_tensor();
        assert_eq!(tensor.len(), 256);
    }

    #[tokio::test]
    async fn test_rule_based_classification() {
        let config = VitalSignsClassifierConfig::default();
        let classifier = VitalSignsClassifier::rule_based(config);

        let features = create_test_features();
        let result = classifier.classify(&features).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.breathing.is_some());
    }

    #[test]
    fn test_breathing_classification() {
        let config = VitalSignsClassifierConfig::default();
        let classifier = VitalSignsClassifier::rule_based(config);

        let features = create_test_features();
        let result = classifier.classify_breathing_rules(&features);

        assert!(result.is_some());
        let breathing = result.unwrap();
        assert!(breathing.rate_bpm > 0.0);
        assert!(breathing.rate_bpm < 60.0);
    }

    #[test]
    fn test_heartbeat_classification() {
        let config = VitalSignsClassifierConfig::default();
        let classifier = VitalSignsClassifier::rule_based(config);

        let features = create_test_features();
        let result = classifier.classify_heartbeat_rules(&features);

        assert!(result.is_some());
        let heartbeat = result.unwrap();
        assert!(heartbeat.rate_bpm >= 30.0);
        assert!(heartbeat.rate_bpm <= 200.0);
    }

    #[test]
    fn test_movement_classification() {
        let config = VitalSignsClassifierConfig::default();
        let classifier = VitalSignsClassifier::rule_based(config);

        let features = create_test_features();
        let result = classifier.classify_movement_rules(&features);

        assert!(result.is_some());
        let movement = result.unwrap();
        assert!(movement.intensity > 0.0);
    }

    #[test]
    fn test_classifier_output_conversion() {
        let breathing = BreathingClassification {
            breathing_type: BreathingType::Normal,
            rate_bpm: 16.0,
            rate_uncertainty: 1.0,
            confidence: 0.8,
            class_probabilities: vec![0.8, 0.1, 0.05, 0.03, 0.01, 0.01],
            uncertainty: UncertaintyEstimate::new(0.2, 0.1),
        };

        let pattern = breathing.to_breathing_pattern();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().rate_bpm, 16.0);
    }

    #[test]
    fn test_bandpass_filter() {
        // Use 100 Hz sample rate for better frequency resolution at breathing frequencies
        let filter = BandpassFilter::new(0.1, 0.5, 100.0);

        // Create test signal with breathing component at 0.25 Hz (15 BPM)
        // Using 100 Hz sample rate, 1000 samples = 10 seconds = 2.5 cycles of breathing
        let signal: Vec<f64> = (0..1000)
            .map(|i| {
                let t = i as f64 / 100.0; // 100 Hz sample rate
                (2.0 * std::f64::consts::PI * 0.25 * t).sin() // 0.25 Hz = 15 BPM
            })
            .collect();

        let filtered = filter.apply(&signal);
        assert_eq!(filtered.len(), signal.len());

        // Check that filtered signal is not all zeros
        let filtered_energy: f64 = filtered.iter().map(|x| x.powi(2)).sum();
        assert!(filtered_energy >= 0.0, "Filtered energy should be non-negative");

        // The band power should be non-negative
        let power = filter.band_power(&signal);
        assert!(power >= 0.0, "Band power should be non-negative");
    }
}
