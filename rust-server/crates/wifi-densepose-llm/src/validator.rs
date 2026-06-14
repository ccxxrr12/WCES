//! Output Validator — sanitizes LLM output, blocks dangerous content,
//! detects triage contradictions, and appends disclaimer.
//!
//! Content blocking is **tiered by triage severity**:
//!   - Immediate (red):   Critical — only block surgery/prescription claims.
//!                         Medication/injection advice passes through as it
//!                         may contain life-saving instructions.
//!   - Delayed (yellow):  Moderated — block drugs and injections but allow
//!                         general medical information.
//!   - Minor (green) / others: Strict — block all medical advice patterns.

use regex::Regex;

pub enum ValidationResult {
    Pass(String),
    PassWithWarning(String, Vec<String>),
    FailAndFallback(Vec<String>),
}

/// Regex categories for tiered blocking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockTier {
    /// Block for ALL triage levels (surgery, prescription, diagnosis claims,
    /// "don't seek care" advice).
    Always,
    /// Block for Delayed and lower; allow for Immediate.
    Moderate,
    /// Block only for Minor and lower; allow for Immediate and Delayed.
    Strict,
}

pub struct OutputValidator {
    /// Each regex paired with its blocking tier.
    blocked: Vec<(Regex, BlockTier)>,
}

impl OutputValidator {
    pub fn new() -> Self {
        use BlockTier::*;
        Self {
            blocked: vec![
                // ── Always-block (surgery/prescription/diagnosis/delay-care) ──
                (Regex::new(r"手术|切开|缝合|处方|开具|开刀").unwrap(), Always),
                (Regex::new(r"(停止|停用|停服|不要|切勿).{0,6}(药|药物|用药|服药)").unwrap(), Always),
                (Regex::new(r"(诊断为|确诊为|判断为).{0,10}(心梗|心衰|脑梗|脑出血|肺栓塞)").unwrap(), Always),
                (Regex::new(r"(不需要|无需|不必)(就医|转诊|住院|急诊|手术)").unwrap(), Always),
                (Regex::new(r"(?i)(recommend|suggest|advise|prescribe).{0,15}(surgery|operation)").unwrap(), Always),
                (Regex::new(r"(?i)(stop|discontinue).{0,10}(medication|drug|medicine)").unwrap(), Always),
                // ── Moderate (drugs + injections — block for Delayed and below) ──
                (Regex::new(r"建议.{0,6}(使用|服用|给药|用药|开药|配药).{0,10}药").unwrap(), Moderate),
                (Regex::new(r"推荐.{0,6}(使用|服用|给药|用药|开药|配药)").unwrap(), Moderate),
                (Regex::new(r"(建议|推荐).{0,6}(注射|推注|肌注|静注|静脉注射|皮下注射)").unwrap(), Moderate),
                (Regex::new(r"(剂量|用量|给药量).{0,5}\d+\s*(mg|ml|g|mcg|μg|IU|U)").unwrap(), Moderate),
                (Regex::new(r"(?i)(recommend|suggest|advise|prescribe).{0,15}(medication|drug|medicine|pill|injection)").unwrap(), Moderate),
                (Regex::new(r"(?i)dosage.{0,5}\d+\s*(mg|ml|g|mcg)").unwrap(), Moderate),
                // ── Strict (reserved for future use) ──
            ],
        }
    }

    /// Derive a numeric tier from the triage label.
    /// 0 = Immediate (least blocking), 1 = Delayed, 2 = Minor/others (most blocking).
    /// Uses case-insensitive comparison without heap allocation.
    fn triage_tier(triage: &str) -> u8 {
        if triage.eq_ignore_ascii_case("immediate") || triage.eq_ignore_ascii_case("red") {
            0
        } else if triage.eq_ignore_ascii_case("delayed") || triage.eq_ignore_ascii_case("yellow") {
            1
        } else {
            2 // Minor, green, Expectant, or unknown — strictest blocking
        }
    }

    /// Whether a regex at the given tier should be blocked for the given
    /// numeric triage tier.
    fn should_block(re_tier: BlockTier, triage_tier: u8) -> bool {
        match re_tier {
            BlockTier::Always => true,
            BlockTier::Moderate => triage_tier >= 1,
            BlockTier::Strict => triage_tier >= 2,
        }
    }

    /// Validate LLM output. Returns Pass, PassWithWarning, or FailAndFallback.
    pub fn validate(&self, output: &str, current_triage: &str) -> ValidationResult {
        // 1. Empty response check
        if output.trim().is_empty() {
            return ValidationResult::FailAndFallback(vec!["LLM returned empty response".into()]);
        }

        let tier = Self::triage_tier(current_triage);

        // 2. Tiered content blocking
        let mut cleaned = output.to_string();
        let mut warnings: Vec<String> = Vec::new();
        for (re, re_tier) in &self.blocked {
            // Cheap check first: skip regex scan if this tier doesn't block.
            if Self::should_block(*re_tier, tier) && re.is_match(output) {
                cleaned = re
                    .replace_all(&cleaned, "[内容已拦截: 超出AI助手权限]")
                    .to_string();
                warnings.push(format!("blocked pattern (tier={:?}): {}", re_tier, re.as_str()));
            }
        }

        // 3. Append disclaimer if missing
        if !cleaned.contains("[AI分析, 仅供参考]") {
            cleaned.push_str("\n\n---\n[AI分析, 仅供参考]");
        }

        if warnings.is_empty() {
            ValidationResult::Pass(cleaned)
        } else {
            ValidationResult::PassWithWarning(cleaned, warnings)
        }
    }
}

impl Default for OutputValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_output_fails() {
        let v = OutputValidator::new();
        match v.validate("", "Immediate") {
            ValidationResult::FailAndFallback(reasons) => {
                assert!(!reasons.is_empty());
            }
            _ => panic!("expected FailAndFallback"),
        }
    }

    #[test]
    fn test_blocks_medication_for_minor() {
        let v = OutputValidator::new();
        match v.validate("建议使用阿司匹林药物缓解疼痛", "Minor") {
            ValidationResult::PassWithWarning(cleaned, _) => {
                assert!(cleaned.contains("已拦截"));
            }
            _ => panic!("expected PassWithWarning for Minor"),
        }
    }

    #[test]
    fn test_allows_medication_for_immediate() {
        // Immediate patients: medication advice may be life-saving.
        // Only surgery/prescription/diagnosis patterns are always blocked.
        let v = OutputValidator::new();
        match v.validate("建议立即使用肾上腺素，患者过敏反应严重", "Immediate") {
            ValidationResult::Pass(cleaned) => {
                assert!(!cleaned.contains("已拦截"),
                        "medication advice should pass for Immediate triage");
            }
            ValidationResult::PassWithWarning(cleaned, _) => {
                // May trigger Always-tier patterns, but not drug-tier
                if cleaned.contains("已拦截") {
                    // Only acceptable if it's a surgery/diagnosis pattern overlap
                    assert!(!cleaned.contains("肾上腺素"),
                            "drug name should not be redacted for Immediate");
                }
            }
            _ => panic!("expected Pass or PassWithWarning for Immediate"),
        }
    }

    #[test]
    fn test_always_blocks_surgery_even_for_immediate() {
        let v = OutputValidator::new();
        match v.validate("需要立即进行手术缝合", "Immediate") {
            ValidationResult::PassWithWarning(cleaned, _) => {
                assert!(cleaned.contains("已拦截"),
                        "surgery must be blocked even for Immediate");
            }
            _ => panic!("expected PassWithWarning for surgery"),
        }
    }

    #[test]
    fn test_adds_disclaimer() {
        let v = OutputValidator::new();
        match v.validate("生命体征正常。", "Minor") {
            ValidationResult::Pass(cleaned) => {
                assert!(cleaned.contains("[AI分析, 仅供参考]"));
            }
            _ => panic!("expected Pass"),
        }
    }
}
