//! Medical Knowledge Base
//!
//! Structured medical knowledge for RAG (Retrieval-Augmented Generation).
//! The 0.5B model lacks internal medical knowledge, so relevant disease patterns
//! are injected via prompt context through a matching/scoring system.
//!
//! Scoring weights:
//! - Vital sign match: 40%
//! - Trend match: 20%
//! - Risk factor match (from patient history): 25%
//! - Edge alert match: 15%
//! - Combined score >= 0.3 → condition is included in prompt

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

// ── Knowledge Base Types ────────────────────────────────────────────────────

/// A single medical condition entry in the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MedicalCondition {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Key vital sign indicators
    pub key_indicators: KeyIndicators,
    /// Differential diagnoses
    pub differential: Vec<String>,
    /// Risk factors that increase probability
    pub risk_factors: Vec<String>,
    /// Urgency level
    pub urgency: String, // "critical", "high", "moderate", "low"
    /// Recommended START triage level
    pub triage_recommendation: String,
    /// Recommended actions
    pub actions: Vec<String>,
    /// What to monitor closely
    pub monitoring_focus: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyIndicators {
    pub breathing_rate: Option<IndicatorRule>,
    pub heart_rate: Option<IndicatorRule>,
    pub motion_pattern: Option<String>,
    pub signal_quality: Option<String>,
    pub edge_alerts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorRule {
    /// Human-readable condition description
    pub condition: String,
    /// Expected trend direction
    pub trend: String,
}

/// Input data for condition matching.
#[derive(Debug, Clone)]
pub struct MatchInput<'a> {
    pub breathing_rate: Option<f64>,
    pub heart_rate: Option<f64>,
    pub motion_score: f64,
    pub breathing_trend: &'a str, // "Rising" / "Stable" / "Falling"
    pub heart_trend: &'a str,
    pub motion_pattern: &'a str,
    pub pre_existing: &'a [String],
    pub age: Option<u8>,
    pub active_edge_alerts: &'a [String],
}

/// A matched condition with its relevance score (0.0–1.0).
pub type MatchResult = (MedicalCondition, f64);

// ── Knowledge Base ──────────────────────────────────────────────────────────

/// Medical knowledge base with condition matching capabilities.
pub struct MedicalKnowledgeBase {
    conditions: Vec<MedicalCondition>,
}

impl MedicalKnowledgeBase {
    /// Load the knowledge base from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content =
            std::fs::read_to_string(&path).context("Failed to read medical knowledge base")?;
        let conditions: Vec<MedicalCondition> =
            serde_json::from_str(&content).context("Failed to parse medical knowledge base")?;
        Ok(Self { conditions })
    }

    /// Load from a JSON string (useful for embedded/default KB).
    pub fn from_json(json: &str) -> Result<Self> {
        let conditions: Vec<MedicalCondition> =
            serde_json::from_str(json).context("Failed to parse medical knowledge base JSON")?;
        Ok(Self { conditions })
    }

    /// Get the number of conditions in the knowledge base.
    pub fn condition_count(&self) -> usize {
        self.conditions.len()
    }

    /// Match current patient state against all conditions, returning top-N matches.
    ///
    /// Scoring:
    /// - Vital sign match: ±checks if RR/HR are in the condition's indicator range
    /// - Trend match: checks if trends align with condition expectations
    /// - Risk factor match: patient history vs condition's known risk factors
    /// - Edge alert match: active alerts vs condition's expected alerts
    pub fn match_conditions(
        &self,
        input: &MatchInput<'_>,
        top_n: usize,
    ) -> Vec<(MedicalCondition, f64)> {
        let mut scored: Vec<(MedicalCondition, f64)> = self
            .conditions
            .iter()
            .map(|cond| {
                let score =
                    self.score_vital_match(cond, input) * 0.40
                        + self.score_trend_match(cond, input) * 0.20
                        + self.score_risk_factor_match(cond, input) * 0.25
                        + self.score_edge_alert_match(cond, input) * 0.15;
                (cond.clone(), (score * 100.0).round() / 100.0)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_n);
        scored
    }

    /// Score vital sign match (40% weight).
    fn score_vital_match(&self, cond: &MedicalCondition, input: &MatchInput<'_>) -> f64 {
        let mut score = 0.0;
        let mut count = 0;

        // Breathing rate
        if let Some(ref rule) = cond.key_indicators.breathing_rate {
            count += 1;
            if let Some(rr) = input.breathing_rate {
                if self.check_vital_against_rule(rr, &rule.condition) {
                    score += 1.0;
                } else if self.check_vital_near_rule(rr, &rule.condition) {
                    score += 0.5;
                }
            }
        }

        // Heart rate
        if let Some(ref rule) = cond.key_indicators.heart_rate {
            count += 1;
            if let Some(hr) = input.heart_rate {
                if self.check_vital_against_rule(hr, &rule.condition) {
                    score += 1.0;
                } else if self.check_vital_near_rule(hr, &rule.condition) {
                    score += 0.5;
                }
            }
        }

        if count == 0 {
            0.5 // No vital indicators → neutral score
        } else {
            score / count as f64
        }
    }

    /// Score trend match (20% weight).
    fn score_trend_match(&self, cond: &MedicalCondition, input: &MatchInput<'_>) -> f64 {
        let mut score = 0.0;
        let mut count = 0;

        if let Some(ref rule) = cond.key_indicators.breathing_rate {
            count += 1;
            if trend_matches(input.breathing_trend, &rule.trend) {
                score += 1.0;
            } else if input.breathing_trend == "Stable" {
                score += 0.3;
            }
        }

        if let Some(ref rule) = cond.key_indicators.heart_rate {
            count += 1;
            if trend_matches(input.heart_trend, &rule.trend) {
                score += 1.0;
            } else if input.heart_trend == "Stable" {
                score += 0.3;
            }
        }

        // Motion pattern
        if let Some(ref expected) = cond.key_indicators.motion_pattern {
            count += 1;
            if input.motion_pattern.contains(expected.as_str()) {
                score += 1.0;
            } else {
                score += 0.2; // partial
            }
        }

        if count == 0 {
            0.5
        } else {
            score / count as f64
        }
    }

    /// Score risk factor match (25% weight) — patient history vs condition risks.
    fn score_risk_factor_match(&self, cond: &MedicalCondition, input: &MatchInput<'_>) -> f64 {
        if cond.risk_factors.is_empty() {
            return 0.5;
        }

        if input.pre_existing.is_empty() && input.age.is_none() {
            return 0.5;
        }

        let pre_set: HashSet<&str> = input.pre_existing.iter().map(|s| s.as_str()).collect();
        let mut hits = 0;

        for factor in &cond.risk_factors {
            let factor_lower = factor.to_lowercase();
            // Check against pre-existing conditions
            for cond_name in &pre_set {
                if cond_name.to_lowercase().contains(&factor_lower)
                    || factor_lower.contains(&cond_name.to_lowercase())
                {
                    hits += 1;
                    break;
                }
            }

            // Check against age
            if let Some(age) = input.age {
                if factor_lower.contains("高龄") && age >= 65 {
                    hits += 1;
                }
            }
        }

        if hits == 0 {
            0.0
        } else {
            (hits as f64 / cond.risk_factors.len() as f64).min(1.0)
        }
    }

    /// Score edge alert match (15% weight).
    fn score_edge_alert_match(&self, cond: &MedicalCondition, input: &MatchInput<'_>) -> f64 {
        if cond.key_indicators.edge_alerts.is_empty() {
            return 0.5;
        }

        if input.active_edge_alerts.is_empty() {
            return 0.0;
        }

        let alert_set: HashSet<&str> = input.active_edge_alerts.iter().map(|s| s.as_str()).collect();
        let mut hits = 0;

        for expected_alert in &cond.key_indicators.edge_alerts {
            if alert_set.contains(expected_alert.as_str()) {
                hits += 1;
            }
        }

        if hits == 0 {
            0.0
        } else {
            (hits as f64 / cond.key_indicators.edge_alerts.len() as f64).min(1.0)
        }
    }

    /// Check if a vital sign value matches a rule description.
    /// Handles patterns like ">30", "<10", ">30 或 <10", etc.
    fn check_vital_against_rule(&self, value: f64, rule: &str) -> bool {
        parse_rule_conditions(rule, value)
    }

    /// Check if a vital sign is near (within ~20%) of a rule threshold.
    fn check_vital_near_rule(&self, value: f64, rule: &str) -> bool {
        parse_rule_conditions_near(rule, value, 0.2)
    }
}

// ── Rule Parsing Helpers ─────────────────────────────────────────────────────

/// Parse a rule condition string and check if a value satisfies it.
///
/// Supports: ">N", "<N", ">=N", "<=N", ">N 或 <M", combinations with 或/and/&.
fn parse_rule_conditions(rule: &str, value: f64) -> bool {
    let parts: Vec<&str> = rule.split(&['或', ',', '&', ';']).collect();
    parts.iter().any(|part| {
        let part = part.trim();
        if part.starts_with(">=") || part.starts_with("≥") {
            // UTF-8 safe: strip_prefix avoids slicing into multi-byte characters
            let num_str = part.strip_prefix(">=").or_else(|| part.strip_prefix("≥")).unwrap_or(part).trim();
            if let Ok(threshold) = num_str.parse::<f64>() {
                return value >= threshold;
            }
        } else if part.starts_with('>') {
            if let Ok(threshold) = part[1..].trim().parse::<f64>() {
                return value > threshold;
            }
        } else if part.starts_with("<=") || part.starts_with("≤") {
            let num_str = part.strip_prefix("<=").or_else(|| part.strip_prefix("≤")).unwrap_or(part).trim();
            if let Ok(threshold) = num_str.parse::<f64>() {
                return value <= threshold;
            }
        } else if part.starts_with('<') {
            if let Ok(threshold) = part[1..].trim().parse::<f64>() {
                return value < threshold;
            }
        }
        // Range like "12-20" (normal range check — value outside = match for abnormal)
        else if part.contains('-') {
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() == 2 {
                if let (Ok(lo), Ok(hi)) =
                    (range_parts[0].trim().parse::<f64>(), range_parts[1].trim().parse::<f64>())
                {
                    // For indicator rules, the range typically represents "normal",
                    // and being outside means abnormality matching the condition
                    return value < lo || value > hi;
                }
            }
        }
        false
    })
}

/// Near-threshold check: value is within `margin` ratio of the threshold.
fn parse_rule_conditions_near(rule: &str, value: f64, margin: f64) -> bool {
    // Extract the threshold values and check if value is near them
    let parts: Vec<&str> = rule.split(&['或', ',', '&', ';']).collect();
    parts.iter().any(|part| {
        let part = part.trim();
        let (threshold_str, is_upper) = if part.starts_with('>') {
            (&part[1..], true)
        } else if part.starts_with('<') {
            (&part[1..], false)
        } else if part.contains('-') {
            // For range rules, check proximity to bounds
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() == 2 {
                if let (Ok(lo), Ok(hi)) = (
                    range_parts[0].trim().parse::<f64>(),
                    range_parts[1].trim().parse::<f64>(),
                ) {
                    let near_lo = (value - lo).abs() / lo.max(1.0) <= margin;
                    let near_hi = (value - hi).abs() / hi.max(1.0) <= margin;
                    return near_lo || near_hi;
                }
            }
            return false;
        } else {
            return false;
        };

        if let Ok(threshold) = threshold_str.trim().parse::<f64>() {
            let diff = (value - threshold).abs() / threshold.max(1.0);
            return diff <= margin;
        }
        false
    })
}

/// Check if the observed trend matches the expected trend description.
fn trend_matches(observed: &str, expected: &str) -> bool {
    let obs_lower = observed.to_lowercase();
    let exp_lower = expected.to_lowercase();

    if exp_lower.contains("上升") || exp_lower.contains("加快") || exp_lower.contains("恶化") {
        obs_lower.contains("rising")
    } else if exp_lower.contains("下降") || exp_lower.contains("减慢") || exp_lower.contains("好转")
    {
        obs_lower.contains("falling")
    } else if exp_lower.contains("稳定") {
        obs_lower.contains("stable")
    } else {
        // Partial match
        obs_lower.contains(&exp_lower) || exp_lower.contains(&obs_lower)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_kb() -> MedicalKnowledgeBase {
        MedicalKnowledgeBase::from_json(include_str!("../data/medical_knowledge.json")).unwrap()
    }

    #[test]
    fn test_load_kb() {
        let kb = test_kb();
        assert!(kb.condition_count() >= 5, "Expected >= 5 conditions");
    }

    #[test]
    fn test_match_copd_patient() {
        let kb = test_kb();
        let input = MatchInput {
            breathing_rate: Some(28.0),
            heart_rate: Some(105.0),
            motion_score: 0.4,
            breathing_trend: "Rising",
            heart_trend: "Rising",
            motion_pattern: "IntermittentMotion",
            pre_existing: &["COPD".into(), "高血压".into()],
            age: Some(68),
            active_edge_alerts: &["med_respiratory_distress".into()],
        };

        let matches = kb.match_conditions(&input, 3);
        assert!(!matches.is_empty(), "Should have matches");

        // COPD exacerbation should be high-ranked
        let copd_match = matches.iter().find(|(c, _)| c.id == "copd_exacerbation");
        assert!(copd_match.is_some(), "COPD exacerbation should match");
        assert!(
            copd_match.unwrap().1 > 0.3,
            "COPD match score should be > 0.3"
        );

        println!("Match results:");
        for (cond, score) in &matches {
            println!("  {} ({:.2})", cond.name, score);
        }
    }

    #[test]
    fn test_match_normal_patient() {
        let kb = test_kb();
        let input = MatchInput {
            breathing_rate: Some(16.0),
            heart_rate: Some(72.0),
            motion_score: 0.2,
            breathing_trend: "Stable",
            heart_trend: "Stable",
            motion_pattern: "ContinuousStill",
            pre_existing: &[],
            age: Some(30),
            active_edge_alerts: &[],
        };

        let matches = kb.match_conditions(&input, 3);
        println!("Normal patient matches:");
        for (cond, score) in &matches {
            println!("  {} ({:.2})", cond.name, score);
        }
        // Normal vitals should produce low match scores
        assert!(
            matches.iter().all(|(_, s)| *s < 0.5),
            "Normal patient should have low match scores"
        );
    }

    #[test]
    fn test_rule_parsing() {
        assert!(parse_rule_conditions(">30", 35.0));
        assert!(!parse_rule_conditions(">30", 28.0));
        assert!(parse_rule_conditions("<10", 8.0));
        assert!(!parse_rule_conditions("<10", 14.0));
        assert!(parse_rule_conditions(">30 或 <10", 35.0));
        assert!(parse_rule_conditions(">30 或 <10", 8.0));
        assert!(!parse_rule_conditions(">30 或 <10", 16.0));
        // Range: outside normal = match
        assert!(parse_rule_conditions("12-20", 30.0)); // above
        assert!(parse_rule_conditions("12-20", 8.0)); // below
        assert!(!parse_rule_conditions("12-20", 16.0)); // normal
    }
}
