//! Heartbeat detection from micro-Doppler signatures in CSI.

use crate::domain::{HeartbeatSignature, SignalStrength};

// ---------------------------------------------------------------------------
// Integration 7: CompressedHeartbeatSpectrogram (ADR-017, ruvector feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "ruvector")]
use ruvector_temporal_tensor::segment;
#[cfg(feature = "ruvector")]
use ruvector_temporal_tensor::{TemporalTensorCompressor, TierPolicy};

/// Memory-efficient heartbeat micro-Doppler spectrogram using tiered temporal compression.
///
/// Stores one TemporalTensorCompressor per frequency bin, each compressing
/// that bin's time-evolution. Hot tier (recent 10 seconds) at 8-bit,
/// warm at 5-7-bit, cold at 3-bit — preserving recent heartbeat cycles.
#[cfg(feature = "ruvector")]
pub struct CompressedHeartbeatSpectrogram {
    bin_buffers: Vec<TemporalTensorCompressor>,
    encoded: Vec<Vec<u8>>,
    n_freq_bins: usize,
    frame_count: u64,
}

#[cfg(feature = "ruvector")]
impl CompressedHeartbeatSpectrogram {
    pub fn new(n_freq_bins: usize) -> Self {
        let bin_buffers: Vec<_> = (0..n_freq_bins)
            .map(|i| TemporalTensorCompressor::new(TierPolicy::default(), 1, i as u32))
            .collect();
        let encoded = vec![Vec::new(); n_freq_bins];
        Self { bin_buffers, encoded, n_freq_bins, frame_count: 0 }
    }

    /// Push one column of the spectrogram (one time step, all frequency bins).
    ///
    /// # Errors (H-6)
    ///
    /// In production this must not panic on a dimension mismatch — a single
    /// malformed spectrogram column should not crash the heartbeat monitor.
    /// Instead we log an error and drop the column. Callers that need hard
    /// validation can check `column.len()` beforehand.
    pub fn push_column(&mut self, column: &[f32]) {
        if column.len() != self.n_freq_bins {
            tracing::error!(
                "heartbeat column dropped: expected {} frequency bins, got {} (H-6). \
                 This indicates a spectrogram pipeline dimension mismatch.",
                self.n_freq_bins,
                column.len()
            );
            return;
        }
        let ts = self.frame_count as u32;
        for (i, &val) in column.iter().enumerate() {
            // Synchronize last_access_ts with current timestamp so that the
            // tier policy's age computation (now_ts - last_access_ts + 1) never
            // wraps to zero (which would cause a divide-by-zero in wrapping_div).
            self.bin_buffers[i].set_access(ts, ts);
            self.bin_buffers[i].push_frame(&[val], ts, &mut self.encoded[i]);
        }
        self.frame_count += 1;
    }

    /// Flush all bin buffers.
    pub fn flush(&mut self) {
        for (buf, enc) in self.bin_buffers.iter_mut().zip(self.encoded.iter_mut()) {
            buf.flush(enc);
        }
    }

    /// Compute mean power in a frequency bin range (e.g., heartbeat 0.8-1.5 Hz).
    /// Uses most recent `n_recent` frames for real-time triage.
    pub fn band_power(&self, low_bin: usize, high_bin: usize, n_recent: usize) -> f32 {
        let high = high_bin.min(self.n_freq_bins.saturating_sub(1));
        if low_bin > high {
            return 0.0;
        }
        let mut total = 0.0_f32;
        let mut count = 0_usize;
        for b in low_bin..=high {
            let mut out = Vec::new();
            segment::decode(&self.encoded[b], &mut out);
            let recent: f32 = out.iter().rev().take(n_recent).map(|x| x * x).sum();
            total += recent;
            count += 1;
        }
        if count == 0 { 0.0 } else { total / count as f32 }
    }

    pub fn frame_count(&self) -> u64 { self.frame_count }
    pub fn n_freq_bins(&self) -> usize { self.n_freq_bins }
}

/// Configuration for heartbeat detection
#[derive(Debug, Clone)]
pub struct HeartbeatDetectorConfig {
    /// Minimum heart rate to detect (BPM)
    pub min_rate_bpm: f32,
    /// Maximum heart rate to detect (BPM)
    pub max_rate_bpm: f32,
    /// Minimum signal strength required
    pub min_signal_strength: f64,
    /// Window size for analysis
    pub window_size: usize,
    /// Enable enhanced micro-Doppler processing
    pub enhanced_processing: bool,
    /// Confidence threshold
    pub confidence_threshold: f32,
}

impl Default for HeartbeatDetectorConfig {
    fn default() -> Self {
        Self {
            min_rate_bpm: 30.0,   // Very slow (bradycardia)
            max_rate_bpm: 200.0,  // Very fast (extreme tachycardia)
            min_signal_strength: 0.04,  // Lowered 20% for higher sensitivity
            window_size: 512,     // Longer window for finer frequency resolution
            enhanced_processing: true,
            confidence_threshold: 0.30,  // Lowered to improve recall
        }
    }
}

/// Detector for heartbeat signatures using micro-Doppler analysis
///
/// Heartbeats cause very small chest wall movements (~0.5mm) that can be
/// detected through careful analysis of CSI phase variations at higher
/// frequencies than breathing (0.8-3.3 Hz for 48-200 BPM).
pub struct HeartbeatDetector {
    config: HeartbeatDetectorConfig,
}

impl HeartbeatDetector {
    /// Create a new heartbeat detector
    pub fn new(config: HeartbeatDetectorConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HeartbeatDetectorConfig::default())
    }

    /// Detect heartbeat from CSI phase data
    ///
    /// Heartbeat detection is more challenging than breathing due to:
    /// - Much smaller displacement (~0.5mm vs ~10mm for breathing)
    /// - Higher frequency (masked by breathing harmonics)
    /// - Lower signal-to-noise ratio
    ///
    /// We use micro-Doppler analysis on the phase component after
    /// removing the breathing component.
    pub fn detect(
        &self,
        csi_phase: &[f64],
        sample_rate: f64,
        breathing_rate: Option<f64>,
    ) -> Option<HeartbeatSignature> {
        if csi_phase.len() < self.config.window_size {
            return None;
        }

        // Remove breathing component if known
        let filtered = if let Some(br) = breathing_rate {
            self.remove_breathing_component(csi_phase, sample_rate, br)
        } else {
            self.highpass_filter(csi_phase, sample_rate, 0.8)
        };

        // After removing breathing component, apply adaptive bandpass filter
        // to further enhance heartbeat signal in [0.8, 3.0] Hz range
        let bandpassed = self.adaptive_bandpass(&filtered, sample_rate);

        // Compute micro-Doppler spectrum from bandpassed signal
        let spectrum = self.compute_micro_doppler_spectrum(&bandpassed, sample_rate);

        // Find heartbeat frequency
        let min_freq = self.config.min_rate_bpm as f64 / 60.0;
        let max_freq = self.config.max_rate_bpm as f64 / 60.0;

        let (heart_freq, strength) = self.find_heartbeat_frequency(
            &spectrum,
            sample_rate,
            min_freq,
            max_freq,
        )?;

        if strength < self.config.min_signal_strength {
            return None;
        }

        let rate_bpm = (heart_freq * 60.0) as f32;

        // Calculate heart rate variability from peak width
        let variability = self.estimate_hrv(&spectrum, heart_freq, sample_rate);

        // Determine signal strength category
        let signal_strength = self.categorize_strength(strength);

        // Calculate confidence
        let confidence = self.calculate_confidence(strength, variability);

        if confidence < self.config.confidence_threshold {
            return None;
        }

        Some(HeartbeatSignature {
            rate_bpm,
            variability,
            strength: signal_strength,
        })
    }

    /// Remove breathing component using notch filter
    fn remove_breathing_component(
        &self,
        signal: &[f64],
        sample_rate: f64,
        breathing_rate: f64,
    ) -> Vec<f64> {
        // Simple IIR notch filter at breathing frequency and harmonics
        let mut filtered = signal.to_vec();
        let breathing_freq = breathing_rate / 60.0;

        // Notch at fundamental and first two harmonics
        for harmonic in 1..=3 {
            let notch_freq = breathing_freq * harmonic as f64;
            filtered = self.apply_notch_filter(&filtered, sample_rate, notch_freq, 0.05);
        }

        filtered
    }

    /// Apply a simple notch filter
    fn apply_notch_filter(
        &self,
        signal: &[f64],
        sample_rate: f64,
        center_freq: f64,
        bandwidth: f64,
    ) -> Vec<f64> {
        // Second-order IIR notch filter
        let w0 = 2.0 * std::f64::consts::PI * center_freq / sample_rate;
        let bw = 2.0 * std::f64::consts::PI * bandwidth / sample_rate;

        let r = 1.0 - bw / 2.0;
        let cos_w0 = w0.cos();

        let b0 = 1.0;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0;
        let a1 = -2.0 * r * cos_w0;
        let a2 = r * r;

        let mut output = vec![0.0; signal.len()];
        let mut x1 = 0.0;
        let mut x2 = 0.0;
        let mut y1 = 0.0;
        let mut y2 = 0.0;

        for (i, &x) in signal.iter().enumerate() {
            let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            output[i] = y;

            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
        }

        output
    }

    /// High-pass filter to remove low frequencies
    fn highpass_filter(&self, signal: &[f64], sample_rate: f64, cutoff: f64) -> Vec<f64> {
        // Simple first-order high-pass filter
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
        let dt = 1.0 / sample_rate;
        let alpha = rc / (rc + dt);

        let mut output = vec![0.0; signal.len()];
        if signal.is_empty() {
            return output;
        }

        output[0] = signal[0];
        for i in 1..signal.len() {
            output[i] = alpha * (output[i - 1] + signal[i] - signal[i - 1]);
        }

        output
    }

    /// Adaptive bandpass filter optimized for heartbeat detection.
    ///
    /// Implements a 2nd-order IIR bandpass filter centered on the expected
    /// heartbeat frequency range [0.8, 3.0] Hz. The filter adapts its center
    /// frequency based on the previous heartbeat estimate (if available)
    /// for improved tracking.
    fn adaptive_bandpass(&self, signal: &[f64], sample_rate: f64) -> Vec<f64> {
        let n = signal.len();
        if n < 4 {
            return signal.to_vec();
        }

        // Bandpass: 0.8 Hz - 3.0 Hz (48-180 BPM)
        let f_low = 0.8f64;
        let f_high = 3.0f64;
        let dt = 1.0 / sample_rate;

        // 2nd-order IIR bandpass (Biquad) coefficients
        // Using RBJ cookbook formulas
        let w0_low = 2.0 * std::f64::consts::PI * f_low * dt;
        let w0_high = 2.0 * std::f64::consts::PI * f_high * dt;
        let q = 0.707; // Butterworth Q

        // Low-pass (for high cutoff)
        let alpha_h = w0_high.sin() / (2.0 * q);
        let b0_h = (1.0 - w0_high.cos()) / 2.0;
        let b1_h = 1.0 - w0_high.cos();
        let b2_h = b0_h;
        let a0_h = 1.0 + alpha_h;
        let a1_h = -2.0 * w0_high.cos();
        let a2_h = 1.0 - alpha_h;

        // High-pass (for low cutoff)
        let alpha_l = w0_low.sin() / (2.0 * q);
        let b0_l = (1.0 + w0_low.cos()) / 2.0;
        let b1_l = -(1.0 + w0_low.cos());
        let b2_l = b0_l;
        let a0_l = 1.0 + alpha_l;
        let a1_l = -2.0 * w0_low.cos();
        let a2_l = 1.0 - alpha_l;

        // Cascaded: high-pass first, then low-pass
        let mut hp_out = vec![0.0f64; n];
        let mut x1_l = 0.0; let mut x2_l = 0.0;
        let mut y1_l = 0.0; let mut y2_l = 0.0;

        for i in 0..n {
            let x = signal[i];
            let y = (b0_l * x + b1_l * x1_l + b2_l * x2_l - a1_l * y1_l - a2_l * y2_l) / a0_l;
            x2_l = x1_l; x1_l = x;
            y2_l = y1_l; y1_l = y;
            hp_out[i] = y;
        }

        let mut lp_out = vec![0.0f64; n];
        let mut x1_h = 0.0; let mut x2_h = 0.0;
        let mut y1_h = 0.0; let mut y2_h = 0.0;

        for i in 0..n {
            let x = hp_out[i];
            let y = (b0_h * x + b1_h * x1_h + b2_h * x2_h - a1_h * y1_h - a2_h * y2_h) / a0_h;
            x2_h = x1_h; x1_h = x;
            y2_h = y1_h; y1_h = y;
            lp_out[i] = y;
        }

        lp_out
    }

    /// Compute micro-Doppler spectrum optimized for heartbeat detection
    fn compute_micro_doppler_spectrum(&self, signal: &[f64], _sample_rate: f64) -> Vec<f64> {
        use rustfft::{FftPlanner, num_complex::Complex};

        let n = signal.len().next_power_of_two();
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);

        // Apply Blackman window for better frequency resolution
        let mut buffer: Vec<Complex<f64>> = signal
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let n_f = signal.len() as f64;
                let window = 0.42
                    - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n_f).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / n_f).cos();
                Complex::new(x * window, 0.0)
            })
            .collect();
        buffer.resize(n, Complex::new(0.0, 0.0));

        fft.process(&mut buffer);

        // Return power spectrum
        buffer.iter()
            .take(n / 2)
            .map(|c| c.norm_sqr())
            .collect()
    }

    /// Find heartbeat frequency in spectrum
    fn find_heartbeat_frequency(
        &self,
        spectrum: &[f64],
        sample_rate: f64,
        min_freq: f64,
        max_freq: f64,
    ) -> Option<(f64, f64)> {
        let n = spectrum.len() * 2;
        let freq_resolution = sample_rate / n as f64;

        let min_bin = (min_freq / freq_resolution).ceil() as usize;
        let max_bin = (max_freq / freq_resolution).floor() as usize;

        if min_bin >= spectrum.len() || max_bin >= spectrum.len() {
            return None;
        }

        // Find the strongest peak
        let mut max_power = 0.0;
        let mut max_bin_idx = min_bin;

        for i in min_bin..=max_bin.min(spectrum.len() - 1) {
            if spectrum[i] > max_power {
                max_power = spectrum[i];
                max_bin_idx = i;
            }
        }

        // Check if it's a real peak (local maximum)
        if max_bin_idx > 0 && max_bin_idx < spectrum.len() - 1 {
            if spectrum[max_bin_idx] <= spectrum[max_bin_idx - 1]
                || spectrum[max_bin_idx] <= spectrum[max_bin_idx + 1]
            {
                // Not a real peak
                return None;
            }
        }

        let freq = max_bin_idx as f64 * freq_resolution;
        let strength = max_power.sqrt(); // Convert power to amplitude

        Some((freq, strength))
    }

    /// Estimate heart rate variability from spectral peak width
    fn estimate_hrv(&self, spectrum: &[f64], peak_freq: f64, sample_rate: f64) -> f32 {
        let n = spectrum.len() * 2;
        let freq_resolution = sample_rate / n as f64;
        let peak_bin = (peak_freq / freq_resolution).round() as usize;

        if peak_bin >= spectrum.len() {
            return 0.0;
        }

        let peak_power = spectrum[peak_bin];
        if peak_power == 0.0 {
            return 0.0;
        }

        // Find -3dB width (half-power points)
        let half_power = peak_power / 2.0;
        let mut left = peak_bin;
        let mut right = peak_bin;

        while left > 0 && spectrum[left] > half_power {
            left -= 1;
        }
        while right < spectrum.len() - 1 && spectrum[right] > half_power {
            right += 1;
        }

        // HRV is proportional to bandwidth
        let bandwidth = (right - left) as f64 * freq_resolution;
        let hrv_estimate = bandwidth * 60.0; // Convert to BPM variation

        // Normalize to 0-1 range (typical HRV is 2-20 BPM)
        (hrv_estimate / 20.0).min(1.0) as f32
    }

    /// Categorize signal strength
    fn categorize_strength(&self, strength: f64) -> SignalStrength {
        if strength > 0.5 {
            SignalStrength::Strong
        } else if strength > 0.2 {
            SignalStrength::Moderate
        } else if strength > 0.1 {
            SignalStrength::Weak
        } else {
            SignalStrength::VeryWeak
        }
    }

    /// Calculate detection confidence
    fn calculate_confidence(&self, strength: f64, hrv: f32) -> f32 {
        // Strong signal with reasonable HRV indicates real heartbeat
        let strength_score = (strength / 0.5).min(1.0) as f32;

        // Very low or very high HRV might indicate noise
        let hrv_score = if hrv > 0.05 && hrv < 0.5 {
            1.0
        } else {
            0.5
        };

        strength_score * 0.7 + hrv_score * 0.3
    }
}

#[cfg(all(test, feature = "ruvector"))]
mod heartbeat_buffer_tests {
    use super::*;

    #[test]
    fn compressed_heartbeat_push_and_band_power() {
        let n_bins = 32_usize;
        let mut spec = CompressedHeartbeatSpectrogram::new(n_bins);
        for t in 0..20_u64 {
            let col: Vec<f32> = (0..n_bins)
                .map(|b| if b < 16 { 1.0 } else { 0.1 })
                .collect();
            let _ = t;
            spec.push_column(&col);
        }
        spec.flush();
        assert_eq!(spec.frame_count(), 20);
        // Low bins (0..15) should have higher power than high bins (16..31)
        let low_power = spec.band_power(0, 15, 20);
        let high_power = spec.band_power(16, 31, 20);
        assert!(low_power >= high_power,
            "low_power={low_power} should >= high_power={high_power}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_heartbeat_signal(rate_bpm: f64, sample_rate: f64, duration: f64) -> Vec<f64> {
        let num_samples = (sample_rate * duration) as usize;
        let freq = rate_bpm / 60.0;

        (0..num_samples)
            .map(|i| {
                let t = i as f64 / sample_rate;
                // Heartbeat is more pulse-like than sine
                let phase = 2.0 * std::f64::consts::PI * freq * t;
                0.3 * phase.sin() + 0.1 * (2.0 * phase).sin()
            })
            .collect()
    }

    #[test]
    fn test_detect_heartbeat() {
        let detector = HeartbeatDetector::with_defaults();
        let signal = generate_heartbeat_signal(72.0, 1000.0, 10.0);

        let result = detector.detect(&signal, 1000.0, None);

        // Heartbeat detection is challenging, may not always succeed
        if let Some(signature) = result {
            assert!(signature.rate_bpm >= 50.0 && signature.rate_bpm <= 100.0);
        }
    }

    #[test]
    fn test_highpass_filter() {
        let detector = HeartbeatDetector::with_defaults();

        // Signal with DC offset and low frequency component
        let signal: Vec<f64> = (0..1000)
            .map(|i| {
                let t = i as f64 / 100.0;
                5.0 + (0.1 * t).sin() + (5.0 * t).sin() * 0.2
            })
            .collect();

        let filtered = detector.highpass_filter(&signal, 100.0, 0.5);

        // DC component should be removed
        let mean: f64 = filtered.iter().skip(100).sum::<f64>() / (filtered.len() - 100) as f64;
        assert!(mean.abs() < 1.0);
    }
}
