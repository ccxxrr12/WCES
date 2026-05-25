//! Shared types for the medical agent engine.

use serde::{Deserialize, Serialize};

// ── Stream Token (kept for engine.rs fallback streaming compatibility) ──────

/// A single token pushed during streaming generation (LLM or fallback).
#[derive(Debug, Clone)]
pub struct StreamToken {
    pub survivor_id: String,
    pub token_index: u32,
    pub text: String,
    pub is_complete: bool,
}

// ── Trigger ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TriggerSource {
    Deterioration { patient_id: u32, from: String, to: String },
    NewPatient { patient_id: u32 },
    ManualRequest { patient_id: u32 },
    PeriodicScan,
}

// ── Trends ───────────────────────────────────────────────────────────────────

/// Trend summary for vital signs over a time window.
/// Uses sliding_window::TrendDirection for direction.
#[derive(Debug, Clone)]
pub struct TrendSummary {
    pub direction: crate::sliding_window::TrendDirection,
    pub delta: f32,
    pub delta_pct: f32,
    pub anomaly_score: f32,
    pub data_points: u16,
}

// ── Structured Context ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StructuredContext {
    pub patient_id: u32,
    pub node_id: u8,
    pub vitals_current: AgentVitalSnapshot,
    pub vitals_trend_1min: TrendSummary,
    pub vitals_trend_5min: TrendSummary,
    pub triage_current: String,
    pub triage_trajectory: Vec<TriageStep>,
    pub patient_history: Option<PatientHistory>,
    pub recent_alerts: Vec<String>,
    pub kb_matches: Vec<KbMatchResult>,
    pub triggered_by: TriggerSource,
    pub built_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AgentVitalSnapshot {
    pub breathing_rate_bpm: Option<f32>,
    pub heart_rate_bpm: Option<f32>,
    pub breathing_confidence: f32,
    pub heartbeat_confidence: f32,
    pub signal_quality: f32,
    pub motion_class: Option<String>,
    pub person_count_estimate: Option<u8>,
    pub rssi: Option<i16>,
}

#[derive(Debug, Clone)]
pub struct TriageStep {
    pub level: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PatientHistory {
    pub record_id: String,
    pub age_estimate: Option<String>,
    pub prior_conditions: Vec<String>,
    pub total_tracking_duration_secs: u64,
    pub triage_level_changes: u16,
    pub prior_llm_analyses: Vec<String>,
}

// ── KB Match ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KbMatchResult {
    pub entry_id: String,
    pub condition: String,
    pub match_score: f32,
    pub matched_conditions: Vec<String>,
    pub risk_factors: Vec<String>,
    pub triage_implication: String,
    pub monitoring_notes: String,
}

// ── Routing ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AnalysisRoute {
    DeepLLM,
    BriefLLM,
    TemplateWithKB,
    TemplateOnly,
    CachedReplay,
    Skip,
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub route: AnalysisRoute,
    pub reason: String,
    pub max_output_tokens: u16,
    pub priority: u8,
}

// ── Prompt ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: String,
    pub context: String,
    pub task: String,
    pub estimated_input_tokens: u16,
}

// ── Analysis Result ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisResult {
    pub patient_id: u32,
    pub text: String,
    pub risk_adjustment: Option<RiskAdjustment>,
    pub source: AnalysisSource,
    pub degrade_level: DegradationLevel,
    pub generated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskAdjustment {
    pub direction: AdjustDirection,
    pub confidence: f32,
    pub reason_short: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AdjustDirection {
    Escalate,
    Maintain,
    Deescalate,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisSource {
    LLM,
    Template,
    Cache,
}

// ── Degradation ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, PartialOrd)]
#[serde(rename_all = "UPPERCASE")]
pub enum DegradationLevel {
    L0FullLLM,
    L1BriefLLM,
    L2TemplateWithKB,
    L3TemplateOnly,
    L4CachedReplay,
}

// ── Gateway (shared across gateway.rs and degrade.rs) ────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub timeout_secs: u64,
    pub max_retries: u8,
    pub temperature: f32,
    pub failure_threshold: u8,
    pub breaker_open_secs: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("LLM_ENDPOINT")
                .unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".into()),
            model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            api_key: std::env::var("LLM_API_KEY").unwrap_or_default(),
            timeout_secs: 20,
            max_retries: 2,
            temperature: 0.3,
            failure_threshold: 3,
            breaker_open_secs: 300,
        }
    }
}
