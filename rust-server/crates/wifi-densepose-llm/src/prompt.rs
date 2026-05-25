//! Prompt Compiler — builds system/context/task prompts for LLM analysis.
//!
//! Three-segment prompt: system message + compact context JSON + task template.
//! CompactContext uses Chinese key names to save tokens.

use crate::types::{AnalysisRoute, Prompt, RouteDecision, StructuredContext};
use serde::Serialize;

// ── Prompt Compiler ──────────────────────────────────────────────────────────

pub struct PromptCompiler {
    system_template: String,
    deep_task_template: String,
    brief_task_template: String,
}

impl PromptCompiler {
    pub fn new(
        system_template: String,
        deep_task_template: String,
        brief_task_template: String,
    ) -> Self {
        Self {
            system_template,
            deep_task_template,
            brief_task_template,
        }
    }

    /// Load templates from the data/prompts/ directory.
    pub fn from_dir(dir: &str) -> Result<Self, std::io::Error> {
        let system = std::fs::read_to_string(format!("{}/system.txt", dir))
            .unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
        let deep = std::fs::read_to_string(format!("{}/deep_analysis.txt", dir))
            .unwrap_or_else(|_| DEFAULT_DEEP_TASK.to_string());
        let brief = std::fs::read_to_string(format!("{}/brief_analysis.txt", dir))
            .unwrap_or_else(|_| DEFAULT_BRIEF_TASK.to_string());
        Ok(Self::new(system, deep, brief))
    }

    pub fn compile(&self, ctx: &StructuredContext, route: &RouteDecision) -> Prompt {
        let system = self.system_template.clone();
        let context = self.serialize_context(ctx);
        let task = match route.route {
            AnalysisRoute::DeepLLM => self.deep_task(ctx),
            AnalysisRoute::BriefLLM => self.brief_task(ctx),
            _ => String::new(),
        };
        let estimated_input = Self::estimate_tokens(&system)
            + Self::estimate_tokens(&context)
            + Self::estimate_tokens(&task);

        Prompt {
            system,
            context,
            task,
            estimated_input_tokens: estimated_input,
        }
    }

    fn serialize_context(&self, ctx: &StructuredContext) -> String {
        let compact = CompactContext::from(ctx);
        serde_json::to_string(&compact).unwrap_or_default()
    }

    fn deep_task(&self, ctx: &StructuredContext) -> String {
        self.deep_task_template
            .replace("{triage}", &ctx.triage_current)
            .replace("{alerts}", &ctx.recent_alerts.join(", "))
    }

    fn brief_task(&self, ctx: &StructuredContext) -> String {
        self.brief_task_template
            .replace("{triage}", &ctx.triage_current)
    }

    fn estimate_tokens(text: &str) -> u16 {
        // Rough: Chinese chars ~1 token, English words ~1.3 tokens per word
        (text.chars().count() as f32 * 0.65) as u16
    }
}

impl Default for PromptCompiler {
    fn default() -> Self {
        Self {
            system_template: DEFAULT_SYSTEM.to_string(),
            deep_task_template: DEFAULT_DEEP_TASK.to_string(),
            brief_task_template: DEFAULT_BRIEF_TASK.to_string(),
        }
    }
}

// ── Compact Context (for LLM) ───────────────────────────────────────────────

#[derive(Serialize)]
struct CompactVital {
    #[serde(rename = "值")]
    value: f32,
    #[serde(rename = "置信度")]
    confidence: f32,
}

#[derive(Serialize)]
struct CompactTrends {
    #[serde(rename = "1分钟")]
    t1m: String,
    #[serde(rename = "5分钟")]
    t5m: String,
}

#[derive(Serialize)]
struct CompactHistory {
    #[serde(rename = "既往病史")]
    conditions: Vec<String>,
    #[serde(rename = "年龄段")]
    age: Option<String>,
}

#[derive(Serialize)]
struct CompactKbMatch {
    #[serde(rename = "疾病")]
    condition: String,
    #[serde(rename = "匹配度")]
    score: String,
    #[serde(rename = "风险因素")]
    risks: Vec<String>,
}

#[derive(Serialize)]
struct CompactContext {
    #[serde(rename = "心率")]
    hr: Option<CompactVital>,
    #[serde(rename = "呼吸")]
    rr: Option<CompactVital>,
    #[serde(rename = "运动状态")]
    motion: Option<String>,
    #[serde(rename = "信号质量")]
    signal_quality: f32,
    #[serde(rename = "分诊等级")]
    triage: String,
    #[serde(rename = "趋势")]
    trends: CompactTrends,
    #[serde(rename = "病史")]
    history: Option<CompactHistory>,
    #[serde(rename = "疾病匹配")]
    kb_matches: Vec<CompactKbMatch>,
    #[serde(rename = "近期告警")]
    alerts: Vec<String>,
}

impl From<&StructuredContext> for CompactContext {
    fn from(ctx: &StructuredContext) -> Self {
        CompactContext {
            hr: ctx.vitals_current.heart_rate_bpm.map(|v| CompactVital {
                value: v,
                confidence: ctx.vitals_current.heartbeat_confidence,
            }),
            rr: ctx.vitals_current.breathing_rate_bpm.map(|v| CompactVital {
                value: v,
                confidence: ctx.vitals_current.breathing_confidence,
            }),
            motion: ctx.vitals_current.motion_class.clone(),
            signal_quality: ctx.vitals_current.signal_quality,
            triage: ctx.triage_current.clone(),
            trends: CompactTrends {
                t1m: format!("{:?}", ctx.vitals_trend_1min.direction),
                t5m: format!("{:?}", ctx.vitals_trend_5min.direction),
            },
            history: ctx.patient_history.as_ref().map(|h| CompactHistory {
                conditions: h.prior_conditions.clone(),
                age: h.age_estimate.clone(),
            }),
            kb_matches: ctx
                .kb_matches
                .iter()
                .map(|m| CompactKbMatch {
                    condition: m.condition.clone(),
                    score: format!("{:.0}%", m.match_score * 100.0),
                    risks: m.risk_factors.clone(),
                })
                .collect(),
            alerts: ctx.recent_alerts.clone(),
        }
    }
}

// ── Default Templates ────────────────────────────────────────────────────────

const DEFAULT_SYSTEM: &str = r#"你是方舱医院急救医疗助手。你的职责是基于提供的体征数据进行分析。
约束:
1. 只输出临床发现、监测建议、鉴别诊断
2. 不输出药物名称、剂量、手术方案
3. 无法确定时, 明确说明"建议由医生评估"
4. 输出格式: markdown, 分节
5. 末尾加一行 [AI分析, 仅供参考]"#;

const DEFAULT_DEEP_TASK: &str = r#"请基于以上体征数据和疾病匹配结果进行深度分析:

1. 主要临床发现
   - 概述最关键的异常指标及其临床意义
   - 结合既往病史分析风险

2. 疾病特征分析
   - 评估匹配到的疾病特征的可能性
   - 列出需要排除的鉴别诊断 (最多3项)

3. 分诊建议 (第二意见)
   - 当前分诊: {triage}
   - 请用此格式标注: [分诊建议: 升级/维持/降级, 置信度: 0-100%]
   - 简述理由

4. 监测要点
   - 具体指标和推荐监测频率
   - 需要特别关注的危险信号"#;

const DEFAULT_BRIEF_TASK: &str = r#"请基于以上体征数据进行简要分析:
1. 主要发现 (1-2句)
2. 分诊建议 [分诊建议: 升级/维持/降级, 置信度: 0-100%]
3. 监测要点 (1-2条)"#;
