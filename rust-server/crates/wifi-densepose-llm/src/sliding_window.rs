//! Sliding Window Trend Analyzer
//!
//! Maintains time-series windows for each patient's vital signs.
//! Compresses raw data into statistical summaries to minimize LLM prompt tokens.
//!
//! Three window sizes:
//! - Short (1 min): Immediate alert confirmation
//! - Medium (5 min): Standard analysis window
//! - Long (30 min): Baseline comparison

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

// ── Trend Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrendDirection {
    Rising,
    Stable,
    Falling,
}

impl TrendDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrendDirection::Rising => "Rising",
            TrendDirection::Stable => "Stable",
            TrendDirection::Falling => "Falling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionPattern {
    /// Consistently low motion
    ContinuousStill,
    /// Periodic bursts of motion
    IntermittentMotion,
    /// Consistently high motion
    ContinuousMotion,
    /// Sudden spike then sharp drop (possible seizure / convulsion)
    SpikeAndDrop,
    /// Gradually decreasing motion (possible decreasing consciousness)
    GradualDecline,
}

impl MotionPattern {
    pub fn as_str(&self) -> &'static str {
        match self {
            MotionPattern::ContinuousStill => "ContinuousStill",
            MotionPattern::IntermittentMotion => "IntermittentMotion",
            MotionPattern::ContinuousMotion => "ContinuousMotion",
            MotionPattern::SpikeAndDrop => "SpikeAndDrop",
            MotionPattern::GradualDecline => "GradualDecline",
        }
    }
}

// ── Vital Sign Snapshot ─────────────────────────────────────────────────────

/// A single data point for the sliding window.
#[derive(Debug, Clone)]
pub struct VitalSnapshot {
    pub timestamp: Instant,
    pub breathing_rate: f64,
    pub heart_rate: f64,
    pub motion_score: f64,
    pub signal_quality: f64,
}

// ── Trend Summary ───────────────────────────────────────────────────────────

/// Statistical summary of a sliding window.
#[derive(Debug, Clone, Serialize)]
pub struct VitalTrendSummary {
    /// Observation window duration (seconds)
    pub window_seconds: f64,
    /// Number of samples in the window
    pub sample_count: usize,

    // Breathing rate
    pub rr_mean: f64,
    pub rr_min: f64,
    pub rr_max: f64,
    pub rr_trend: TrendDirection,
    pub rr_change_pct: f64,
    pub rr_volatility: f64,

    // Heart rate
    pub hr_mean: f64,
    pub hr_min: f64,
    pub hr_max: f64,
    pub hr_trend: TrendDirection,
    pub hr_change_pct: f64,
    pub hr_volatility: f64,

    // Motion
    pub motion_mean: f64,
    pub motion_min: f64,
    pub motion_max: f64,
    pub motion_pattern: MotionPattern,

    // Signal quality
    pub signal_quality_mean: f64,
    pub signal_quality_trend: TrendDirection,
}

impl Default for VitalTrendSummary {
    fn default() -> Self {
        Self {
            window_seconds: 0.0,
            sample_count: 0,
            rr_mean: 0.0,
            rr_min: 0.0,
            rr_max: 0.0,
            rr_trend: TrendDirection::Stable,
            rr_change_pct: 0.0,
            rr_volatility: 0.0,
            hr_mean: 0.0,
            hr_min: 0.0,
            hr_max: 0.0,
            hr_trend: TrendDirection::Stable,
            hr_change_pct: 0.0,
            hr_volatility: 0.0,
            motion_mean: 0.0,
            motion_min: 0.0,
            motion_max: 0.0,
            motion_pattern: MotionPattern::ContinuousStill,
            signal_quality_mean: 0.0,
            signal_quality_trend: TrendDirection::Stable,
        }
    }
}

// ── Single Window ───────────────────────────────────────────────────────────

/// A single sliding window for one metric dimension.
struct TimeWindow {
    samples: VecDeque<VitalSnapshot>,
    window_duration: Duration,
}

impl TimeWindow {
    fn new(window_duration: Duration) -> Self {
        Self {
            samples: VecDeque::new(),
            window_duration,
        }
    }

    /// Push a new sample and prune old ones outside the window.
    fn push(&mut self, snapshot: VitalSnapshot) {
        self.samples.push_back(snapshot);
        self.prune();
    }

    /// Remove samples older than `window_duration`.
    fn prune(&mut self) {
        let cutoff = Instant::now() - self.window_duration;
        while let Some(front) = self.samples.front() {
            if front.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    fn len(&self) -> usize {
        self.samples.len()
    }

    fn summarize(&self) -> VitalTrendSummary {
        let n = self.samples.len();
        if n < 2 {
            // Not enough data for meaningful trend
            let rr = self.samples.back().map(|s| s.breathing_rate).unwrap_or(0.0);
            let hr = self.samples.back().map(|s| s.heart_rate).unwrap_or(0.0);
            let motion = self
                .samples
                .back()
                .map(|s| s.motion_score)
                .unwrap_or(0.0);
            let sq = self
                .samples
                .back()
                .map(|s| s.signal_quality)
                .unwrap_or(0.0);

            return VitalTrendSummary {
                window_seconds: self.window_duration.as_secs_f64(),
                sample_count: n,
                rr_mean: rr,
                rr_min: rr,
                rr_max: rr,
                rr_trend: TrendDirection::Stable,
                rr_change_pct: 0.0,
                rr_volatility: 0.0,
                hr_mean: hr,
                hr_min: hr,
                hr_max: hr,
                hr_trend: TrendDirection::Stable,
                hr_change_pct: 0.0,
                hr_volatility: 0.0,
                motion_mean: motion,
                motion_min: motion,
                motion_max: motion,
                motion_pattern: motion_pattern_from_single(motion),
                signal_quality_mean: sq,
                signal_quality_trend: TrendDirection::Stable,
            };
        }

        // Extract values
        let rr_values: Vec<f64> = self.samples.iter().map(|s| s.breathing_rate).collect();
        let hr_values: Vec<f64> = self.samples.iter().map(|s| s.heart_rate).collect();
        let motion_values: Vec<f64> = self.samples.iter().map(|s| s.motion_score).collect();
        let sq_values: Vec<f64> = self.samples.iter().map(|s| s.signal_quality).collect();

        VitalTrendSummary {
            window_seconds: self.window_duration.as_secs_f64(),
            sample_count: n,

            rr_mean: mean(&rr_values),
            rr_min: rr_values.iter().cloned().fold(f64::INFINITY, f64::min),
            rr_max: rr_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            rr_trend: compute_trend(&rr_values),
            rr_change_pct: percent_change(&rr_values),
            rr_volatility: coefficient_of_variation(&rr_values),

            hr_mean: mean(&hr_values),
            hr_min: hr_values.iter().cloned().fold(f64::INFINITY, f64::min),
            hr_max: hr_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            hr_trend: compute_trend(&hr_values),
            hr_change_pct: percent_change(&hr_values),
            hr_volatility: coefficient_of_variation(&hr_values),

            motion_mean: mean(&motion_values),
            motion_min: motion_values.iter().cloned().fold(f64::INFINITY, f64::min),
            motion_max: motion_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            motion_pattern: compute_motion_pattern(&motion_values),

            signal_quality_mean: mean(&sq_values),
            signal_quality_trend: compute_trend(&sq_values),
        }
    }
}

// ── Multi-Window Manager ────────────────────────────────────────────────────

/// Manages three sliding windows per patient: short, medium, long.
pub struct SlidingWindow {
    short: TimeWindow,
    medium: TimeWindow,
    long: TimeWindow,
}

impl SlidingWindow {
    /// Create a new sliding window tracker.
    pub fn new(short_secs: u64, medium_secs: u64, long_secs: u64) -> Self {
        Self {
            short: TimeWindow::new(Duration::from_secs(short_secs)),
            medium: TimeWindow::new(Duration::from_secs(medium_secs)),
            long: TimeWindow::new(Duration::from_secs(long_secs)),
        }
    }

    /// Push a new vital snapshot into all three windows.
    pub fn push(&mut self, snapshot: VitalSnapshot) {
        self.short.push(snapshot.clone());
        self.medium.push(snapshot.clone());
        self.long.push(snapshot);
    }

    /// Get the short-term trend summary (1 min).
    pub fn short_summary(&self) -> VitalTrendSummary {
        self.short.summarize()
    }

    /// Get the medium-term trend summary (5 min) — primary analysis window.
    pub fn medium_summary(&self) -> VitalTrendSummary {
        self.medium.summarize()
    }

    /// Get the long-term trend summary (30 min) — baseline comparison.
    pub fn long_summary(&self) -> VitalTrendSummary {
        self.long.summarize()
    }

    /// Check if we have enough data for a meaningful analysis.
    pub fn has_enough_data(&self) -> bool {
        self.short.len() >= 3
    }
}

// ── Multi-Patient Window Manager ────────────────────────────────────────────

/// Manages sliding windows for all patients.
pub struct WindowManager {
    windows: HashMap<String, SlidingWindow>,
    short_secs: u64,
    medium_secs: u64,
    long_secs: u64,
}

impl WindowManager {
    pub fn new(short_secs: u64, medium_secs: u64, long_secs: u64) -> Self {
        Self {
            windows: HashMap::new(),
            short_secs,
            medium_secs,
            long_secs,
        }
    }

    /// Push vitals for a patient, creating a window if needed.
    pub fn push(&mut self, patient_id: &str, snapshot: VitalSnapshot) {
        let window = self
            .windows
            .entry(patient_id.to_string())
            .or_insert_with(|| {
                SlidingWindow::new(self.short_secs, self.medium_secs, self.long_secs)
            });
        window.push(snapshot);
    }

    /// Get the sliding window for a patient.
    pub fn get(&self, patient_id: &str) -> Option<&SlidingWindow> {
        self.windows.get(patient_id)
    }

    /// Prune all windows (removes old data).
    pub fn prune_all(&mut self) {
        // Pruning happens automatically on push().
        // This is just for explicit cleanup of empty windows.
        self.windows.retain(|_, w| w.short.len() > 0);
    }

    /// Number of tracked patients.
    pub fn patient_count(&self) -> usize {
        self.windows.len()
    }
}

// ── Statistical Helpers ─────────────────────────────────────────────────────

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn compute_trend(values: &[f64]) -> TrendDirection {
    if values.len() < 3 {
        return TrendDirection::Stable;
    }

    let n = values.len();
    // Simple linear regression slope
    let x_mean = (n - 1) as f64 / 2.0;
    let y_mean = mean(values);

    let mut num = 0.0;
    let mut den = 0.0;

    for (i, &y) in values.iter().enumerate() {
        let dx = i as f64 - x_mean;
        num += dx * (y - y_mean);
        den += dx * dx;
    }

    if den == 0.0 {
        return TrendDirection::Stable;
    }

    let slope = num / den;

    // Normalize slope by mean to get relative direction
    let rel = if y_mean != 0.0 {
        slope / y_mean.abs()
    } else {
        slope
    };

    if rel > 0.02 {
        TrendDirection::Rising
    } else if rel < -0.02 {
        TrendDirection::Falling
    } else {
        TrendDirection::Stable
    }
}

fn percent_change(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }

    let first_half = &values[..values.len() / 2];
    let second_half = &values[values.len() / 2..];

    let first_mean = mean(first_half);
    let second_mean = mean(second_half);

    if first_mean == 0.0 {
        return 0.0;
    }

    ((second_mean - first_mean) / first_mean * 100.0 * 100.0).round() / 100.0
}

fn coefficient_of_variation(values: &[f64]) -> f64 {
    let m = mean(values);
    if m == 0.0 || values.len() < 2 {
        return 0.0;
    }

    let variance = values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64;
    let std_dev = variance.sqrt();
    (std_dev / m * 100.0).round() / 100.0
}

fn compute_motion_pattern(values: &[f64]) -> MotionPattern {
    if values.len() < 3 {
        return motion_pattern_from_single(
            values.iter().cloned().fold(0.0, f64::max),
        );
    }

    let m = mean(values);
    let max_val = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Thresholds
    const HIGH_MOTION: f64 = 0.6;
    const LOW_MOTION: f64 = 0.2;
    const SPIKE_RATIO: f64 = 3.0; // max/mean ratio to detect spike

    if max_val > HIGH_MOTION && max_val / m.max(0.001) > SPIKE_RATIO {
        // High spike relative to mean → possible SpikeAndDrop
        // Check if values drop after spike
        let peak_idx = values.iter().position(|&v| v == max_val).unwrap_or(0);
        let post_peak: Vec<f64> = values[peak_idx..].to_vec();
        if post_peak.len() > 1 {
            let post_mean = mean(&post_peak);
            if post_mean < max_val * 0.5 {
                return MotionPattern::SpikeAndDrop;
            }
        }
        return MotionPattern::IntermittentMotion;
    }

    if m > HIGH_MOTION {
        MotionPattern::ContinuousMotion
    } else if m < LOW_MOTION {
        // Check for gradual decline
        let trend = compute_trend(values);
        if trend == TrendDirection::Falling {
            MotionPattern::GradualDecline
        } else {
            MotionPattern::ContinuousStill
        }
    } else {
        MotionPattern::IntermittentMotion
    }
}

fn motion_pattern_from_single(value: f64) -> MotionPattern {
    if value > 0.6 {
        MotionPattern::ContinuousMotion
    } else if value < 0.2 {
        MotionPattern::ContinuousStill
    } else {
        MotionPattern::IntermittentMotion
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_window_basic() {
        let mut sw = SlidingWindow::new(60, 300, 1800);
        assert!(!sw.has_enough_data());

        // Push 5 samples
        for i in 0..5 {
            sw.push(VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate: 16.0 + i as f64 * 2.0, // Rising: 16 → 24
                heart_rate: 72.0 + i as f64 * 5.0,     // Rising: 72 → 92
                motion_score: 0.3,
                signal_quality: 0.9,
            });
        }

        assert!(sw.has_enough_data());

        let summary = sw.medium_summary();
        assert_eq!(summary.sample_count, 5);
        assert_eq!(summary.rr_trend, TrendDirection::Rising);
        assert!(summary.rr_change_pct > 0.0);
        assert_eq!(summary.hr_trend, TrendDirection::Rising);
    }

    #[test]
    fn test_stable_vitals() {
        let mut sw = SlidingWindow::new(60, 300, 1800);

        for _ in 0..10 {
            sw.push(VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate: 16.0,
                heart_rate: 72.0,
                motion_score: 0.1,
                signal_quality: 0.95,
            });
        }

        let summary = sw.medium_summary();
        assert_eq!(summary.rr_trend, TrendDirection::Stable);
        assert_eq!(summary.motion_pattern, MotionPattern::ContinuousStill);
    }

    #[test]
    fn test_window_manager() {
        let mut wm = WindowManager::new(60, 300, 1800);

        wm.push(
            "PAT-1",
            VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate: 32.0,
                heart_rate: 115.0,
                motion_score: 0.3,
                signal_quality: 0.8,
            },
        );

        assert_eq!(wm.patient_count(), 1);
        assert!(wm.get("PAT-1").is_some());
        assert!(wm.get("PAT-99").is_none());
    }
}
