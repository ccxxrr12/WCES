//! Fallback Analyzer (L2 Degradation)
//!
//! When the LLM is unavailable, timed out, or not loaded,
//! this module provides template-based structured analysis
//! using the same medical knowledge base for condition matching.
//!
//! The fallback output is structurally identical to LLM output
//! so the triage UI can consume both seamlessly.

use crate::medical_knowledge::MedicalCondition;
use crate::patient_record::PatientRecord;
use crate::sliding_window::{TrendDirection, VitalTrendSummary};
use serde::Serialize;

// ── Analysis Output Types ───────────────────────────────────────────────────

/// Complete analysis result — same structure whether from LLM or fallback.
#[derive(Debug, Clone, Serialize)]
pub struct LlmAnalysisResult {
    pub patient_id: String,
    pub generated_at: String,
    pub generated_by: String, // "llm" or "fallback_template"
    pub analysis_time_ms: u64,

    pub risk_assessment: RiskAssessment,
    pub condition_match: ConditionMatch,
    pub triage_opinion: TriageOpinion,
    pub trend_analysis: TrendAnalysis,
    pub history_relevance: HistoryRelevance,
    pub recommendations: Vec<String>,
    pub monitoring_priority: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskAssessment {
    pub overall_level: String,
    pub primary_concern: String,
    pub deteriorating: bool,
    pub deterioration_evidence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConditionMatch {
    pub most_likely: String,
    pub confidence: String,
    pub differential: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriageOpinion {
    pub agrees_with_start: bool,
    pub suggested_level: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrendAnalysis {
    pub respiratory: String,
    pub cardiac: String,
    pub combined: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRelevance {
    pub relevant_conditions: Vec<String>,
    pub impact_on_assessment: String,
}

// ── Fallback Analyzer ───────────────────────────────────────────────────────

/// Context for fallback analysis.
pub struct FallbackContext {
    pub patient: PatientRecord,
    pub current_rr: Option<f64>,
    pub current_hr: Option<f64>,
    pub current_motion: f64,
    pub current_signal_quality: f64,
    pub current_triage: String,
    pub trend_summary: VitalTrendSummary,
    pub matched_conditions: Vec<(MedicalCondition, f64)>,
    pub active_edge_alerts: Vec<String>,
}

/// Rule-based fallback analysis engine.
pub struct FallbackAnalyzer;

impl FallbackAnalyzer {
    /// Generate structured analysis without LLM inference.
    pub fn analyze(ctx: &FallbackContext) -> LlmAnalysisResult {
        let overall_level = determine_overall_level(
            &ctx.current_triage,
            &ctx.trend_summary,
            &ctx.matched_conditions,
        );
        let primary_concern = determine_primary_concern(
            &ctx.current_triage,
            &ctx.trend_summary,
            &ctx.matched_conditions,
        );
        let (deteriorating, det_evidence) =
            determine_deterioration(&ctx.trend_summary, &ctx.matched_conditions);

        let (most_likely, confidence, differential) =
            determine_condition_match(&ctx.matched_conditions);

        let (agrees, suggested_level, reason) = determine_triage_opinion(
            &ctx.current_triage,
            &ctx.trend_summary,
            &ctx.matched_conditions,
        );

        let (resp_analysis, card_analysis, combined_analysis) = analyze_trends(&ctx.trend_summary);

        let (relevant_conditions, history_impact) =
            analyze_history(&ctx.patient, &ctx.matched_conditions);

        let recommendations = generate_recommendations(
            &ctx.current_triage,
            &ctx.matched_conditions,
            &ctx.trend_summary,
            &ctx.active_edge_alerts,
        );

        let monitoring = determine_monitoring_priority(
            &ctx.trend_summary,
            &ctx.matched_conditions,
            &ctx.active_edge_alerts,
        );

        LlmAnalysisResult {
            patient_id: ctx.patient.patient_id.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            generated_by: "fallback_template".into(),
            analysis_time_ms: 0,

            risk_assessment: RiskAssessment {
                overall_level,
                primary_concern,
                deteriorating,
                deterioration_evidence: det_evidence,
            },
            condition_match: ConditionMatch {
                most_likely,
                confidence,
                differential,
            },
            triage_opinion: TriageOpinion {
                agrees_with_start: agrees,
                suggested_level,
                reason,
            },
            trend_analysis: TrendAnalysis {
                respiratory: resp_analysis,
                cardiac: card_analysis,
                combined: combined_analysis,
            },
            history_relevance: HistoryRelevance {
                relevant_conditions,
                impact_on_assessment: history_impact,
            },
            recommendations,
            monitoring_priority: monitoring,
        }
    }
}

// ── Rule-Based Analysis Functions ───────────────────────────────────────────

fn determine_overall_level(
    triage: &str,
    trends: &VitalTrendSummary,
    matches: &[(MedicalCondition, f64)],
) -> String {
    // BASE: START triage level
    let base_level = match triage {
        "Immediate" => "critical",
        "Delayed" => "high",
        "Minor" => "low",
        "Deceased" => "critical",
        _ => "moderate",
    };

    // UPGRADE: deteriorating trends
    let trend_danger = matches!(trends.rr_trend, TrendDirection::Rising)
        && trends.rr_change_pct > 20.0
        || matches!(trends.hr_trend, TrendDirection::Rising) && trends.hr_change_pct > 15.0;

    // UPGRADE: high-confidence critical condition match
    let has_critical_match = matches
        .first()
        .map(|(c, s)| c.urgency == "critical" && *s > 0.5)
        .unwrap_or(false);

    // Critical patients must never be downgraded — skip elevation when already at "critical".
    if (has_critical_match || trend_danger) && base_level != "critical" {
        if base_level == "low" { "moderate" } else { "high" }.into()
    } else {
        base_level.into()
    }
}

fn determine_primary_concern(
    triage: &str,
    trends: &VitalTrendSummary,
    matches: &[(MedicalCondition, f64)],
) -> String {
    // Priority 1: High-confidence condition match
    if let Some((cond, score)) = matches.first() {
        if *score > 0.4 {
            return format!("符合{}特征 (匹配度{:.0}%)", cond.name, score * 100.0);
        }
    }

    // Priority 2: Vital sign anomalies
    let mut concerns = Vec::new();
    if trends.rr_mean > 30.0 {
        concerns.push(format!(
            "呼吸急促 (均值{:.1}次/分，超过危急阈值30)",
            trends.rr_mean
        ));
    } else if trends.rr_mean < 10.0 {
        concerns.push(format!(
            "呼吸过缓 (均值{:.1}次/分，低于危急阈值10)",
            trends.rr_mean
        ));
    }

    if trends.hr_mean > 120.0 {
        concerns.push(format!(
            "心动过速 (均值{:.0}BPM，超过危急阈值120)",
            trends.hr_mean
        ));
    } else if trends.hr_mean < 40.0 {
        concerns.push(format!(
            "心动过缓 (均值{:.0}BPM，低于危急阈值40)",
            trends.hr_mean
        ));
    }

    if !concerns.is_empty() {
        return concerns.join("；");
    }

    // Priority 3: Trend-based concern
    if matches!(trends.rr_trend, TrendDirection::Rising) {
        return format!(
            "呼吸率呈上升趋势 (+{:.1}%)，需持续关注",
            trends.rr_change_pct
        );
    }
    if matches!(trends.hr_trend, TrendDirection::Rising) {
        return format!(
            "心率呈上升趋势 (+{:.1}%)，需持续关注",
            trends.hr_change_pct
        );
    }

    format!("当前体征稳定，{}分诊合理", triage)
}

fn determine_deterioration(
    trends: &VitalTrendSummary,
    matches: &[(MedicalCondition, f64)],
) -> (bool, String) {
    let mut evidence = Vec::new();

    // Check RR trend
    if matches!(trends.rr_trend, TrendDirection::Rising) && trends.rr_change_pct > 10.0 {
        evidence.push(format!(
            "呼吸率上升{:.1}% (均值{:.1}→趋势↑)",
            trends.rr_change_pct, trends.rr_mean
        ));
    } else if matches!(trends.rr_trend, TrendDirection::Falling) && trends.rr_change_pct < -15.0 {
        evidence.push(format!(
            "呼吸率下降{:.1}% (可能提示呼吸抑制)",
            trends.rr_change_pct.abs()
        ));
    }

    // Check HR trend
    if matches!(trends.hr_trend, TrendDirection::Rising) && trends.hr_change_pct > 10.0 {
        evidence.push(format!(
            "心率上升{:.1}% (均值{:.1}→趋势↑)",
            trends.hr_change_pct, trends.hr_mean
        ));
    }

    // Check motion pattern
    if matches!(trends.motion_pattern, crate::sliding_window::MotionPattern::GradualDecline) {
        evidence.push("运动评分逐渐下降 (可能提示意识水平降低)".into());
    }
    if matches!(trends.motion_pattern, crate::sliding_window::MotionPattern::SpikeAndDrop) {
        evidence.push("运动评分突增后骤降 (可能提示癫痫/惊厥发作)".into());
    }

    // Check condition match severity
    if let Some((cond, score)) = matches.first() {
        if *score > 0.5 && (cond.urgency == "critical" || cond.urgency == "high") {
            evidence.push(format!("体征模式符合{}特征", cond.name));
        }
    }

    let deteriorating = !evidence.is_empty();
    let det_evidence = if deteriorating {
        evidence.join("；")
    } else {
        "无明显恶化趋势".into()
    };

    (deteriorating, det_evidence)
}

fn determine_condition_match(
    matches: &[(MedicalCondition, f64)],
) -> (String, String, Vec<String>) {
    if let Some((cond, score)) = matches.first() {
        let confidence = if *score > 0.6 {
            "high"
        } else if *score > 0.35 {
            "medium"
        } else {
            "low"
        };

        let diff = if cond.differential.is_empty() {
            vec!["无明显鉴别诊断".into()]
        } else {
            cond.differential[..cond.differential.len().min(2)].to_vec()
        };

        (cond.name.clone(), confidence.into(), diff)
    } else {
        (
            "无明确模式匹配".into(),
            "low".into(),
            vec!["生命体征在正常范围内".into()],
        )
    }
}

fn determine_triage_opinion(
    current_triage: &str,
    trends: &VitalTrendSummary,
    matches: &[(MedicalCondition, f64)],
) -> (bool, String, String) {
    let mut agrees = true;
    let mut suggested = current_triage.to_string();
    let mut reason = String::new();

    // Check if deteriorating trends suggest upgrade
    let rr_critical = trends.rr_mean > 30.0 && matches!(trends.rr_trend, TrendDirection::Rising);
    let hr_critical = trends.hr_mean > 120.0 && matches!(trends.hr_trend, TrendDirection::Rising);

    if matches!(trends.motion_pattern, crate::sliding_window::MotionPattern::SpikeAndDrop) {
        if current_triage != "Immediate" {
            agrees = false;
            suggested = "Immediate".into();
            reason = "运动评分突增后骤降，疑似癫痫发作，建议升级为红色优先级".into();
            return (agrees, suggested, reason);
        }
    }

    if (rr_critical || hr_critical) && current_triage != "Immediate" {
        agrees = false;
        suggested = "Immediate".into();

        let mut reasons = Vec::new();
        if rr_critical {
            reasons.push(format!(
                "呼吸率均值{:.1}次/分，超过危急阈值且持续上升",
                trends.rr_mean
            ));
        }
        if hr_critical {
            reasons.push(format!(
                "心率均值{:.0}BPM，超过危急阈值且持续上升",
                trends.hr_mean
            ));
        }
        reason = reasons.join("；");
        reason.push_str("，建议升级为红色优先级");
    } else if let Some((cond, _score)) = matches.first() {
        if cond.triage_recommendation != current_triage
            && (cond.urgency == "critical" || cond.urgency == "high")
        {
            // Only suggest upgrade, never downgrade
            if !is_upgrade(current_triage, &cond.triage_recommendation) {
                agrees = true;
                reason = format!("认可START {}判断，符合{}特征", current_triage, cond.name);
            } else {
                agrees = false;
                suggested = extract_triage_level(&cond.triage_recommendation);
                reason = format!(
                    "体征模式符合{}特征 (紧急度: {})，建议考虑升级到{}",
                    cond.name, cond.urgency, suggested
                );
            }
        } else {
            reason = format!("认可START {}判断", current_triage);
        }
    } else {
        reason = format!("认可START {}判断，生命体征在预期范围内", current_triage);
    }

    (agrees, suggested, reason)
}

fn analyze_trends(trends: &VitalTrendSummary) -> (String, String, String) {
    let rr_trend = match trends.rr_trend {
        TrendDirection::Rising => format!(
            "呼吸率呈上升趋势 (均值{:.1}次/分，+{:.1}%)，波动系数{:.2}",
            trends.rr_mean, trends.rr_change_pct, trends.rr_volatility
        ),
        TrendDirection::Falling => format!(
            "呼吸率呈下降趋势 (均值{:.1}次/分，{:.1}%)，需警惕呼吸抑制",
            trends.rr_mean, trends.rr_change_pct
        ),
        TrendDirection::Stable => format!(
            "呼吸率稳定 (均值{:.1}次/分，波动系数{:.2})",
            trends.rr_mean, trends.rr_volatility
        ),
    };

    let hr_trend = match trends.hr_trend {
        TrendDirection::Rising => format!(
            "心率呈上升趋势 (均值{:.0}BPM，+{:.1}%)，波动系数{:.2}",
            trends.hr_mean, trends.hr_change_pct, trends.hr_volatility
        ),
        TrendDirection::Falling => format!(
            "心率呈下降趋势 (均值{:.0}BPM，{:.1}%)，需警惕心输出量下降",
            trends.hr_mean, trends.hr_change_pct
        ),
        TrendDirection::Stable => format!(
            "心率稳定 (均值{:.0}BPM，波动系数{:.2})",
            trends.hr_mean, trends.hr_volatility
        ),
    };

    let combined = if matches!(trends.rr_trend, TrendDirection::Rising)
        && matches!(trends.hr_trend, TrendDirection::Rising)
    {
        format!(
            "⚠️ 呼吸和心率同步上升 — 提示全身性应激反应或代偿机制激活。建议密切监测，排查感染、疼痛、低血容量等原因。"
        )
    } else if matches!(trends.rr_trend, TrendDirection::Rising)
        && matches!(trends.hr_trend, TrendDirection::Stable)
    {
        format!("呼吸率上升但心率稳定 — 倾向于单纯呼吸系统问题，心输出量代偿尚可。")
    } else if matches!(trends.hr_trend, TrendDirection::Rising)
        && matches!(trends.rr_trend, TrendDirection::Stable)
    {
        format!("心率上升但呼吸稳定 — 可能为心脏问题、疼痛或焦虑。需排除器质性心脏病。")
    } else {
        format!("呼吸和心率趋势一致，无明显代偿失衡。")
    };

    (rr_trend, hr_trend, combined)
}

fn analyze_history(patient: &PatientRecord, matches: &[(MedicalCondition, f64)]) -> (Vec<String>, String) {
    let mut relevant = Vec::new();

    // Find pre-existing conditions relevant to current matches
    for (cond, _score) in matches {
        for factor in &cond.risk_factors {
            for pre in &patient.pre_existing {
                if pre.to_lowercase().contains(&factor.to_lowercase())
                    || factor.to_lowercase().contains(&pre.to_lowercase())
                {
                    if !relevant.contains(pre) {
                        relevant.push(pre.clone());
                    }
                }
            }
        }
    }

    let impact = if relevant.is_empty() {
        if patient.pre_existing.is_empty() {
            "无既往病史，评估基于当前体征".into()
        } else {
            format!(
                "既往病史 ({}) 与当前体征模式无明显关联",
                patient.pre_existing.join("、")
            )
        }
    } else {
        format!(
            "既往病史 ({}) 增加了当前风险。{}",
            relevant.join("、"),
            match relevant.first().map(|s| s.as_str()) {
                Some("COPD") => "COPD患者呼吸代偿储备降低，对呼吸率升高的耐受性更差。",
                Some("心脏病") | Some("高血压") => "心血管病史患者对心率异常的耐受性降低。",
                Some("糖尿病") => "糖尿病患者感染风险增加，需关注脓毒症早期征象。",
                _ => "建议综合考虑既往病史对当前状态的影响。",
            }
        )
    };

    (relevant, impact)
}

fn generate_recommendations(
    triage: &str,
    matches: &[(MedicalCondition, f64)],
    trends: &VitalTrendSummary,
    alerts: &[String],
) -> Vec<String> {
    let mut recs = Vec::new();

    // Priority 1: From matched conditions
    if let Some((cond, score)) = matches.first() {
        if *score > 0.35 {
            for action in &cond.actions {
                if recs.len() < 4 {
                    recs.push(action.clone());
                }
            }
        }
    }

    // Priority 2: Based on trends
    if recs.is_empty() {
        if matches!(trends.rr_trend, TrendDirection::Rising) && trends.rr_change_pct > 15.0 {
            recs.push("密切监测呼吸频率，每5分钟评估一次".into());
            if trends.rr_mean > 25.0 {
                recs.push("考虑给予氧疗，监测血氧饱和度".into());
            }
        }
        if matches!(trends.hr_trend, TrendDirection::Rising) && trends.hr_change_pct > 15.0 {
            recs.push("持续心电监测，评估心率上升原因".into());
        }
    }

    // Priority 3: General
    if recs.is_empty() {
        match triage {
            "Immediate" => {
                recs.push("持续监测生命体征，准备随时干预".into());
                recs.push("每5分钟评估一次生命体征趋势".into());
            }
            "Delayed" => {
                recs.push("每15分钟监测一次生命体征".into());
                recs.push("如有恶化趋势立即报告".into());
            }
            _ => {
                recs.push("定期生命体征监测".into());
            }
        }
    }

    // Edge alert recommendations
    if alerts.contains(&"med_seizure_detect".to_string()) {
        recs.insert(0, "保护伤员，防止二次伤害，记录发作持续时间".into());
    }
    if alerts.contains(&"med_cardiac_arrhythmia".to_string()) {
        recs.insert(0, "立即评估意识状态和脉搏，准备除颤设备".into());
    }

    recs.truncate(5);
    recs
}

fn determine_monitoring_priority(
    trends: &VitalTrendSummary,
    matches: &[(MedicalCondition, f64)],
    alerts: &[String],
) -> Vec<String> {
    let mut priority = Vec::new();

    // From matched conditions
    if let Some((cond, _score)) = matches.first() {
        for focus in &cond.monitoring_focus {
            if priority.len() < 4 {
                priority.push(focus.clone());
            }
        }
    }

    // From trends
    if matches!(trends.rr_trend, TrendDirection::Rising) && trends.rr_change_pct > 10.0 {
        priority.push("呼吸频率变化趋势 (持续上升)".into());
    }
    if matches!(trends.hr_trend, TrendDirection::Rising) && trends.hr_change_pct > 10.0 {
        priority.push("心率变化趋势 (持续上升)".into());
    }

    if priority.is_empty() {
        priority.push("生命体征稳定性 (呼吸率+心率)".into());
        priority.push("运动评分 (活动状态)".into());
    }

    // Remove duplicates while preserving order
    let mut seen = std::collections::HashSet::new();
    priority.retain(|p| seen.insert(p.clone()));
    priority.truncate(5);

    priority
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn is_upgrade(current: &str, suggested: &str) -> bool {
    let priority = |level: &str| -> u8 {
        match level {
            "Immediate" | "Immediate (红)" => 4,
            "Delayed" | "Delayed (黄)" => 3,
            "Minor" | "Minor (绿)" => 2,
            "Deceased" | "Deceased (黑)" => 1,
            _ => 0,
        }
    };

    let cur = priority(current);
    let sug = priority(&extract_triage_level(suggested));
    sug > cur
}

fn extract_triage_level(raw: &str) -> String {
    if raw.contains("Immediate") || raw.contains("红") {
        "Immediate".into()
    } else if raw.contains("Delayed") || raw.contains("黄") {
        "Delayed".into()
    } else if raw.contains("Minor") || raw.contains("绿") {
        "Minor".into()
    } else if raw.contains("Deceased") || raw.contains("黑") {
        "Deceased".into()
    } else {
        raw.to_string()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::medical_knowledge::MedicalKnowledgeBase;
    use crate::patient_record::{PatientRecord};
    use crate::sliding_window::{MotionPattern, TrendDirection, VitalTrendSummary};

    #[test]
    fn test_fallback_analysis_critical() {
        let patient = PatientRecord {
            patient_id: "PAT-001".into(),
            name: None,
            age: Some(68),
            gender: None,
            pre_existing: vec!["COPD".into()],
            chief_complaint: Some("呼吸困难".into()),
            allergies: vec![],
            medications: vec![],
            node_id: Some(2),
            admission_time: None,
            notes: None,
        };

        let trend = VitalTrendSummary {
            window_seconds: 300.0,
            sample_count: 60,
            rr_mean: 32.0,
            rr_min: 25.0,
            rr_max: 38.0,
            rr_trend: TrendDirection::Rising,
            rr_change_pct: 25.0,
            rr_volatility: 0.15,
            hr_mean: 118.0,
            hr_min: 100.0,
            hr_max: 130.0,
            hr_trend: TrendDirection::Rising,
            hr_change_pct: 18.0,
            hr_volatility: 0.1,
            motion_mean: 0.4,
            motion_min: 0.1,
            motion_max: 0.6,
            motion_pattern: MotionPattern::IntermittentMotion,
            signal_quality_mean: 0.8,
            signal_quality_trend: TrendDirection::Stable,
        };

        let kb = MedicalKnowledgeBase::from_json(
            include_str!("../data/medical_knowledge.json")
        ).unwrap();

        use crate::medical_knowledge::MatchInput;
        let match_input = MatchInput {
            breathing_rate: Some(32.0),
            heart_rate: Some(118.0),
            motion_score: 0.4,
            breathing_trend: "Rising",
            heart_trend: "Rising",
            motion_pattern: "IntermittentMotion",
            pre_existing: &["COPD".into()],
            age: Some(68),
            active_edge_alerts: &["med_respiratory_distress".into()],
        };
        let matched = kb.match_conditions(&match_input, 3);

        let ctx = FallbackContext {
            patient,
            current_rr: Some(32.0),
            current_hr: Some(118.0),
            current_motion: 0.4,
            current_signal_quality: 0.8,
            current_triage: "Immediate".into(),
            trend_summary: trend,
            matched_conditions: matched,
            active_edge_alerts: vec!["med_respiratory_distress".into()],
        };

        let result = FallbackAnalyzer::analyze(&ctx);
        println!(
            "Fallback result:\n{}",
            serde_json::to_string_pretty(&result).unwrap()
        );

        assert_eq!(result.patient_id, "PAT-001");
        assert_eq!(result.generated_by, "fallback_template");
        assert_eq!(result.risk_assessment.overall_level, "critical");
        assert!(result.risk_assessment.deteriorating);
        assert!(!result.recommendations.is_empty());
    }

    #[test]
    fn test_fallback_analysis_stable() {
        let patient = PatientRecord::new("PAT-002");

        let trend = VitalTrendSummary {
            window_seconds: 300.0,
            sample_count: 60,
            rr_mean: 16.0,
            rr_min: 14.0,
            rr_max: 18.0,
            rr_trend: TrendDirection::Stable,
            rr_change_pct: 0.0,
            rr_volatility: 0.05,
            hr_mean: 72.0,
            hr_min: 68.0,
            hr_max: 76.0,
            hr_trend: TrendDirection::Stable,
            hr_change_pct: 0.0,
            hr_volatility: 0.04,
            motion_mean: 0.1,
            motion_min: 0.05,
            motion_max: 0.15,
            motion_pattern: MotionPattern::ContinuousStill,
            signal_quality_mean: 0.95,
            signal_quality_trend: TrendDirection::Stable,
        };

        let ctx = FallbackContext {
            patient,
            current_rr: Some(16.0),
            current_hr: Some(72.0),
            current_motion: 0.1,
            current_signal_quality: 0.95,
            current_triage: "Minor".into(),
            trend_summary: trend,
            matched_conditions: vec![],
            active_edge_alerts: vec![],
        };

        let result = FallbackAnalyzer::analyze(&ctx);
        assert_eq!(result.risk_assessment.overall_level, "low");
        assert!(!result.risk_assessment.deteriorating);
        assert!(result.triage_opinion.agrees_with_start);
    }
}
