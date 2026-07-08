//! Risk Adjustment Extractor — parses LLM output for triage second-opinion.
//!
//! Extracts [分诊建议: 升级/维持/降级, 置信度: 0-100%] from analysis text.
//! RiskAdjustment is advisory only — it never automatically changes START triage.

use crate::types::{AdjustDirection, RiskAdjustment};
use regex::Regex;

pub struct RiskAdjustmentExtractor {
    re: Regex,
}

impl RiskAdjustmentExtractor {
    pub fn new() -> Self {
        Self {
            re: Regex::new(r"\[分诊建议:\s*(升级|维持|降级),\s*置信度:\s*(\d+)%\]").unwrap(),
        }
    }

    /// Extract RiskAdjustment from analysis text. Returns None if not found.
    pub fn extract(&self, text: &str) -> Option<RiskAdjustment> {
        let caps = self.re.captures(text)?;
        let direction = match &caps[1] {
            "升级" => AdjustDirection::Escalate,
            "维持" => AdjustDirection::Maintain,
            "降级" => AdjustDirection::Deescalate,
            _ => return None,
        };
        let confidence = caps[2].parse::<f32>().ok()? / 100.0;

        let reason_short = text
            .lines()
            .find(|l| l.contains("理由") || l.contains("原因"))
            .unwrap_or("详见分析")
            .trim()
            .to_string();

        let detail = text.lines().take(5).collect::<Vec<_>>().join("\n");

        Some(RiskAdjustment {
            direction,
            confidence,
            reason_short,
            detail,
        })
    }
}

impl Default for RiskAdjustmentExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_escalate() {
        let extractor = RiskAdjustmentExtractor::new();
        let text = "伤员心率持续上升...\n[分诊建议: 升级, 置信度: 78%]\n理由: 心动过速持续恶化";
        let adj = extractor.extract(text).expect("should extract");
        assert_eq!(adj.direction, AdjustDirection::Escalate);
        assert!((adj.confidence - 0.78).abs() < 0.01);
    }

    #[test]
    fn test_extract_maintain() {
        let extractor = RiskAdjustmentExtractor::new();
        let text = "生命体征稳定。[分诊建议: 维持, 置信度: 95%]";
        let adj = extractor.extract(text).expect("should extract");
        assert_eq!(adj.direction, AdjustDirection::Maintain);
    }

    #[test]
    fn test_no_match() {
        let extractor = RiskAdjustmentExtractor::new();
        assert!(extractor.extract("没有分诊建议").is_none());
    }
}
