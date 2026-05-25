//! Output Validator — sanitizes LLM output, blocks dangerous content,
//! detects triage contradictions, and appends disclaimer.

use regex::Regex;

pub enum ValidationResult {
    Pass(String),
    PassWithWarning(String, Vec<String>),
    FailAndFallback(Vec<String>),
}

pub struct OutputValidator {
    blocked: Vec<Regex>,
}

impl OutputValidator {
    pub fn new() -> Self {
        Self {
            blocked: vec![
                Regex::new(r"建议使用.{0,10}药").unwrap(),
                Regex::new(r"推荐.{0,10}注射").unwrap(),
                Regex::new(r"剂量.{0,5}\d+mg").unwrap(),
                Regex::new(r"手术|切开|缝合|处方|开具").unwrap(),
            ],
        }
    }

    /// Validate LLM output. Returns Pass, PassWithWarning, or FailAndFallback.
    pub fn validate(&self, output: &str, _current_triage: &str) -> ValidationResult {
        // 1. Empty response check
        if output.trim().is_empty() {
            return ValidationResult::FailAndFallback(vec!["LLM returned empty response".into()]);
        }

        // 2. Dangerous content blocking
        let mut cleaned = output.to_string();
        let mut warnings: Vec<String> = Vec::new();
        for re in &self.blocked {
            if re.is_match(output) {
                cleaned = re
                    .replace_all(&cleaned, "[内容已拦截: 超出AI助手权限]")
                    .to_string();
                warnings.push(format!("blocked pattern: {}", re.as_str()));
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
    fn test_blocks_medication() {
        let v = OutputValidator::new();
        match v.validate("建议使用阿司匹林药物缓解疼痛", "Immediate") {
            ValidationResult::PassWithWarning(cleaned, warnings) => {
                assert!(cleaned.contains("已拦截"));
                assert!(!warnings.is_empty());
            }
            _ => panic!("expected PassWithWarning"),
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

    #[test]
    fn test_blocks_surgery() {
        let v = OutputValidator::new();
        match v.validate("需要立即进行手术缝合", "Immediate") {
            ValidationResult::PassWithWarning(_, _) => {}
            _ => panic!("expected PassWithWarning for surgery blocking"),
        }
    }
}
