//! Edge Medical Agent for WCES Field Hospital Triage
//!
//! Coordinator pattern: RZ/G2L does local signal processing + rule-based triage,
//! then offloads deep analysis to cloud LLM API. Graceful degradation when offline.
//!
//! # Architecture
//!
//! ```text
//! StructuredContext ──→ Router ──→ PromptCompiler ──→ LlmGateway (cloud)
//!       │                    │                              │
//!       │                    ├── TemplateWithKB (local)     ├── Validator
//!       │                    └── TemplateOnly (local)       └── RiskAdjust
//! ```
//!
//! # Feature Flags
//!
//! - `agent` (default): Cloud LLM API gateway + streaming + validation
//! - `template-only`: Rule-based analysis only, zero external dependencies

pub mod config;
pub mod engine;
pub mod fallback;
pub mod medical_knowledge;
pub mod patient_record;
pub mod prompt_builder;
pub mod sliding_window;
pub mod types;

// ── Agent modules (replaces the old Candle-based streaming) ───────────────────

#[cfg(feature = "agent")]
pub mod gateway;
#[cfg(feature = "agent")]
pub mod prompt;
#[cfg(feature = "agent")]
pub mod validator;
#[cfg(feature = "agent")]
pub mod risk_adjust;

// ── Always-available modules ─────────────────────────────────────────────────

pub mod agent;
pub mod context;
pub mod degrade;
pub mod medical_kb;
pub mod router;
pub mod template;

// ── Re-exports ───────────────────────────────────────────────────────────────

// Core types
pub use types::{
    AdjustDirection, AnalysisResult, AnalysisRoute, AnalysisSource, DegradationLevel,
    GatewayConfig, KbMatchResult, PatientHistory, Prompt, RiskAdjustment, RouteDecision,
    StreamToken, StructuredContext, TriageStep, TrendSummary,
    TriggerSource, AgentVitalSnapshot,
};

// Core components
pub use config::LlmConfig;
pub use engine::LlmAnalysisEngine;
pub use fallback::{FallbackAnalyzer, FallbackContext};
pub use medical_knowledge::{MatchInput, MedicalCondition, MedicalKnowledgeBase};
pub use patient_record::{Gender, PatientRecord, PatientRecordDB};
pub use prompt_builder::{PromptBuilder, PromptContext};
pub use sliding_window::{
    MotionPattern, SlidingWindow, TrendDirection, VitalSnapshot,
    VitalTrendSummary,
};

// Agent components
pub use agent::MedicalAgent;
pub use context::ContextCollator;
pub use degrade::{DegradationConfig, DegradationManager};
pub use medical_kb::MedicalKb;
pub use router::AnalysisRouter;
pub use template::TemplateEngine;

#[cfg(feature = "agent")]
pub use gateway::{GatewayError, LlmGateway};
#[cfg(feature = "agent")]
pub use prompt::PromptCompiler;
#[cfg(feature = "agent")]
pub use validator::OutputValidator;
#[cfg(feature = "agent")]
pub use risk_adjust::RiskAdjustmentExtractor;
