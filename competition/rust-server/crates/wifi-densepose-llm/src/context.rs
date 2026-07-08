//! Context Collator — builds StructuredContext from live system state.
//!
//! Gathers data from PatientRecordDB, VitalSigns ring buffer, TriageEngine,
//! edge module alerts, and MedicalKB search into a single context snapshot.

use crate::sliding_window::TrendDirection;
use crate::types::{
    AgentVitalSnapshot, KbMatchResult, PatientHistory, StructuredContext,
    TriageStep, TrendSummary, TriggerSource,
};

// ── Error Type ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CollatorError {
    PatientNotFound(u32),
    NoVitalData(u32),
    Internal(String),
}

impl std::fmt::Display for CollatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PatientNotFound(id) => write!(f, "patient {} not found", id),
            Self::NoVitalData(id) => write!(f, "no vital data for patient {}", id),
            Self::Internal(msg) => write!(f, "collator internal error: {}", msg),
        }
    }
}

impl std::error::Error for CollatorError {}

// ── Context Collator ─────────────────────────────────────────────────────────

pub struct ContextCollator;

impl ContextCollator {
    pub fn new() -> Self {
        Self
    }

    /// Build a full StructuredContext for the given patient.
    ///
    /// Intended to be called from the agent main loop, which holds a read lock
    /// on AppState. The caller passes in pre-extracted data to avoid coupling.
    pub fn build(
        &self,
        patient_id: u32,
        node_id: u8,
        trigger: TriggerSource,
        vitals: AgentVitalSnapshot,
        triage_current: String,
        triage_history: Vec<TriageStep>,
        patient_history: Option<PatientHistory>,
        recent_alerts: Vec<String>,
        kb_matches: Vec<KbMatchResult>,
        trend_1min: TrendSummary,
        trend_5min: TrendSummary,
    ) -> Result<StructuredContext, CollatorError> {
        Ok(StructuredContext {
            patient_id,
            node_id,
            vitals_current: vitals,
            vitals_trend_1min: trend_1min,
            vitals_trend_5min: trend_5min,
            triage_current,
            triage_trajectory: triage_history,
            patient_history,
            recent_alerts,
            kb_matches,
            triggered_by: trigger,
            built_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        })
    }
}

impl Default for ContextCollator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Trend Computation ───────────────────────────────────────────────────────

/// Compute a simple trend from a sequence of (timestamp_ms, value) pairs.
/// Returns Rising/Falling/Stable/InsufficientData based on linear regression slope.
pub fn compute_trend(values: &[(u64, f32)]) -> TrendSummary {
    let n = values.len();
    if n < 4 {
        return TrendSummary {
            direction: TrendDirection::Stable,
            delta: 0.0,
            delta_pct: 0.0,
            anomaly_score: 0.0,
            data_points: n as u16,
        };
    }

    let mean_x = values.iter().map(|(t, _)| *t as f64).sum::<f64>() / n as f64;
    let mean_y = values.iter().map(|(_, v)| *v as f64).sum::<f64>() / n as f64;

    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (t, v) in values {
        let dx = *t as f64 - mean_x;
        num += dx * (*v as f64 - mean_y);
        den += dx * dx;
    }

    let slope = if den.abs() < 1e-9 { 0.0 } else { num / den };
    let delta = (slope * n as f64 * 1000.0) as f32; // project over ~1s

    let delta_pct = if mean_y.abs() > 1e-6 {
        (delta / (mean_y as f32)) * 100.0
    } else {
        0.0
    };

    let direction = if slope.abs() < 0.02 * mean_y.abs().max(1.0) {
        TrendDirection::Stable
    } else if slope > 0.0 {
        TrendDirection::Rising
    } else {
        TrendDirection::Falling
    };

    let anomaly_score = if delta_pct.abs() > 30.0 { 8.0 } else if delta_pct.abs() > 15.0 { 5.0 } else { 2.0 };

    TrendSummary {
        direction,
        delta,
        delta_pct,
        anomaly_score,
        data_points: n as u16,
    }
}
