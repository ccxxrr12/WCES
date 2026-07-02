//! Signal processing pipeline bridge — per-node CSI quality enhancement chain.
//!
//! Wraps multiple `wifi_densepose_signal` modules into a single per-frame
//! processing pipeline that runs before vital sign detection:
//!
//! # Pipeline
//! Raw CSI → PhaseSanitizer → HardwareNormalizer → HampelFilter → Clean CSI
//!        → MotionDetector → motion score + human presence
//!        → CoherenceState + GatePolicy → signal quality gate
//!
//! All modules use default configs tuned for ESP32-C5 HT20 (64 subcarriers).

use wifi_densepose_signal::phase_sanitizer::{
    PhaseSanitizer, PhaseSanitizerConfig,
};
use wifi_densepose_signal::hardware_norm::{
    HardwareNormalizer, HardwareType, CanonicalCsiFrame,
};
use wifi_densepose_signal::hampel::{hampel_filter, HampelConfig};
use wifi_densepose_signal::motion::{
    MotionDetector, MotionDetectorConfig, HumanDetectionResult,
};
use wifi_densepose_signal::features::{
    FeatureExtractor, FeatureExtractorConfig,
};
use wifi_densepose_signal::ruvsense::coherence::{CoherenceState, coherence_score};
use wifi_densepose_signal::ruvsense::coherence_gate::{
    GatePolicy, GateDecision, GatePolicyConfig,
};
// CsiData is needed by FeatureExtractor
use wifi_densepose_signal::csi_processor::CsiData;
use ndarray::Array2;

/// Output from one frame through the signal pipeline.
#[derive(Debug, Clone)]
pub struct SignalPipelineOutput {
    /// Hampel-filtered, canonical-56 normalized amplitudes.
    pub cleaned_amplitudes: Vec<f64>,
    /// Phase-sanitized, canonical-56 normalized phases.
    pub cleaned_phases: Vec<f64>,
    /// Raw hardware type detected.
    pub hardware_type: String,
    /// 0-1 motion intensity score.
    pub motion_score: f64,
    /// Whether a human is detected.
    pub human_detected: bool,
    /// Motion detection confidence [0-1].
    pub motion_confidence: f64,
    /// Per-subcarrier coherence score [0-1] (signal stability).
    pub coherence_score: f32,
    /// Quality gate decision: "accept" | "predict" | "reject" | "recalibrate".
    pub gate_decision: String,
    /// Whether the gate allows this frame to update downstream state.
    pub gate_allows_update: bool,
    /// Number of phase outliers removed.
    pub phase_outlier_count: usize,
    /// Motion analysis detail.
    pub estimated_velocity: f64,
    /// Number of consecutive low-coherence frames.
    pub stale_count: u64,
    /// Whether calibration (coherence reference) has been initialized.
    pub calibrated: bool,
}

/// Per-node signal processing pipeline.
pub struct SignalPipeline {
    phase_sanitizer: PhaseSanitizer,
    normalizer: HardwareNormalizer,
    hampel_config: HampelConfig,
    motion_detector: MotionDetector,
    feature_extractor: FeatureExtractor,
    coherence: CoherenceState,
    gate: GatePolicy,
    /// Number of frames processed.
    frame_count: u64,
    /// Whether the coherence reference has been calibrated.
    calibrated: bool,
    /// Calibration frame counter.
    calibration_count: usize,
}

impl SignalPipeline {
    /// Create a new signal pipeline with defaults tuned for ESP32-C5.
    pub fn new() -> Self {
        // Phase sanitizer: standard unwrap + outlier removal + light smoothing.
        let ps_config = PhaseSanitizerConfig {
            unwrapping_method: wifi_densepose_signal::phase_sanitizer::UnwrappingMethod::Standard,
            outlier_threshold: 3.0,
            smoothing_window: 5,
            enable_outlier_removal: true,
            enable_smoothing: true,
            enable_noise_filtering: false,
            noise_threshold: 0.1,
            phase_range: (-std::f64::consts::PI, std::f64::consts::PI),
        };
        let phase_sanitizer = PhaseSanitizer::new(ps_config)
            .expect("PhaseSanitizer::new with valid config should always succeed");

        let normalizer = HardwareNormalizer::new();

        let hampel_config = HampelConfig {
            half_window: 3,
            threshold: 3.0,
        };

        // Motion detector: sensitive to subtle breathing-related motion.
        let motion_config = MotionDetectorConfig {
            human_detection_threshold: 0.4,
            motion_threshold: 0.15,
            smoothing_factor: 0.1,
            amplitude_threshold: 0.15,
            phase_threshold: 0.2,
            history_size: 100,
            adaptive_threshold: true,
            amplitude_weight: 0.4,
            phase_weight: 0.3,
            motion_weight: 0.3,
        };
        let motion_detector = MotionDetector::new(motion_config);

        // Feature extractor: needed by MotionDetector.
        let feature_config = FeatureExtractorConfig {
            fft_size: 128,
            sampling_rate: 30.0,
            min_doppler_history: 10,
            enable_doppler: false, // skip Doppler for lower latency
        };
        let feature_extractor = FeatureExtractor::new(feature_config);

        // Coherence: track per-subcarrier z-score stability.
        let coherence = CoherenceState::new(56, 0.85);

        // Gate: Accept >= 0.85, Reject < 0.5, max 200 stale frames before recalibrate.
        let gate_config = GatePolicyConfig {
            accept_threshold: 0.85,
            reject_threshold: 0.5,
            max_stale_frames: 200,
            predict_only_noise: 3.0,
            adaptive: false,
        };
        let gate = GatePolicy::from_config(&gate_config);

        Self {
            phase_sanitizer,
            normalizer,
            hampel_config,
            motion_detector,
            feature_extractor,
            coherence,
            gate,
            frame_count: 0,
            calibrated: false,
            calibration_count: 0,
        }
    }

    /// Process one CSI frame through the full pipeline.
    ///
    /// Returns `None` if the input is empty or mismatched.
    pub fn process(&mut self, amplitudes: &[f64], phases: &[f64]) -> Option<SignalPipelineOutput> {
        if amplitudes.is_empty() || phases.is_empty() || amplitudes.len() != phases.len() {
            return None;
        }

        self.frame_count += 1;
        let n_sub = amplitudes.len();

        // ── Step 1: Phase sanitization ──
        // Reshape 1D &[f64] → Array2 (1 antenna × N subcarriers).
        let phase_array = Array2::from_shape_vec(
            (1, n_sub),
            phases.to_vec(),
        ).ok()?;

        let sanitized_phase = self.phase_sanitizer.sanitize_phase(&phase_array).ok()?;
        let stats = self.phase_sanitizer.get_statistics();
        let outlier_count = stats.outliers_removed;

        // Extract sanitized phases back to Vec<f64>.
        let clean_phases: Vec<f64> = sanitized_phase.row(0).iter().cloned().collect();

        // ── Step 2: Hardware normalization ──
        let hw = HardwareNormalizer::detect_hardware(n_sub);
        let canonical = self.normalizer.normalize(amplitudes, &clean_phases, hw).ok()?;

        // ── Step 3: Hampel outlier filter on amplitudes ──
        let amp_f64: Vec<f64> = canonical.amplitude.iter().map(|&a| a as f64).collect();
        let hampel_result = hampel_filter(&amp_f64, &self.hampel_config).ok()?;
        let cleaned_amps: Vec<f64> = hampel_result.filtered;
        let phase_f64: Vec<f64> = canonical.phase.iter().map(|&p| p as f64).collect();

        // ── Step 4: Coherence tracking ──
        let amp_f32: Vec<f32> = cleaned_amps.iter().map(|&a| a as f32).collect();
        let coh_score = if !self.calibrated {
            // Auto-calibrate: first 30 frames build the reference.
            self.calibration_count += 1;
            if self.calibration_count >= 30 {
                self.coherence.initialize(&amp_f32);
                self.calibrated = true;
            }
            1.0 // assume perfect coherence during calibration
        } else {
            self.coherence.update(&amp_f32).unwrap_or(1.0)
        };

        // ── Step 5: Quality gate ──
        let stale = self.coherence.stale_count();
        let decision = self.gate.evaluate(coh_score, stale);
        let gate_allows = decision.allows_update();
        let gate_name = format!("{:?}", decision);

        // ── Step 6: Motion detection ──
        // Build CsiData for FeatureExtractor from cleaned amplitudes/phases.
        let amp_arr = Array2::from_shape_vec((1, cleaned_amps.len()), cleaned_amps.clone()).ok()?;
        let phase_arr = Array2::from_shape_vec((1, phase_f64.len()), phase_f64.clone()).ok()?;
        let csi_data = CsiData::builder()
            .amplitude(amp_arr)
            .phase(phase_arr)
            .frequency(2.437e9) // 2.4 GHz WiFi channel 6
            .bandwidth(20e6)    // HT20
            .build()
            .ok()?;

        let features = self.feature_extractor.extract(&csi_data);
        let detection: HumanDetectionResult = self.motion_detector.detect_human(&features);

        Some(SignalPipelineOutput {
            cleaned_amplitudes: cleaned_amps,
            cleaned_phases: phase_f64,
            hardware_type: format!("{:?}", hw),
            motion_score: detection.motion_score,
            human_detected: detection.human_detected,
            motion_confidence: detection.confidence,
            coherence_score: coh_score,
            gate_decision: gate_name,
            gate_allows_update: gate_allows,
            phase_outlier_count: outlier_count,
            estimated_velocity: detection.motion_analysis.estimated_velocity,
            stale_count: stale,
            calibrated: self.calibrated,
        })
    }

    /// Whether the pipeline is calibrated and producing quality outputs.
    pub fn ready(&self) -> bool {
        self.calibrated && self.frame_count > 30
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_pipeline_constructs() {
        let pipe = SignalPipeline::new();
        assert!(!pipe.ready());
    }

    #[test]
    fn signal_pipeline_processes_valid_frame() {
        let mut pipe = SignalPipeline::new();
        let n = 64;
        let amps: Vec<f64> = (0..n).map(|i| 0.5 + 0.1 * (i as f64 * 0.3).sin()).collect();
        let phases: Vec<f64> = (0..n).map(|i| (i as f64 * 0.1).sin() * 0.5).collect();

        let output = pipe.process(&amps, &phases);
        assert!(output.is_some(), "Pipeline should process valid frame");
        let out = output.unwrap();
        assert_eq!(out.cleaned_amplitudes.len(), 56); // canonical-56
        assert_eq!(out.cleaned_phases.len(), 56);
        assert!(out.motion_score >= 0.0 && out.motion_score <= 1.0);
    }

    #[test]
    fn signal_pipeline_rejects_empty() {
        let mut pipe = SignalPipeline::new();
        assert!(pipe.process(&[], &[]).is_none());
    }
}
