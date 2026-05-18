//! Pure signal processing functions extracted from `main.rs`.
//!
//! These functions perform computation on sensing data without mutating
//! `AppStateInner`.  They are stateless, side-effect free, and only depend on
//! their explicit input parameters.

use std::collections::VecDeque;

use crate::types::{
    BoundingBox, ClassificationInfo, Esp32Frame, FeatureInfo, PersonDetection,
    PoseKeypoint, SensingUpdate, SignalField,
};

// ── Signal field generation ──────────────────────────────────────────────────

/// Generate a signal field that reflects where motion and signal changes are occurring.
///
/// Instead of a fixed-animation circle, this function uses the actual sensing data:
/// - `subcarrier_variances`: per-subcarrier variance computed from the frame history.
///   High-variance subcarriers indicate spatial directions where the signal is disrupted.
/// - `motion_score`: overall motion intensity [0, 1].
/// - `breathing_rate_hz`: estimated breathing rate in Hz; if > 0, adds a breathing ring.
/// - `signal_quality`: overall quality metric [0, 1] modulates field brightness.
///
/// The field grid is 20×20 cells representing a top-down view of the room.
/// Hotspots are derived from the subcarrier index (treated as an angular bin) so that
/// subcarriers with the highest variance produce peaks at the corresponding directions.
pub(crate) fn generate_signal_field(
    _mean_rssi: f64,
    motion_score: f64,
    breathing_rate_hz: f64,
    signal_quality: f64,
    subcarrier_variances: &[f64],
) -> SignalField {
    let grid = 20usize;
    let mut values = vec![0.0f64; grid * grid];
    let center = (grid as f64 - 1.0) / 2.0;

    // Normalise subcarrier variances to [0, 1].
    let max_var = subcarrier_variances.iter().cloned().fold(0.0f64, f64::max);
    let norm_factor = if max_var > 1e-9 { max_var } else { 1.0 };

    // For each cell, accumulate contributions from all subcarriers.
    // Each subcarrier k is assigned an angular direction proportional to its index
    // so that different subcarriers illuminate different regions of the room.
    let n_sub = subcarrier_variances.len().max(1);
    for (k, &var) in subcarrier_variances.iter().enumerate() {
        let weight = (var / norm_factor) * motion_score;
        if weight < 1e-6 {
            continue;
        }
        // Map subcarrier index to an angle across the full 2π sweep.
        let angle = (k as f64 / n_sub as f64) * 2.0 * std::f64::consts::PI;
        // Place the hotspot at a distance proportional to the weight, capped at 40% of
        // the grid radius so it stays within the room model.
        let radius = center * 0.8 * weight.sqrt();
        let hx = center + radius * angle.cos();
        let hz = center + radius * angle.sin();

        for z in 0..grid {
            for x in 0..grid {
                let dx = x as f64 - hx;
                let dz = z as f64 - hz;
                let dist2 = dx * dx + dz * dz;
                // Gaussian blob centred on the hotspot; spread scales with weight.
                let spread = (0.5 + weight * 2.0).max(0.5);
                values[z * grid + x] += weight * (-dist2 / (2.0 * spread * spread)).exp();
            }
        }
    }

    // Base radial attenuation from the router assumed at grid centre.
    for z in 0..grid {
        for x in 0..grid {
            let dx = x as f64 - center;
            let dz = z as f64 - center;
            let dist = (dx * dx + dz * dz).sqrt();
            let base = signal_quality * (-dist * 0.12).exp();
            values[z * grid + x] += base * 0.3;
        }
    }

    // Breathing ring: if a breathing rate was estimated add a faint annular highlight
    // at a radius corresponding to typical chest-wall displacement range.
    if breathing_rate_hz > 0.05 {
        let ring_r = center * 0.55;
        let ring_width = 1.8f64;
        for z in 0..grid {
            for x in 0..grid {
                let dx = x as f64 - center;
                let dz = z as f64 - center;
                let dist = (dx * dx + dz * dz).sqrt();
                let ring_val = 0.08 * (-(dist - ring_r).powi(2) / (2.0 * ring_width * ring_width)).exp();
                values[z * grid + x] += ring_val;
            }
        }
    }

    // Clamp and normalise to [0, 1].
    let field_max = values.iter().cloned().fold(0.0f64, f64::max);
    let scale = if field_max > 1e-9 { 1.0 / field_max } else { 1.0 };
    for v in &mut values {
        *v = (*v * scale).clamp(0.0, 1.0);
    }

    SignalField {
        grid_size: [grid, 1, grid],
        values,
    }
}

// ── Feature extraction from ESP32 frame ──────────────────────────────────────

/// Estimate breathing rate in Hz from the amplitude time series stored in `frame_history`.
///
/// Approach:
/// 1. Build a scalar time series by computing the mean amplitude of each historical frame.
/// 2. Run a peak-detection pass: count rising-edge zero-crossings of the de-meaned signal.
/// 3. Convert the crossing rate to Hz, clipped to the physiological range 0.1—.5 Hz
///    (12–0 breaths/min).
///
/// For accuracy the function additionally applies a simple 3-tap Goertzel-style power
/// estimate at evenly-spaced candidate frequencies in the breathing band and returns
/// the candidate with the highest energy.
pub(crate) fn estimate_breathing_rate_hz(frame_history: &VecDeque<Vec<f64>>, sample_rate_hz: f64) -> f64 {
    let n = frame_history.len();
    if n < 6 {
        return 0.0;
    }

    // Build scalar time series: mean amplitude per frame.
    let series: Vec<f64> = frame_history.iter()
        .map(|amps| {
            if amps.is_empty() { 0.0 } else { amps.iter().sum::<f64>() / amps.len() as f64 }
        })
        .collect();

    let mean_s = series.iter().sum::<f64>() / n as f64;
    // De-mean.
    let detrended: Vec<f64> = series.iter().map(|x| x - mean_s).collect();

    // Goertzel power at candidate frequencies in the breathing band [0.1, 0.5] Hz.
    // We evaluate 9 candidate frequencies uniformly spaced in that band.
    let n_candidates = 9usize;
    let f_low = 0.1f64;
    let f_high = 0.5f64;
    let mut best_freq = 0.0f64;
    let mut best_power = 0.0f64;

    for i in 0..n_candidates {
        let freq = f_low + (f_high - f_low) * i as f64 / (n_candidates - 1).max(1) as f64;
        let omega = 2.0 * std::f64::consts::PI * freq / sample_rate_hz;
        let coeff = 2.0 * omega.cos();
        let mut s_prev2 = 0.0f64;
        let mut s_prev1 = 0.0f64;
        for &x in &detrended {
            let s = x + coeff * s_prev1 - s_prev2;
            s_prev2 = s_prev1;
            s_prev1 = s;
        }
        // Goertzel magnitude squared.
        let power = s_prev2 * s_prev2 + s_prev1 * s_prev1 - coeff * s_prev1 * s_prev2;
        if power > best_power {
            best_power = power;
            best_freq = freq;
        }
    }

    // Only report a breathing rate if the Goertzel energy is meaningfully above noise.
    // Threshold: power must exceed 10× the average power across all candidates.
    let avg_power = {
        let mut total = 0.0f64;
        for i in 0..n_candidates {
            let freq = f_low + (f_high - f_low) * i as f64 / (n_candidates - 1).max(1) as f64;
            let omega = 2.0 * std::f64::consts::PI * freq / sample_rate_hz;
            let coeff = 2.0 * omega.cos();
            let mut s_prev2 = 0.0f64;
            let mut s_prev1 = 0.0f64;
            for &x in &detrended {
                let s = x + coeff * s_prev1 - s_prev2;
                s_prev2 = s_prev1;
                s_prev1 = s;
            }
            total += s_prev2 * s_prev2 + s_prev1 * s_prev1 - coeff * s_prev1 * s_prev2;
        }
        total / n_candidates as f64
    };

    if best_power > avg_power * 3.0 {
        best_freq.clamp(f_low, f_high)
    } else {
        0.0
    }
}

/// Compute per-subcarrier variance across the sliding window of `frame_history`.
///
/// For each subcarrier index `k`, returns `Var[A_k]` over all stored frames.
/// This captures spatial signal variation; subcarriers whose amplitude fluctuates
/// heavily across time correspond to directions with motion.
pub(crate) fn compute_subcarrier_variances(frame_history: &VecDeque<Vec<f64>>, n_sub: usize) -> Vec<f64> {
    if frame_history.is_empty() || n_sub == 0 {
        return vec![0.0; n_sub];
    }

    let n_frames = frame_history.len() as f64;
    let mut means = vec![0.0f64; n_sub];
    let mut sq_means = vec![0.0f64; n_sub];

    for frame in frame_history.iter() {
        for k in 0..n_sub {
            let a = if k < frame.len() { frame[k] } else { 0.0 };
            means[k] += a;
            sq_means[k] += a * a;
        }
    }

    (0..n_sub)
        .map(|k| {
            let mean = means[k] / n_frames;
            let sq_mean = sq_means[k] / n_frames;
            (sq_mean - mean * mean).max(0.0)
        })
        .collect()
}

/// Select top-K most sensitive subcarriers by temporal variance.
/// Returns indices of the most responsive subcarriers for vital sign detection.
pub(crate) fn select_sensitive_subcarriers(frame_history: &VecDeque<Vec<f64>>, n_sub: usize, top_k: usize) -> Vec<usize> {
    let variances = compute_subcarrier_variances(frame_history, n_sub);
    if variances.is_empty() { return Vec::new(); }
    let mut ranked: Vec<(usize, f64)> = variances.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.iter().take(top_k.min(n_sub)).map(|(idx, _)| *idx).collect()
}

/// Extract amplitudes for selected subcarriers only, for improved SNR vital sign detection.
pub(crate) fn extract_selected_amplitudes(amplitudes: &[f64], selected: &[usize]) -> Vec<f64> {
    selected.iter().filter_map(|&i| amplitudes.get(i).copied()).collect()
}

/// Extract features from the current ESP32 frame, enhanced with temporal context from
/// `frame_history`.
///
/// Improvements over the previous single-frame approach:
///
/// - **Variance**: computed as the mean of per-subcarrier temporal variance across the
///   sliding window, not just the intra-frame spatial variance.
/// - **Motion detection**: uses frame-to-frame temporal difference (mean L2 change
///   between the current frame and the previous frame) normalised by signal amplitude,
///   so that actual changes are detected rather than just a threshold on the current frame.
/// - **Breathing rate**: estimated via Goertzel filter bank on the 0.1—.5 Hz band of
///   the amplitude time series.
/// - **Signal quality**: based on SNR estimate (RSSI —noise floor) and subcarrier
///   variance stability.
/// Returns (features, raw_classification, breathing_rate_hz, sub_variances, raw_motion_score).
pub(crate) fn extract_features_from_frame(
    frame: &Esp32Frame,
    frame_history: &VecDeque<Vec<f64>>,
    sample_rate_hz: f64,
) -> (FeatureInfo, ClassificationInfo, f64, Vec<f64>, f64) {
    let n_sub = frame.amplitudes.len().max(1);
    let n = n_sub as f64;
    let mean_amp: f64 = frame.amplitudes.iter().sum::<f64>() / n;
    let mean_rssi = frame.rssi as f64;

    // ── Intra-frame subcarrier variance (spatial spread across subcarriers) ──
    let intra_variance: f64 = frame.amplitudes.iter()
        .map(|a| (a - mean_amp).powi(2))
        .sum::<f64>() / n;

    // ── Temporal (sliding-window) per-subcarrier variance ──
    let sub_variances = compute_subcarrier_variances(frame_history, n_sub);
    let temporal_variance: f64 = if sub_variances.is_empty() {
        intra_variance
    } else {
        sub_variances.iter().sum::<f64>() / sub_variances.len() as f64
    };

    // Use the larger of intra-frame and temporal variance as the reported variance.
    let variance = intra_variance.max(temporal_variance);

    // ── Spectral power ──
    let spectral_power: f64 = frame.amplitudes.iter().map(|a| a * a).sum::<f64>() / n;

    // ── Motion band power (upper half of subcarriers, high spatial frequency) ──
    let half = frame.amplitudes.len() / 2;
    let motion_band_power = if half > 0 {
        frame.amplitudes[half..].iter()
            .map(|a| (a - mean_amp).powi(2))
            .sum::<f64>() / (frame.amplitudes.len() - half) as f64
    } else {
        0.0
    };

    // ── Breathing band power (lower half of subcarriers, low spatial frequency) ──
    let breathing_band_power = if half > 0 {
        frame.amplitudes[..half].iter()
            .map(|a| (a - mean_amp).powi(2))
            .sum::<f64>() / half as f64
    } else {
        0.0
    };

    // ── Dominant frequency via peak subcarrier index ──
    let peak_idx = frame.amplitudes.iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let dominant_freq_hz = peak_idx as f64 * 0.05;

    // ── Change point detection (threshold-crossing count in current frame) ──
    let threshold = mean_amp * 1.2;
    let change_points = frame.amplitudes.windows(2)
        .filter(|w| (w[0] < threshold) != (w[1] < threshold))
        .count();

    // ── Motion score: sliding-window temporal difference ──
    // Compare current frame against the most recent historical frame.
    // The difference is normalised by the mean amplitude to be scale-invariant.
    let temporal_motion_score = if let Some(prev_frame) = frame_history.back() {
        let n_cmp = n_sub.min(prev_frame.len());
        if n_cmp > 0 {
            let diff_energy: f64 = (0..n_cmp)
                .map(|k| (frame.amplitudes[k] - prev_frame[k]).powi(2))
                .sum::<f64>() / n_cmp as f64;
            // Normalise by mean squared amplitude to get a dimensionless ratio.
            let ref_energy = mean_amp * mean_amp + 1e-9;
            (diff_energy / ref_energy).sqrt().clamp(0.0, 1.0)
        } else {
            0.0
        }
    } else {
        // No history yet —fall back to intra-frame variance-based estimate.
        (intra_variance / (mean_amp * mean_amp + 1e-9)).sqrt().clamp(0.0, 1.0)
    };

    // Blend temporal motion with variance-based motion for robustness.
    // Also factor in motion_band_power and change_points for ESP32 real-world sensitivity.
    let variance_motion = (temporal_variance / 10.0).clamp(0.0, 1.0);
    let mbp_motion = (motion_band_power / 25.0).clamp(0.0, 1.0);
    let cp_motion = (change_points as f64 / 15.0).clamp(0.0, 1.0);
    let motion_score = (temporal_motion_score * 0.4 + variance_motion * 0.2 + mbp_motion * 0.25 + cp_motion * 0.15).clamp(0.0, 1.0);

    // ── Signal quality metric ──
    // Based on estimated SNR (RSSI relative to noise floor) and subcarrier consistency.
    let snr_db = (frame.rssi as f64 - frame.noise_floor as f64).max(0.0);
    let snr_quality = (snr_db / 40.0).clamp(0.0, 1.0); // 40 dB →quality = 1.0
    // Penalise quality when temporal variance is very high (unstable signal).
    let stability = (1.0 - (temporal_variance / (mean_amp * mean_amp + 1e-9)).clamp(0.0, 1.0)).max(0.0);
    let signal_quality = (snr_quality * 0.6 + stability * 0.4).clamp(0.0, 1.0);

    // ── Breathing rate estimation ──
    let breathing_rate_hz = estimate_breathing_rate_hz(frame_history, sample_rate_hz);

    let features = FeatureInfo {
        mean_rssi,
        variance,
        motion_band_power,
        breathing_band_power,
        dominant_freq_hz,
        change_points,
        spectral_power,
    };

    // Return raw motion_score and signal_quality —classification is done by
    // `smooth_and_classify()` which has access to EMA state and hysteresis.
    let raw_classification = ClassificationInfo {
        motion_level: raw_classify(motion_score),
        presence: motion_score > 0.04,
        confidence: (0.4 + signal_quality * 0.3 + motion_score * 0.3).clamp(0.0, 1.0),
    };

    (features, raw_classification, breathing_rate_hz, sub_variances, motion_score)
}

/// Simple threshold classification (no smoothing) —used as the "raw" input.
pub(crate) fn raw_classify(score: f64) -> String {
    // Lowered thresholds for Windows WiFi mode (RSSI-based, less sensitive than CSI)
    if score > 0.15 { "active".into() }
    else if score > 0.08 { "present_moving".into() }
    else if score > 0.03 { "present_still".into() }
    else { "absent".into() }
}

/// Trimmed mean: sort, drop top/bottom 25%, average the middle 50%.
/// More robust than median (uses more data) and less noisy than raw mean.
pub(crate) fn trimmed_mean(buf: &VecDeque<f64>) -> f64 {
    if buf.is_empty() { return 0.0; }
    let mut sorted: Vec<f64> = buf.iter().copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let trim = n / 4; // drop 25% from each end
    let middle = &sorted[trim..n - trim.max(0)];
    if middle.is_empty() {
        sorted[n / 2] // fallback to median if too few samples
    } else {
        middle.iter().sum::<f64>() / middle.len() as f64
    }
}

/// Generate synthetic 17-point COCO skeleton pose keypoints for DensePose visualization.
/// Used when a model is loaded via --model flag (placeholder until real ONNX model available).
pub(crate) fn generate_synthetic_pose(tick: u64, amplitudes: &[f64], motion_score: f64) -> Option<Vec<[f64; 4]>> {
    let t = tick as f64 * 0.1;
    let amp_mean = if amplitudes.is_empty() { 15.0 } else { amplitudes.iter().sum::<f64>() / amplitudes.len() as f64 };
    let scale = (amp_mean / 20.0).clamp(0.5, 1.5);
    let motion = motion_score.clamp(0.0, 1.0);
    // 17 COCO keypoints: [x, y, z, confidence]
    let kps: Vec<[f64; 4]> = vec![
        [0.0, -1.6 * scale, 0.0, 0.9],           // 0  nose
        [0.15 * scale, -1.5 * scale, 0.0, 0.85],  // 1  left_eye
        [-0.15 * scale, -1.5 * scale, 0.0, 0.85], // 2  right_eye
        [0.1 * scale, -1.35 * scale, 0.0, 0.8],   // 3  left_ear
        [-0.1 * scale, -1.35 * scale, 0.0, 0.8],  // 4  right_ear
        [0.25 * scale, -0.9 * scale, 0.1 * motion, 0.8],  // 5  left_shoulder
        [-0.25 * scale, -0.9 * scale, 0.1 * motion, 0.8], // 6  right_shoulder
        [0.2 * scale, -0.3 * scale, 0.05, 0.75],  // 7  left_elbow
        [-0.2 * scale, -0.3 * scale, 0.05, 0.75], // 8  right_elbow
        [0.1 * scale, 0.3 * scale, 0.0, 0.7],     // 9  left_wrist
        [-0.1 * scale, 0.3 * scale, 0.0, 0.7],    // 10 right_wrist
        [0.05 * scale, -0.65 * scale, 0.0, 0.8],  // 11 left_hip
        [-0.05 * scale, -0.65 * scale, 0.0, 0.8], // 12 right_hip
        [0.08 * scale, 0.0, 0.05 * motion, 0.7],  // 13 left_knee
        [-0.08 * scale, 0.0, 0.05 * motion, 0.7], // 14 right_knee
        [0.03 * scale, 0.65 * scale, 0.0, 0.65],  // 15 left_ankle
        [-0.03 * scale, 0.65 * scale, 0.0, 0.65], // 16 right_ankle
    ];
    // 加入呼吸微动
    let br_amp = 0.02 * scale * (t * 0.8).sin();
    let kps_with_motion: Vec<[f64; 4]> = kps.iter().enumerate().map(|(i, k)| {
        let y_shift = if i >= 5 && i <= 10 { br_amp } else { 0.0 };
        [k[0] + (t * 0.5 + i as f64).sin() * motion * 0.05, k[1] + y_shift, k[2], k[3]]
    }).collect();
    Some(kps_with_motion)
}

// ── Simulated data generator ─────────────────────────────────────────────────

pub(crate) fn generate_simulated_frame(tick: u64) -> Esp32Frame {
    let t = tick as f64 * 0.1;
    let n_sub = 56usize;
    let mut amplitudes = Vec::with_capacity(n_sub);
    let mut phases = Vec::with_capacity(n_sub);

    for i in 0..n_sub {
        let base = 15.0 + 5.0 * (i as f64 * 0.1 + t * 0.3).sin();
        let noise = (i as f64 * 7.3 + t * 13.7).sin() * 2.0;
        amplitudes.push((base + noise).max(0.1));
        phases.push((i as f64 * 0.2 + t * 0.5).sin() * std::f64::consts::PI);
    }

    Esp32Frame {
        magic: 0xC511_0001,
        node_id: 1,
        n_antennas: 1,
        n_subcarriers: n_sub as u16,
        freq_mhz: 2437u32,
        sequence: tick as u32,
        rssi: (-40.0 + 5.0 * (t * 0.2).sin()) as i8,
        noise_floor: -90,
        amplitudes,
        phases,
    }
}

// ── Multi-person estimation (issue #97) ──────────────────────────────────────

/// Estimate person count from CSI features using a weighted composite heuristic.
///
/// Single ESP32 link limitations: variance-based detection can reliably detect
/// 1-2 persons. 3+ is speculative and requires ≥ nodes for spatial resolution.
///
/// Returns a raw score (0.0..1.0) that the caller converts to person count
/// after temporal smoothing.
pub(crate) fn compute_person_score(feat: &FeatureInfo) -> f64 {
    // Normalize each feature to [0, 1] using calibrated ranges:
    //
    //   variance: intra-frame amp variance. 1-person ~2-15, 2-person ~15-60,
    //     real ESP32 can go higher. Use 30.0 as scaling midpoint.
    let var_norm = (feat.variance / 30.0).clamp(0.0, 1.0);

    //   change_points: threshold crossings in 56 subcarriers. 1-person ~5-15,
    //     2-person ~15-30. Scale by 30.0 (half of max 55).
    let cp_norm = (feat.change_points as f64 / 30.0).clamp(0.0, 1.0);

    //   motion_band_power: upper-half subcarrier variance. 1-person ~1-8,
    //     2-person ~8-25. Scale by 20.0.
    let motion_norm = (feat.motion_band_power / 20.0).clamp(0.0, 1.0);

    //   spectral_power: mean squared amplitude. Highly variable (~100-1000+).
    //     Use relative change indicator: high spectral_power with high variance
    //     suggests multiple reflectors. Scale by 500.0.
    let sp_norm = (feat.spectral_power / 500.0).clamp(0.0, 1.0);

    // Weighted composite —variance and change_points carry the most signal.
    var_norm * 0.35 + cp_norm * 0.30 + motion_norm * 0.20 + sp_norm * 0.15
}

/// Convert smoothed person score to discrete count with hysteresis.
///
/// Uses asymmetric thresholds: higher threshold to *add* a person, lower to
/// *drop* one.  This prevents flickering when the score hovers near a boundary
/// (the #1 user-reported issue —see #237, #249, #280, #292).
pub(crate) fn score_to_person_count(smoothed_score: f64, prev_count: usize) -> usize {
    // Up-thresholds (must exceed to increase count):
    //   1→: 0.65  (raised from 0.50 —multipath in small rooms hit 0.50 easily)
    //   2→: 0.85  (raised from 0.80 —3 persons needs strong sustained signal)
    // Down-thresholds (must drop below to decrease count):
    //   2→: 0.45  (hysteresis gap of 0.20)
    //   3→: 0.70  (hysteresis gap of 0.15)
    match prev_count {
        0 | 1 => {
            if smoothed_score > 0.85 {
                3
            } else if smoothed_score > 0.65 {
                2
            } else {
                1
            }
        }
        2 => {
            if smoothed_score > 0.85 {
                3
            } else if smoothed_score < 0.45 {
                1
            } else {
                2 // hold —within hysteresis band
            }
        }
        _ => {
            // prev_count >= 3
            if smoothed_score < 0.45 {
                1
            } else if smoothed_score < 0.70 {
                2
            } else {
                3 // hold
            }
        }
    }
}

/// Generate a single person's skeleton with per-person spatial offset and phase stagger.
///
/// `person_idx`: 0-based index of this person.
/// `total_persons`: total number of detected persons (for spacing calculation).
pub(crate) fn derive_single_person_pose(
    update: &SensingUpdate,
    person_idx: usize,
    total_persons: usize,
) -> PersonDetection {
    let cls = &update.classification;
    let feat = &update.features;

    // Per-person phase offset: ~120 degrees apart so they don't move in sync.
    let phase_offset = person_idx as f64 * 2.094;

    // Spatial spread: persons distributed symmetrically around center.
    let half = (total_persons as f64 - 1.0) / 2.0;
    let person_x_offset = (person_idx as f64 - half) * 120.0; // 120px spacing

    // Confidence decays for additional persons (less certain about person 2, 3).
    let conf_decay = 1.0 - person_idx as f64 * 0.15;

    // ── Signal-derived scalars ────────────────────────────────────────────────

    let motion_score = (feat.motion_band_power / 15.0).clamp(0.0, 1.0);
    let is_walking = motion_score > 0.55;
    let breath_amp = (feat.breathing_band_power * 4.0).clamp(0.0, 12.0);

    let breath_phase = if let Some(ref vs) = update.vital_signs {
        let bpm = vs.breathing_rate_bpm.unwrap_or(15.0);
        let freq = (bpm / 60.0).clamp(0.1, 0.5);
        (update.tick as f64 * freq * 0.1 * std::f64::consts::TAU + phase_offset).sin()
    } else {
        (update.tick as f64 * 0.08 + feat.breathing_band_power + phase_offset).sin()
    };

    let lean_x = (feat.dominant_freq_hz / 5.0 - 1.0).clamp(-1.0, 1.0) * 18.0;

    let stride_x = if is_walking {
        let stride_phase = (feat.motion_band_power * 0.7 + update.tick as f64 * 0.12 + phase_offset).sin();
        stride_phase * 45.0 * motion_score
    } else {
        0.0
    };

    let burst = (feat.change_points as f64 / 8.0).clamp(0.0, 1.0);

    let noise_seed = feat.variance * 31.7 + update.tick as f64 * 17.3 + person_idx as f64 * 97.1;
    let noise_val = (noise_seed.sin() * 43758.545).fract();

    let snr_factor = ((feat.variance - 0.5) / 10.0).clamp(0.0, 1.0);
    let base_confidence = cls.confidence * (0.6 + 0.4 * snr_factor) * conf_decay;

    // ── Skeleton base position ────────────────────────────────────────────────

    let base_x = 320.0 + stride_x + lean_x * 0.5 + person_x_offset;
    let base_y = 240.0 - motion_score * 8.0;

    // ── COCO 17-keypoint offsets from hip-center ──────────────────────────────

    let kp_names = [
        "nose", "left_eye", "right_eye", "left_ear", "right_ear",
        "left_shoulder", "right_shoulder", "left_elbow", "right_elbow",
        "left_wrist", "right_wrist", "left_hip", "right_hip",
        "left_knee", "right_knee", "left_ankle", "right_ankle",
    ];

    let kp_offsets: [(f64, f64); 17] = [
        (  0.0,  -80.0), // 0  nose
        ( -8.0,  -88.0), // 1  left_eye
        (  8.0,  -88.0), // 2  right_eye
        (-16.0,  -82.0), // 3  left_ear
        ( 16.0,  -82.0), // 4  right_ear
        (-30.0,  -50.0), // 5  left_shoulder
        ( 30.0,  -50.0), // 6  right_shoulder
        (-45.0,  -15.0), // 7  left_elbow
        ( 45.0,  -15.0), // 8  right_elbow
        (-50.0,   20.0), // 9  left_wrist
        ( 50.0,   20.0), // 10 right_wrist
        (-20.0,   20.0), // 11 left_hip
        ( 20.0,   20.0), // 12 right_hip
        (-22.0,   70.0), // 13 left_knee
        ( 22.0,   70.0), // 14 right_knee
        (-24.0,  120.0), // 15 left_ankle
        ( 24.0,  120.0), // 16 right_ankle
    ];

    const TORSO_KP: [usize; 4] = [5, 6, 11, 12];
    const EXTREMITY_KP: [usize; 4] = [9, 10, 15, 16];

    let keypoints: Vec<PoseKeypoint> = kp_names.iter().zip(kp_offsets.iter())
        .enumerate()
        .map(|(i, (name, (dx, dy)))| {
            let breath_dx = if TORSO_KP.contains(&i) {
                let sign = if *dx < 0.0 { -1.0 } else { 1.0 };
                sign * breath_amp * breath_phase * 0.5
            } else {
                0.0
            };
            let breath_dy = if TORSO_KP.contains(&i) {
                let sign = if *dy < 0.0 { -1.0 } else { 1.0 };
                sign * breath_amp * breath_phase * 0.3
            } else {
                0.0
            };

            let extremity_jitter = if EXTREMITY_KP.contains(&i) {
                let phase = noise_seed + i as f64 * 2.399;
                (
                    phase.sin() * burst * motion_score * 12.0,
                    (phase * 1.31).cos() * burst * motion_score * 8.0,
                )
            } else {
                (0.0, 0.0)
            };

            let kp_noise_x = ((noise_seed + i as f64 * 1.618).sin() * 43758.545).fract()
                * feat.variance.sqrt().clamp(0.0, 3.0) * motion_score;
            let kp_noise_y = ((noise_seed + i as f64 * 2.718).cos() * 31415.926).fract()
                * feat.variance.sqrt().clamp(0.0, 3.0) * motion_score * 0.6;

            let swing_dy = if is_walking {
                let stride_phase =
                    (feat.motion_band_power * 0.7 + update.tick as f64 * 0.12 + phase_offset).sin();
                match i {
                    7 | 9  => -stride_phase * 20.0 * motion_score,
                    8 | 10 =>  stride_phase * 20.0 * motion_score,
                    13 | 15 =>  stride_phase * 25.0 * motion_score,
                    14 | 16 => -stride_phase * 25.0 * motion_score,
                    _ => 0.0,
                }
            } else {
                0.0
            };

            let final_x = base_x + dx + breath_dx + extremity_jitter.0 + kp_noise_x;
            let final_y = base_y + dy + breath_dy + extremity_jitter.1 + kp_noise_y + swing_dy;

            let kp_conf = if EXTREMITY_KP.contains(&i) {
                base_confidence * (0.7 + 0.3 * snr_factor) * (0.85 + 0.15 * noise_val)
            } else {
                base_confidence * (0.88 + 0.12 * ((i as f64 * 0.7 + noise_seed).cos()))
            };

            PoseKeypoint {
                name: name.to_string(),
                x: final_x,
                y: final_y,
                z: lean_x * 0.02,
                confidence: kp_conf.clamp(0.1, 1.0),
            }
        })
        .collect();

    let xs: Vec<f64> = keypoints.iter().map(|k| k.x).collect();
    let ys: Vec<f64> = keypoints.iter().map(|k| k.y).collect();
    let min_x = xs.iter().cloned().fold(f64::MAX, f64::min) - 10.0;
    let min_y = ys.iter().cloned().fold(f64::MAX, f64::min) - 10.0;
    let max_x = xs.iter().cloned().fold(f64::MIN, f64::max) + 10.0;
    let max_y = ys.iter().cloned().fold(f64::MIN, f64::max) + 10.0;

    PersonDetection {
        id: (person_idx + 1) as u32,
        confidence: cls.confidence * conf_decay,
        keypoints,
        bbox: BoundingBox {
            x: min_x,
            y: min_y,
            width: (max_x - min_x).max(80.0),
            height: (max_y - min_y).max(160.0),
        },
        zone: format!("zone_{}", person_idx + 1),
    }
}

pub(crate) fn derive_pose_from_sensing(update: &SensingUpdate) -> Vec<PersonDetection> {
    let cls = &update.classification;
    if !cls.presence {
        return vec![];
    }

    // Use estimated_persons if set by the tick loop; otherwise default to 1.
    let person_count = update.estimated_persons.unwrap_or(1).max(1);

    (0..person_count)
        .map(|idx| derive_single_person_pose(update, idx, person_count))
        .collect()
}
