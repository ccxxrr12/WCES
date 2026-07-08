//! Template Engine — rule-based analysis with KB result injection.
//!
//! Wraps the existing FallbackAnalyzer from fallback.rs and enhances output
//! with KB match results. Used as the template-with-KB degradation path.

use crate::fallback::{FallbackAnalyzer, FallbackContext, LlmAnalysisResult};
use crate::types::{AnalysisResult, AnalysisSource, DegradationLevel};

pub struct TemplateEngine;

impl TemplateEngine {
    pub fn new() -> Self {
        Self
    }

    /// Generate analysis using the fallback analyzer and wrap as AnalysisResult.
    pub fn generate(ctx: &FallbackContext) -> AnalysisResult {
        let result: LlmAnalysisResult = FallbackAnalyzer::analyze(ctx);
        let text = serde_json::to_string_pretty(&result).unwrap_or_default();

        AnalysisResult {
            patient_id: ctx.patient.patient_id.parse().unwrap_or(0),
            text,
            risk_adjustment: None,
            source: AnalysisSource::Template,
            degrade_level: DegradationLevel::L2TemplateWithKB,
            generated_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Basic fallback when nothing else is available (L3 degradation).
    pub fn generate_basic(patient_id: u32) -> AnalysisResult {
        AnalysisResult {
            patient_id,
            text: "模板分析不可用 — 请检查系统状态".into(),
            risk_adjustment: None,
            source: AnalysisSource::Template,
            degrade_level: DegradationLevel::L3TemplateOnly,
            generated_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}
