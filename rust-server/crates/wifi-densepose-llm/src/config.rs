//! LLM Analysis Configuration
//!
//! Controls all aspects of the LLM analysis engine behavior.

/// Configuration for the LLM analysis engine.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    // ── Model Settings ──
    /// Path to GGUF model file (required for `llm` feature)
    pub model_path: Option<String>,
    /// Path to tokenizer JSON (required for `llm` feature)
    pub tokenizer_path: Option<String>,
    /// Maximum new tokens to generate per analysis
    pub max_new_tokens: usize,
    /// Sampling temperature (0.1–1.0, lower = more deterministic)
    pub temperature: f64,

    // ── Storage Settings ──
    /// Path to patient record database (sled)
    pub patient_db_path: String,
    /// Path to medical knowledge base JSON
    pub medical_kb_path: String,

    // ── Window Settings ──
    /// Short-term window duration (seconds) — for immediate alert confirmation
    pub short_window_secs: u64,
    /// Medium-term window duration (seconds) — standard analysis window
    pub medium_window_secs: u64,
    /// Long-term window duration (seconds) — baseline comparison
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
            model_path: Some("data/models/qwen2.5-0.5b-q4.gguf".into()),
            tokenizer_path: Some("data/models/qwen2_tokenizer.json".into()),
            max_new_tokens: 256,
            temperature: 0.3,

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
    /// Create a competition-optimized configuration.
    pub fn competition() -> Self {
        Self {
            model_path: Some("data/models/qwen2.5-0.5b-q4.gguf".into()),
            tokenizer_path: Some("data/models/qwen2_tokenizer.json".into()),
            max_new_tokens: 200,
            temperature: 0.3,

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

    /// Create a minimal config for template-only mode (no LLM model needed).
    pub fn template_only() -> Self {
        Self {
            model_path: None,
            tokenizer_path: None,
            ..Self::competition()
        }
    }
}
