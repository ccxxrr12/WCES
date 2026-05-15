//! Shared types for the LLM analysis engine.
//!
//! These types are always available, regardless of feature flags.

use serde::Serialize;

// ── Stream Token Types ──────────────────────────────────────────────────────

/// A single token pushed during streaming generation (LLM or fallback).
#[derive(Debug, Clone)]
pub struct StreamToken {
    /// The patient/survivor being analyzed
    pub survivor_id: String,
    /// Token index (0-based)
    pub token_index: u32,
    /// Incremental text for this token
    pub text: String,
    /// Whether this is the final token
    pub is_complete: bool,
}

/// Result of a completed LLM generation.
#[derive(Debug, Clone, Serialize)]
pub struct LlmGenerationResult {
    pub survivor_id: String,
    pub full_text: String,
    pub generated_tokens: usize,
    pub elapsed_ms: u64,
    pub prompt_tokens: usize,
}
