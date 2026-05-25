//! Unified configuration loader for WCES.
//!
//! Reads `wces.config.toml` from the project root (or a path specified via
//! `--config` CLI flag).  CLI flags take precedence over config-file values,
//! which take precedence over defaults.

use serde::Deserialize;
use tracing::warn;

// ── Top-level ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WcesConfig {
    #[serde(default)]
    pub server: ServerSection,
}

// ── Server ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct ServerSection {
    #[serde(default)]
    pub source: Option<String>,
    pub http_port: Option<u16>,
    pub ws_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub bind_addr: Option<String>,
    pub ui_path: Option<String>,
    pub log_level: Option<String>,

    #[serde(default)]
    pub agent: AgentSection,
}

// ── Agent ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AgentSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_periodic_secs")]
    pub periodic_analysis_secs: u64,
    #[serde(default = "default_analysis_timeout")]
    pub analysis_timeout_secs: u64,
    #[serde(default = "default_cooldown")]
    pub per_patient_cooldown_secs: u64,
    #[serde(default = "default_true")]
    pub fallback_to_template: bool,

    // Windows
    #[serde(default = "default_short_window")]
    pub short_window_secs: u64,
    #[serde(default = "default_medium_window")]
    pub medium_window_secs: u64,
    #[serde(default = "default_long_window")]
    pub long_window_secs: u64,

    // KB paths
    #[serde(default = "default_patient_db")]
    pub patient_db_path: String,
    #[serde(default = "default_medical_kb")]
    pub medical_kb_path: String,
    #[serde(default = "default_agent_kb")]
    pub agent_kb_path: String,

    #[serde(default)]
    pub gateway: GatewaySection,
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerSection,
    #[serde(default)]
    pub degradation: DegradationSection,
    #[serde(default)]
    pub validator: ValidatorSection,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "agent".into(),
            periodic_analysis_secs: 30,
            analysis_timeout_secs: 120,
            per_patient_cooldown_secs: 300,
            fallback_to_template: true,
            short_window_secs: 60,
            medium_window_secs: 300,
            long_window_secs: 1800,
            patient_db_path: "data/patients".into(),
            medical_kb_path: "data/medical_knowledge.json".into(),
            agent_kb_path: "data/agent_kb.json".into(),
            gateway: GatewaySection::default(),
            circuit_breaker: CircuitBreakerSection::default(),
            degradation: DegradationSection::default(),
            validator: ValidatorSection::default(),
        }
    }
}

// ── Gateway ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GatewaySection {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_gateway_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: "gpt-4o-mini".into(),
            api_key: String::new(),
            timeout_secs: 20,
            max_retries: 2,
            temperature: 0.3,
        }
    }
}

// ── Circuit Breaker ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CircuitBreakerSection {
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u8,
    #[serde(default = "default_breaker_open_secs")]
    pub open_duration_secs: u64,
}

impl Default for CircuitBreakerSection {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            open_duration_secs: 300,
        }
    }
}

// ── Degradation ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DegradationSection {
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: u64,
    #[serde(default = "default_max_cache")]
    pub max_cache_size: usize,
    #[serde(default = "default_network_failure_threshold")]
    pub network_failure_threshold: u8,
}

impl Default for DegradationSection {
    fn default() -> Self {
        Self {
            cooldown_secs: 300,
            max_cache_size: 32,
            network_failure_threshold: 5,
        }
    }
}

// ── Validator ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ValidatorSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub block_medication: bool,
    #[serde(default = "default_true")]
    pub block_injection: bool,
    #[serde(default = "default_true")]
    pub block_dosage: bool,
    #[serde(default = "default_true")]
    pub block_surgery: bool,
    #[serde(default = "default_true")]
    pub append_disclaimer: bool,
}

impl Default for ValidatorSection {
    fn default() -> Self {
        Self {
            enabled: true,
            block_medication: true,
            block_injection: true,
            block_dosage: true,
            block_surgery: true,
            append_disclaimer: true,
        }
    }
}

// ── Serde default helpers ──────────────────────────────────────────────────────

fn default_true() -> bool { true }
fn default_mode() -> String { "agent".into() }
fn default_periodic_secs() -> u64 { 30 }
fn default_analysis_timeout() -> u64 { 120 }
fn default_cooldown() -> u64 { 300 }
fn default_short_window() -> u64 { 60 }
fn default_medium_window() -> u64 { 300 }
fn default_long_window() -> u64 { 1800 }
fn default_patient_db() -> String { "data/patients".into() }
fn default_medical_kb() -> String { "data/medical_knowledge.json".into() }
fn default_agent_kb() -> String { "data/agent_kb.json".into() }
fn default_model() -> String { "gpt-4o-mini".into() }
fn default_gateway_timeout() -> u64 { 20 }
fn default_max_retries() -> u8 { 2 }
fn default_temperature() -> f32 { 0.3 }
fn default_failure_threshold() -> u8 { 3 }
fn default_breaker_open_secs() -> u64 { 300 }
fn default_max_cache() -> usize { 32 }
fn default_network_failure_threshold() -> u8 { 5 }

// ── Loader ─────────────────────────────────────────────────────────────────────

/// Load `wces.config.toml` from the given path.
///
/// If the file doesn't exist, returns `Ok(None)`.  If it exists but parsing
/// fails, returns the parse error.
pub fn load_config(path: &str) -> Result<Option<WcesConfig>, anyhow::Error> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("Config file not found: {path} — using defaults");
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };

    let config: WcesConfig = toml::from_str(&content)?;
    Ok(Some(config))
}

/// Search common locations for `wces.config.toml` and return the first that
/// exists, or `None`.
pub fn find_config() -> Option<String> {
    let candidates = [
        "wces.config.toml",
        "../wces.config.toml",
        "../../wces.config.toml",
        "../../../wces.config.toml",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}
