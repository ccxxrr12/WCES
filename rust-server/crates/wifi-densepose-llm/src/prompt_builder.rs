//! Prompt Builder
//!
//! Assembles LLM analysis prompts from multiple context sources:
//! patient history, medical knowledge matches, vital trends, and current vitals.

use crate::medical_knowledge::MedicalCondition;
use crate::patient_record::PatientRecord;
use crate::sliding_window::VitalTrendSummary;

/// Result of building a prompt — the full prompt string.
pub struct BuiltPrompt {
    /// The complete prompt text to send to the LLM.
    pub prompt: String,
    /// Approximate token count (for logging/monitoring).
    pub estimated_tokens: usize,
}

/// Context data needed to build an analysis prompt.
pub struct PromptContext {
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

/// Builder for constructing LLM analysis prompts.
pub struct PromptBuilder;

impl PromptBuilder {
    /// Build a structured analysis prompt for the LLM.
    pub fn build(ctx: &PromptContext) -> BuiltPrompt {
        let prompt = format!(
            r#"# 角色
你是一名方舱医院的急救医师，负责根据WiFi CSI无接触监测系统采集的生命体征数据，
为医护人员提供辅助分析。你的分析仅供参考，最终决策由医护人员做出。

# 伤员基本信息
- ID: {patient_id}
- 年龄: {age}
- 性别: {gender}
{pre_existing}
{chief_complaint}
{medications}

# 当前体征数据
- 呼吸率: {rr} 次/分钟 (正常: 12-20, 危急: <10或>30)
- 心率: {hr} 次/分钟 (正常: 60-100)
- 信号质量: {signal_quality}
- 运动状态: {motion_state}
- START分诊: {triage}

# 趋势分析 (过去5分钟)
- 呼吸率: 均值{rr_mean}/min, 趋势{rr_trend}(变化{rr_change}%), 波动系数{rr_vol}
- 心率: 均值{hr_mean}/min, 趋势{hr_trend}(变化{hr_change}%), 波动系数{hr_vol}
- 运动模式: {motion_pattern}
- 信号质量均值: {sq_mean}

# 活跃告警
{alerts}

# 参考知识 (体征模式匹配)
{matched_conditions}

# 分析要求
请基于以上所有信息进行联合分析，输出以下内容（JSON格式）：

```json
{{
  "risk_assessment": {{
    "overall_level": "critical/high/moderate/low",
    "primary_concern": "最主要的医学担忧",
    "deteriorating": true/false,
    "deterioration_evidence": "恶化的具体证据"
  }},
  "condition_match": {{
    "most_likely": "最可能的疾病/状态",
    "confidence": "high/medium/low",
    "differential": ["鉴别诊断1", "鉴别诊断2"]
  }},
  "triage_opinion": {{
    "agrees_with_start": true/false,
    "suggested_level": "Immediate/Delayed/Minor/Deceased",
    "reason": "修改理由 (如不变则填'认可START判断')"
  }},
  "trend_analysis": {{
    "respiratory": "呼吸趋势分析",
    "cardiac": "心率趋势分析",
    "combined": "联合趋势解读"
  }},
  "history_relevance": {{
    "relevant_conditions": ["相关的既往病史"],
    "impact_on_assessment": "病史如何影响当前判断"
  }},
  "recommendations": [
    "处置建议1 (优先级最高)",
    "处置建议2",
    "处置建议3"
  ],
  "monitoring_priority": [
    "需要优先监测的指标1",
    "需要优先监测的指标2"
  ]
}}
```

只输出 JSON，不要额外解释。"#,
            patient_id = ctx.patient.patient_id,
            age = age_display(ctx.patient.age),
            gender = gender_display(&ctx.patient.gender),
            pre_existing = pre_existing_section(&ctx.patient),
            chief_complaint = chief_complaint_section(&ctx.patient),
            medications = medications_section(&ctx.patient),
            rr = format_vital(ctx.current_rr),
            hr = format_vital(ctx.current_hr),
            signal_quality = format!("{:.1}%", ctx.current_signal_quality * 100.0),
            motion_state = motion_state_text(ctx.current_motion),
            triage = ctx.current_triage,
            rr_mean = format!("{:.1}", ctx.trend_summary.rr_mean),
            rr_trend = ctx.trend_summary.rr_trend.as_str(),
            rr_change = format!("{:.1}", ctx.trend_summary.rr_change_pct),
            rr_vol = format!("{:.2}", ctx.trend_summary.rr_volatility),
            hr_mean = format!("{:.1}", ctx.trend_summary.hr_mean),
            hr_trend = ctx.trend_summary.hr_trend.as_str(),
            hr_change = format!("{:.1}", ctx.trend_summary.hr_change_pct),
            hr_vol = format!("{:.2}", ctx.trend_summary.hr_volatility),
            motion_pattern = ctx.trend_summary.motion_pattern.as_str(),
            sq_mean = format!("{:.1}%", ctx.trend_summary.signal_quality_mean * 100.0),
            alerts = alerts_section(&ctx.active_edge_alerts),
            matched_conditions = matched_conditions_section(&ctx.matched_conditions),
        );

        let est_tokens = prompt.chars().count() / 2; // Rough estimate: ~2 chars/token for Chinese

        BuiltPrompt {
            prompt,
            estimated_tokens: est_tokens,
        }
    }
}

// ── Section Builders ────────────────────────────────────────────────────────

fn age_display(age: Option<u8>) -> String {
    age.map(|a| format!("{}岁", a)).unwrap_or_else(|| "未知".into())
}

fn gender_display(gender: &Option<crate::patient_record::Gender>) -> String {
    match gender {
        Some(crate::patient_record::Gender::Male) => "男".into(),
        Some(crate::patient_record::Gender::Female) => "女".into(),
        Some(crate::patient_record::Gender::Other) => "其他".into(),
        None => "未知".into(),
    }
}

fn pre_existing_section(patient: &PatientRecord) -> String {
    if patient.pre_existing.is_empty() {
        "- 既往病史: 无".to_string()
    } else {
        format!("- 既往病史: {}", patient.pre_existing.join("、"))
    }
}

fn chief_complaint_section(patient: &PatientRecord) -> String {
    match &patient.chief_complaint {
        Some(c) => format!("- 主诉: {}", c),
        None => "- 主诉: 未记录".into(),
    }
}

fn medications_section(patient: &PatientRecord) -> String {
    if patient.medications.is_empty() {
        String::new()
    } else {
        format!("- 长期用药: {}", patient.medications.join("、"))
    }
}

fn motion_state_text(motion: f64) -> String {
    if motion > 0.6 {
        "持续活动".into()
    } else if motion > 0.3 {
        "间歇活动".into()
    } else if motion > 0.1 {
        "静息".into()
    } else {
        "静止/卧床".into()
    }
}

fn format_vital(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.1}", v))
        .unwrap_or_else(|| "无数据".into())
}

fn alerts_section(alerts: &[String]) -> String {
    if alerts.is_empty() {
        "无活跃告警".into()
    } else {
        alerts
            .iter()
            .enumerate()
            .map(|(i, a)| format!("{}. {}", i + 1, a))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn matched_conditions_section(matches: &[(MedicalCondition, f64)]) -> String {
    if matches.is_empty() {
        return "无匹配疾病模式".into();
    }

    let mut lines = Vec::new();
    for (i, (cond, score)) in matches.iter().enumerate() {
        if *score < 0.3 {
            continue;
        }
        lines.push(format!(
            "{}. {} (匹配度: {:.0}%, 紧急度: {})",
            i + 1,
            cond.name,
            score * 100.0,
            cond.urgency
        ));
        lines.push(format!("   - 关键指标: 呼吸{} 心率{}",
            cond.key_indicators.breathing_rate.as_ref()
                .map(|r| r.condition.as_str()).unwrap_or("—"),
            cond.key_indicators.heart_rate.as_ref()
                .map(|r| r.condition.as_str()).unwrap_or("—"),
        ));
        lines.push(format!("   - 风险因子: {}", cond.risk_factors.join("、")));
        lines.push(format!("   - 建议行动: {}", cond.actions.first().map(|s| s.as_str()).unwrap_or("—")));
    }

    if lines.is_empty() {
        "无高匹配度疾病模式".into()
    } else {
        lines.join("\n")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patient_record::PatientRecord;
    use crate::sliding_window::{MotionPattern, TrendDirection, VitalTrendSummary};

    #[test]
    fn test_build_prompt() {
        let patient = PatientRecord {
            patient_id: "PAT-0001".into(),
            name: Some("测试伤员".into()),
            age: Some(68),
            gender: Some(crate::patient_record::Gender::Male),
            pre_existing: vec!["COPD".into(), "高血压".into()],
            chief_complaint: Some("呼吸困难3小时".into()),
            allergies: vec![],
            medications: vec!["沙美特罗替卡松 50/250μg bid".into()],
            node_id: Some(2),
            admission_time: Some(chrono::Utc::now()),
            notes: None,
        };

        let trend = VitalTrendSummary {
            window_seconds: 300.0,
            sample_count: 60,
            rr_mean: 28.5,
            rr_min: 22.0,
            rr_max: 34.0,
            rr_trend: TrendDirection::Rising,
            rr_change_pct: 15.5,
            rr_volatility: 0.12,
            hr_mean: 105.0,
            hr_min: 95.0,
            hr_max: 118.0,
            hr_trend: TrendDirection::Rising,
            hr_change_pct: 10.0,
            hr_volatility: 0.08,
            motion_mean: 0.3,
            motion_min: 0.1,
            motion_max: 0.5,
            motion_pattern: MotionPattern::IntermittentMotion,
            signal_quality_mean: 0.85,
            signal_quality_trend: TrendDirection::Stable,
        };

        let ctx = PromptContext {
            patient,
            current_rr: Some(32.0),
            current_hr: Some(115.0),
            current_motion: 0.3,
            current_signal_quality: 0.85,
            current_triage: "Immediate".into(),
            trend_summary: trend,
            matched_conditions: vec![],
            active_edge_alerts: vec!["med_respiratory_distress".into()],
        };

        let built = PromptBuilder::build(&ctx);
        assert!(!built.prompt.is_empty());
        assert!(built.estimated_tokens > 0);
        assert!(built.prompt.contains("PAT-0001"));
        assert!(built.prompt.contains("COPD"));
        assert!(built.prompt.contains("Immediate"));
        assert!(built.prompt.contains("med_respiratory_distress"));
        println!("Prompt ({} est. tokens):\n{}", built.estimated_tokens, built.prompt);
    }
}
