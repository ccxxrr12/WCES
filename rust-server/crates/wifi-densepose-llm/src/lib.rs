//! Edge LLM Intelligent Analysis Engine for WCES Field Hospital Triage
//!
//! This crate provides a two-tier architecture for medical analysis:
//! - **L1 (Rule Engine)**: START triage + edge alerts (hard safety boundary, not in this crate)
//! - **L2 (LLM Enhancement)**: Context-aware analysis combining patient history,
//!   medical knowledge base, vital sign trends, and multi-modal data.
//!
//! # Architecture
//!
//! ```text
//! PatientRecordDB ──┐
//! MedicalKB ────────┤
//! SlidingWindow ────┼──→ PromptBuilder ──→ LLM/fallback ──→ AnalysisResult
//! Current Vitals ───┘
//! ```
//!
//! # Feature Flags
//!
//! - `template-only` (default): Template-based analysis, no LLM model required
//! - `llm`: Full LLM inference via Candle + Qwen2.5-0.5B

pub mod config;
pub mod engine;
pub mod fallback;
pub mod medical_knowledge;
pub mod patient_record;
pub mod prompt_builder;
pub mod sliding_window;
pub mod types;

#[cfg(feature = "llm")]
pub mod streaming;

// Always available types
pub use types::{LlmGenerationResult, StreamToken};

// Feature-gated LLM runtime
#[cfg(feature = "llm")]
pub use streaming::{LlmRuntime, StreamingGenerator};

// Core components
pub use config::LlmConfig;
pub use engine::LlmAnalysisEngine;
pub use fallback::{FallbackAnalyzer, FallbackContext};
pub use medical_knowledge::{MatchInput, MedicalCondition, MedicalKnowledgeBase};
pub use patient_record::{Gender, PatientRecord, PatientRecordDB};
pub use prompt_builder::{PromptBuilder, PromptContext};
pub use sliding_window::{MotionPattern, SlidingWindow, TrendDirection, VitalSnapshot, VitalTrendSummary};
