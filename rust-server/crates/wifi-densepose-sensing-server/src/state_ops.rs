//! State-mutating helper functions for motion classification, adaptive override,
//! and vital sign smoothing.
//!
//! Extracted from `main.rs` to keep the entry point slim.

use crate::adaptive_classifier;
use crate::signal_processing::{raw_classify, trimmed_mean};
use crate::types::{
    BASELINE_EMA_ALPHA, BASELINE_WARMUP, BR_DEAD_BAND, BR_MAX_JUMP,
    DEBOUNCE_FRAMES, ClassificationInfo, FeatureInfo,
    HR_DEAD_BAND, HR_MAX_JUMP, MOTION_EMA_ALPHA,
    VITAL_EMA_ALPHA, VITAL_MEDIAN_WINDOW, PerNodeState,
};
use crate::vital_signs::VitalSigns;
use crate::AppStateInner;
use std::collections::VecDeque;

/// Apply EMA smoothing, adaptive baseline subtraction, and hysteresis debounce
/// to the raw classification.  Mutates the smoothing state in `AppStateInner`.
pub(crate) fn smooth_and_classify(state: &mut AppStateInner, raw: &mut ClassificationInfo, raw_motion: f64) {
    // 1. Adaptive baseline: slowly track the "quiet room" floor.
    //    Only update baseline when raw score is below the current smoothed level
    //    (i.e. during calm periods) so walking doesn't inflate the baseline.
    state.baseline_frames += 1;
    if state.baseline_frames < BASELINE_WARMUP {
        // During warm-up, aggressively learn the baseline.
        state.baseline_motion = state.baseline_motion * 0.9 + raw_motion * 0.1;
    } else if raw_motion < state.smoothed_motion + 0.05 {
        state.baseline_motion = state.baseline_motion * (1.0 - BASELINE_EMA_ALPHA)
                              + raw_motion * BASELINE_EMA_ALPHA;
    }

    // 2. Subtract baseline and clamp.
    let adjusted = (raw_motion - state.baseline_motion * 0.7).max(0.0);

    // 3. EMA smooth the adjusted score.
    state.smoothed_motion = state.smoothed_motion * (1.0 - MOTION_EMA_ALPHA)
                          + adjusted * MOTION_EMA_ALPHA;
    let sm = state.smoothed_motion;

    // 4. Classify from smoothed score.
    let candidate = raw_classify(sm);

    // 5. Hysteresis debounce: require N consecutive frames agreeing on a new state.
    if candidate == state.current_motion_level {
        // Already in this state —reset debounce.
        state.debounce_counter = 0;
        state.debounce_candidate = candidate;
    } else if candidate == state.debounce_candidate {
        state.debounce_counter += 1;
        if state.debounce_counter >= DEBOUNCE_FRAMES {
            // Transition accepted.
            state.current_motion_level = candidate;
            state.debounce_counter = 0;
        }
    } else {
        // New candidate —restart counter.
        state.debounce_candidate = candidate;
        state.debounce_counter = 1;
    }

    // 6. Write the smoothed result back into the classification.
    raw.motion_level = state.current_motion_level.clone();
    raw.presence = sm > 0.03;
    raw.confidence = (0.4 + sm * 0.6).clamp(0.0, 1.0);
}

/// If an adaptive model is loaded, override the classification with the
/// model's prediction.  Uses the full 15-feature vector for higher accuracy.
pub(crate) fn adaptive_override(state: &AppStateInner, features: &FeatureInfo, classification: &mut ClassificationInfo) {
    if let Some(ref model) = state.adaptive_model {
        // Get current frame amplitudes from the latest history entry.
        let amps = state.frame_history.back()
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let feat_arr = adaptive_classifier::features_from_runtime(
            &serde_json::json!({
                "variance": features.variance,
                "motion_band_power": features.motion_band_power,
                "breathing_band_power": features.breathing_band_power,
                "spectral_power": features.spectral_power,
                "dominant_freq_hz": features.dominant_freq_hz,
                "change_points": features.change_points,
                "mean_rssi": features.mean_rssi,
            }),
            amps,
        );
        let (label, conf) = model.classify(&feat_arr);
        classification.motion_level = label.to_string();
        classification.presence = label != "absent";
        // Blend model confidence with existing smoothed confidence.
        classification.confidence = (conf * 0.7 + classification.confidence * 0.3).clamp(0.0, 1.0);
    }
}

/// Smooth vital signs using median-filter outlier rejection + EMA.
/// Mutates `state.smoothed_hr`, `state.smoothed_br`, etc.
/// Returns the smoothed VitalSigns to broadcast.
pub(crate) fn smooth_vitals(state: &mut AppStateInner, raw: &VitalSigns) -> VitalSigns {
    let raw_hr = raw.heart_rate_bpm.unwrap_or(0.0);
    let raw_br = raw.breathing_rate_bpm.unwrap_or(0.0);

    // -- Outlier rejection: skip values that jump too far from current EMA --
    let hr_ok = state.smoothed_hr < 1.0 || (raw_hr - state.smoothed_hr).abs() < HR_MAX_JUMP;
    let br_ok = state.smoothed_br < 1.0 || (raw_br - state.smoothed_br).abs() < BR_MAX_JUMP;

    // Push into buffer (only non-outlier values)
    if hr_ok && raw_hr > 0.0 {
        state.hr_buffer.push_back(raw_hr);
        if state.hr_buffer.len() > VITAL_MEDIAN_WINDOW { state.hr_buffer.pop_front(); }
    }
    if br_ok && raw_br > 0.0 {
        state.br_buffer.push_back(raw_br);
        if state.br_buffer.len() > VITAL_MEDIAN_WINDOW { state.br_buffer.pop_front(); }
    }

    // Compute trimmed mean: drop top/bottom 25% then average the middle 50%.
    // This is more stable than pure median and less noisy than raw mean.
    let trimmed_hr = trimmed_mean(&state.hr_buffer);
    let trimmed_br = trimmed_mean(&state.br_buffer);

    // EMA smooth with dead-band: only update if the trimmed mean differs
    // from the current smoothed value by more than the dead-band.
    // This prevents the display from constantly creeping by tiny amounts.
    if trimmed_hr > 0.0 {
        if state.smoothed_hr < 1.0 {
            state.smoothed_hr = trimmed_hr;
        } else if (trimmed_hr - state.smoothed_hr).abs() > HR_DEAD_BAND {
            state.smoothed_hr = state.smoothed_hr * (1.0 - VITAL_EMA_ALPHA)
                              + trimmed_hr * VITAL_EMA_ALPHA;
        }
        // else: within dead-band, hold current value
    }
    if trimmed_br > 0.0 {
        if state.smoothed_br < 1.0 {
            state.smoothed_br = trimmed_br;
        } else if (trimmed_br - state.smoothed_br).abs() > BR_DEAD_BAND {
            state.smoothed_br = state.smoothed_br * (1.0 - VITAL_EMA_ALPHA)
                              + trimmed_br * VITAL_EMA_ALPHA;
        }
    }

    // Smooth confidence
    state.smoothed_hr_conf = state.smoothed_hr_conf * 0.92 + raw.heartbeat_confidence * 0.08;
    state.smoothed_br_conf = state.smoothed_br_conf * 0.92 + raw.breathing_confidence * 0.08;

    VitalSigns {
        breathing_rate_bpm: if state.smoothed_br > 1.0 { Some(state.smoothed_br) } else { None },
        heart_rate_bpm: if state.smoothed_hr > 1.0 { Some(state.smoothed_hr) } else { None },
        breathing_confidence: state.smoothed_br_conf,
        heartbeat_confidence: state.smoothed_hr_conf,
        signal_quality: raw.signal_quality,
    }
}

// ── Per-node variants (multi-node support) ─────────────────────────────────────

pub(crate) fn smooth_and_classify_node(state: &mut PerNodeState, raw: &mut ClassificationInfo, raw_motion: f64) {
    state.baseline_frames += 1;
    if state.baseline_frames < BASELINE_WARMUP {
        state.baseline_motion = state.baseline_motion * 0.9 + raw_motion * 0.1;
    } else if raw_motion < state.smoothed_motion + 0.05 {
        state.baseline_motion = state.baseline_motion * (1.0 - BASELINE_EMA_ALPHA)
                              + raw_motion * BASELINE_EMA_ALPHA;
    }
    // BUG 2 fix: apply 0.7 factor to baseline, matching the global smooth_and_classify
    let adjusted = (raw_motion - state.baseline_motion * 0.7).max(0.0);
    state.smoothed_motion = state.smoothed_motion * (1.0 - MOTION_EMA_ALPHA)
                          + adjusted * MOTION_EMA_ALPHA;
    let new_level = raw_classify(state.smoothed_motion);
    if new_level != state.current_motion_level {
        if new_level == state.debounce_candidate {
            state.debounce_counter += 1;
            if state.debounce_counter >= DEBOUNCE_FRAMES {
                state.current_motion_level = new_level.clone();
                state.debounce_counter = 0;
            }
        } else {
            state.debounce_candidate = new_level;
            state.debounce_counter = 1;
        }
    } else {
        state.debounce_counter = 0;
    }
    raw.motion_level = state.current_motion_level.clone();
    // BUG 3 fix: write back presence and confidence, matching global smooth_and_classify
    let sm = state.smoothed_motion;
    raw.presence = sm > 0.03;
    raw.confidence = (0.4 + sm * 0.6).clamp(0.0, 1.0);
}

pub(crate) fn smooth_vitals_node(state: &mut PerNodeState, raw: &VitalSigns) -> VitalSigns {
    // BUG 3+4 fix: use trimmed_mean (not median) and dead-band logic,
    // matching the global smooth_vitals for consistency across single/multi-node paths.

    let raw_hr = raw.heart_rate_bpm.unwrap_or(0.0);
    let raw_br = raw.breathing_rate_bpm.unwrap_or(0.0);

    // Outlier rejection: skip jumps larger than HR_MAX_JUMP / BR_MAX_JUMP
    let hr_ok = state.smoothed_hr < 1.0 || (raw_hr - state.smoothed_hr).abs() < HR_MAX_JUMP;
    let br_ok = state.smoothed_br < 1.0 || (raw_br - state.smoothed_br).abs() < BR_MAX_JUMP;

    if hr_ok && raw_hr > 0.0 {
        state.hr_buffer.push_back(raw_hr);
        if state.hr_buffer.len() > VITAL_MEDIAN_WINDOW { state.hr_buffer.pop_front(); }
    }
    if br_ok && raw_br > 0.0 {
        state.br_buffer.push_back(raw_br);
        if state.br_buffer.len() > VITAL_MEDIAN_WINDOW { state.br_buffer.pop_front(); }
    }

    // Use trimmed_mean for robustness (matching global path)
    let trimmed_hr = trimmed_mean(&state.hr_buffer);
    let trimmed_br = trimmed_mean(&state.br_buffer);

    // EMA with dead-band: only update when difference exceeds threshold
    if trimmed_hr > 0.0 {
        if state.smoothed_hr < 1.0 {
            state.smoothed_hr = trimmed_hr;
        } else if (trimmed_hr - state.smoothed_hr).abs() > HR_DEAD_BAND {
            state.smoothed_hr = state.smoothed_hr * (1.0 - VITAL_EMA_ALPHA) + trimmed_hr * VITAL_EMA_ALPHA;
        }
    }
    if trimmed_br > 0.0 {
        if state.smoothed_br < 1.0 {
            state.smoothed_br = trimmed_br;
        } else if (trimmed_br - state.smoothed_br).abs() > BR_DEAD_BAND {
            state.smoothed_br = state.smoothed_br * (1.0 - VITAL_EMA_ALPHA) + trimmed_br * VITAL_EMA_ALPHA;
        }
    }

    state.smoothed_hr_conf = state.smoothed_hr_conf * 0.92 + raw.heartbeat_confidence * 0.08;
    state.smoothed_br_conf = state.smoothed_br_conf * 0.92 + raw.breathing_confidence * 0.08;
    VitalSigns {
        breathing_rate_bpm: if state.smoothed_br > 1.0 { Some(state.smoothed_br) } else { None },
        heart_rate_bpm: if state.smoothed_hr > 1.0 { Some(state.smoothed_hr) } else { None },
        breathing_confidence: state.smoothed_br_conf,
        heartbeat_confidence: state.smoothed_hr_conf,
        signal_quality: raw.signal_quality,
    }
}
// median_of removed — replaced by trimmed_mean for consistency with global path
