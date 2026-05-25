//! Medical Agent Configuration
//!
//! Controls all aspects of the medical agent behavior.

/// Configuration for the medical agent engine.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    // ── Storage Settings ──
    /// Path to patient record database (sled)
    pub patient_db_path: String,
    /// Path to medical knowledge base JSON
    pub medical_kb_path: String,

    // ── Window Settings ──
    /// Short-term window duration (seconds)
    pub short_window_secs: u64,
    /// Medium-term window duration (seconds)
    pub medium_window_secs: u64,
    /// Long-term window duration (seconds)
    pub long_window_secs: u64,

    // ── Trigger Settings ──
    /// Periodic analysis interval (seconds), 0 = disabled
    pub periodic_interval_secs: u64,
    /// Max analysis time before timeout (seconds)
    pub analysis_timeout_secs: u64,
    /// Cooldown between analyses for the same patient (seconds)
    pub per_patient_cooldown_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            patient_db_path: "data/patients".into(),
            medical_kb_path: "data/medical_knowledge.json".into(),

            short_window_secs: 60,
            medium_window_secs: 300,
            long_window_secs: 1800,

            periodic_interval_secs: 30,
            analysis_timeout_secs: 120,
            per_patient_cooldown_secs: 30,
        }
    }
}

impl LlmConfig {
    /// Competition-optimized configuration.
    pub fn competition() -> Self {
        Self {
            periodic_interval_secs: 30,
            analysis_timeout_secs: 120,
            per_patient_cooldown_secs: 30,
            ..Self::default()
        }
    }

    /// Minimal config for template-only mode (no external deps).
    pub fn template_only() -> Self {
        Self::competition()
    }
}
