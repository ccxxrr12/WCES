# 端侧 Agent 开发计划

> 目标：`wifi-densepose-llm` 重构 — 本地 Candle 推理 → Coordinator + 远端 LLM API
> 硬件：RZ/G2L（2×A55 + M33, 1GB DDR4）
> 前置：主控已从 RZ/V2H 切换为 RZ/G2L

---

## 零、开发前的阅读清单

动手前先读这些文件，理解现有数据结构和调用链：

```
rust-server/crates/
├── wifi-densepose-llm/src/
│   ├── lib.rs           ← 入口, feature flags, 当前 llm/template-only 切换逻辑
│   ├── types.rs         ← LlmGenerationResult, StreamToken, 现有类型
│   ├── streaming.rs     ← 当前 Candle 推理代码, 要被替换
│   └── template.rs      ← 模板分析引擎, 保留并增强
├── wifi-densepose-mat/src/
│   └── mat_pipeline.rs  ← TriageEngine::process(), 了解 TriageUpdate 结构
├── wifi-densepose-sensing-server/src/
│   ├── tasks/udp_receiver.rs    ← 看 TriageEngine 怎么被调用的
│   ├── handlers/llm.rs          ← 当前 LLM 分析 HTTP 端点
│   └── handlers/routes.rs       ← 路由注册
└── wifi-densepose-core/src/
    └── types.rs         ← VitalSigns, EdgeAlert 等基础类型
```

---

## 一、需求清单 (6 项, 全部落地的方案)

| # | 需求 | 实现位置 | 怎么做的 |
|---|------|---------|---------|
| 1 | 既往病史 + 当前病史 | `context.rs` | Context Collator 查 PatientRecordDB (已有 sled) |
| 2 | 多体征联合推断 | `prompt.rs` | StructuredContext 塞 5 维数据进 prompt, LLM 做推断 |
| 3 | 疾病特征匹配 | `medical_kb.rs` | 本地规则引擎做 VitalPattern 模式匹配, 结果注进 prompt |
| 4 | 趋势感知 | `context.rs` | 三窗口 TrendSummary, O(n) 滑动窗口, <1ms |
| 5 | 优先级细化 | `risk_adjust.rs` | LLM 输出 RiskAdjustment, 反馈到 UI 分诊卡片显示第二意见 |
| 6 | 处置建议生成 | `prompt.rs` + `validator.rs` | Prompt 约束输出范围 + Validator 拦截危险内容 |

---

## 二、数据流 (一次完整分析)

```
触发事件 (恶化 / 新伤员 / 手动 / 巡检)
  │
  ▼
Context Collator (context.rs)
  │
  ├──→ 查 PatientRecordDB ───→ 既往病史         ← 需求1
  ├──→ 读 VitalSigns 环形缓冲 → 30帧历史 + 趋势  ← 需求4
  ├──→ 读 TriageEngine 状态  → 分诊轨迹
  ├──→ 读 EdgeModule 输出    → 最近10条告警
  └──→ MedicalKB.search()   → 疾病特征匹配结果  ← 需求3
  │
  ▼
StructuredContext (完整, 一次性构建)
  │
  ▼
Router (router.rs) → 需要 LLM? 走哪条通道? 多少 token 预算?
  │
  ├── LLM 通道 ──→ Prompt Compiler ──→ API Gateway ──→ Validator
  │                                              │
  │                                              ▼
  │                                     RiskAdjustment ← 需求5
  │                                              │
  └── 模板通道 ──→ Template Engine                 │
                                                   ▼
                                          AnalysisResult {
                                            text,          // 分析文本
                                            risk_adjust,   // 第二意见
                                            source,        // LLM / Template / Cache
                                            degrade_level, // 当前降级等级
                                          }
                                                   │
                                                   ▼
                                          WebSocket → triage.html
```

---

## 三、文件变更清单

```
rust-server/crates/wifi-densepose-llm/
├── Cargo.toml            # [改] 删 candle/tokenizers/hf-hub, 加 reqwest/tokio-stream/backoff
├── src/
│   ├── lib.rs            # [改] feature llm→agent, 重导出新模块
│   ├── types.rs          # [改] 删 LLM 推理类型, 加 Agent 类型
│   ├── streaming.rs      # [删] 整个文件, 功能移到 gateway.rs
│   │
│   ├── context.rs        # [新] Context Collator
│   ├── medical_kb.rs     # [新] MedicalKB + VitalPattern 模式匹配
│   ├── router.rs         # [新] Route Decision 规则引擎
│   ├── prompt.rs         # [新] 三段式 Prompt Compiler
│   ├── gateway.rs        # [新] API Gateway + Circuit Breaker
│   ├── validator.rs      # [新] Output Validator
│   ├── degrade.rs        # [新] Degradation Ladder
│   ├── risk_adjust.rs    # [新] RiskAdjustment 生成
│   └── template.rs       # [留] 保留, 增强 KB 注入能力

data/
├── medical_knowledge.json  # [新] 医学知识库, ~50KB
└── prompts/
    ├── system.txt          # [新] System prompt 模板
    ├── deep_analysis.txt   # [新] DeepLLM task 模板
    └── brief_analysis.txt  # [新] BriefLLM task 模板
```

### Cargo.toml 变更

```toml
# 删除
# candle-core = "0.8"
# candle-transformers = "0.8"
# tokenizers = "0.21"
# hf-hub = "0.4"

# 新增
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
tokio-stream = "0.1"
backoff = { version = "0.4", features = ["tokio"] }
regex = "1"          # validator 用
serde_json = "1"     # 已有, 不用改

[features]
default = ["agent"]
agent = ["reqwest", "tokio-stream", "backoff", "regex"]
template-only = []   # 零外部依赖, 降级用
```

---

## 四、各模块实现要点

### 4.0 `types.rs` — 所有新类型集中定义

```rust
// ── 触发源 ──
pub enum TriggerSource {
    Deterioration { patient_id: u32, from: TriageLevel, to: TriageLevel },
    NewPatient { patient_id: u32 },
    ManualRequest { patient_id: u32 },
    PeriodicScan,
}

// ── 趋势 ──
pub struct TrendSummary {
    pub direction: TrendDirection,   // Rising / Falling / Stable / InsufficientData
    pub delta: f32,
    pub delta_pct: f32,
    pub anomaly_score: f32,          // 0.0-10.0, 规则引擎打分
    pub data_points: u16,            // 有效数据点数
}

pub enum TrendDirection { Rising, Falling, Stable, InsufficientData }

// ── 上下文 ──
pub struct StructuredContext {
    pub patient_id: u32,
    pub node_id: u8,
    // 当前体征
    pub vitals_current: VitalSnapshot,
    pub vitals_trend_1min: TrendSummary,
    pub vitals_trend_5min: TrendSummary,
    // 分诊
    pub triage_current: TriageLevel,
    pub triage_trajectory: Vec<TriageStep>,
    // 病史 (需求1)
    pub patient_history: Option<PatientHistory>,
    // 边缘模块告警
    pub recent_alerts: Vec<EdgeAlert>,
    // KB 匹配 (需求3)
    pub kb_matches: Vec<KbMatchResult>,
    // 元信息
    pub triggered_by: TriggerSource,
    pub built_at_ms: u64,
}

pub struct VitalSnapshot {
    pub breathing_rate_bpm: Option<f32>,
    pub heart_rate_bpm: Option<f32>,
    pub breathing_confidence: f32,
    pub heartbeat_confidence: f32,
    pub signal_quality: f32,
    pub motion_class: Option<String>,      // "active" | "present_still" | "idle"
    pub person_count_estimate: Option<u8>,
    pub rssi: Option<i16>,
}

pub struct TriageStep {
    pub level: TriageLevel,
    pub timestamp_ms: u64,
}

pub struct PatientHistory {
    pub record_id: String,
    pub age_estimate: Option<String>,      // "Infant" | "Child" | "Adult" | "Elderly"
    pub prior_conditions: Vec<String>,     // 既往病史标签
    pub total_tracking_duration_secs: u64,
    pub triage_level_changes: u16,
    pub prior_llm_analyses: Vec<String>,   // 最近分析摘要, 最多3条
}

// ── KB 匹配 ──
pub struct KbMatchResult {
    pub entry_id: String,
    pub condition: String,
    pub match_score: f32,     // 0.0-1.0, 匹配度
    pub matched_conditions: Vec<String>,  // 满足的条件描述, e.g. "心率>100", "呼吸>20"
    pub risk_factors: Vec<String>,
    pub triage_implication: String,
    pub monitoring_notes: String,
}

// ── 路由 ──
pub enum AnalysisRoute {
    DeepLLM,          // 完整分析, max 300 output tokens
    BriefLLM,         // 简要分析, max 150 output tokens  
    TemplateWithKB,   // 模板 + 本地KB注入
    TemplateOnly,     // 纯模板
    CachedReplay,     // 复用缓存
    Skip,             // 跳过
}

pub struct RouteDecision {
    pub route: AnalysisRoute,
    pub reason: String,       // 为什么走这个通道, 调试/日志用
    pub max_output_tokens: u16,
    pub priority: u8,
}

// ── Prompt ──
pub struct Prompt {
    pub system: String,
    pub context: String,    // 序列化后的 StructuredContext JSON
    pub task: String,
    pub estimated_input_tokens: u16,
}

// ── 分析结果 ──
pub struct AnalysisResult {
    pub patient_id: u32,
    pub text: String,                          // 分析文本 (markdown)
    pub risk_adjustment: Option<RiskAdjustment>, // 需求5
    pub source: AnalysisSource,
    pub degrade_level: DegradationLevel,
    pub generated_at_ms: u64,
}

pub struct RiskAdjustment {
    pub direction: AdjustDirection,  // Escalate / Maintain / Deescalate
    pub confidence: f32,             // 0.0-1.0
    pub reason_short: String,        // 一句话原因, 展示在 UI
    pub detail: String,              // 详细说明
}

pub enum AdjustDirection { Escalate, Maintain, Deescalate }
pub enum AnalysisSource { LLM, Template, Cache }

// ── 降级 ──
pub enum DegradationLevel {
    L0_FullLLM, L1_BriefLLM, L2_TemplateWithKB, L3_TemplateOnly, L4_CachedReplay,
}
```

### 4.1 `context.rs` — Context Collator

**接入点**: 从 sensing-server 的 AppState 读取数据。

```rust
pub struct ContextCollator;

impl ContextCollator {
    /// 主入口: 给定 patient_id, 构建完整 StructuredContext
    /// 耗时: <2ms (纯内存读取)
    pub async fn build(
        patient_id: u32,
        trigger: TriggerSource,
        state: &AppState,          // sensing-server 的全局状态
    ) -> Result<StructuredContext, CollatorError> {
        let inner = state.inner.read().await;  // RwLock read

        // 1. 查 MAT 管线中的伤员数据
        let survivor = inner.triage_engine.get_survivor(patient_id)?;

        // 2. 构建 VitalSnapshot
        let vitals = survivor.latest_vitals();
        let vitals_current = VitalSnapshot { ... };

        // 3. 趋势计算 (O(n) 扫描帧历史)
        let vitals_trend_1min = self.compute_trend(&survivor.vital_history, 60);
        let vitals_trend_5min = self.compute_trend(&survivor.vital_history, 300);

        // 4. 分诊轨迹
        let triage_trajectory = survivor.triage_changes().collect();

        // 5. 拉取既往病史 ← 需求1
        let patient_history = self.load_history(patient_id, &inner.patient_db)?;
        // patient_db: Option<sled::Db>, 已在 AppState 中

        // 6. 拉取边缘模块告警 (最近10条)
        let recent_alerts = inner.edge_module_engine.recent_alerts(patient_id, 10);

        // 7. KB 匹配 ← 需求3
        let kb_matches = inner.medical_kb.match_vitals(&vitals_current);

        // 注意: RwLock 在这里释放, 后续 prompt/LLM 调用不持锁

        Ok(StructuredContext { ... })
    }

    fn compute_trend(&self, history: &VitalHistory, window_secs: u64) -> TrendSummary;
    fn load_history(&self, patient_id: u32, db: &Option<sled::Db>) -> Result<Option<PatientHistory>, CollatorError>;
}
```

**注意锁**: `state.inner.read()` 只需要在 Context Collator 这一步持锁，构建完 StructuredContext 后立即释放。后续 prompt 编译、LLM 调用都不需要锁。

### 4.2 `medical_kb.rs` — 医学知识库 + 模式匹配

```rust
pub struct MedicalKB {
    entries: Vec<MedicalKBEntry>,
    pub(crate) loaded_at: Instant,
}

struct MedicalKBEntry {
    id: String,
    condition: String,            // "心动过速"
    vital_pattern: VitalPattern,  // 体征匹配规则
    risk_factors: Vec<String>,
    triage_implication: String,
    monitoring_notes: String,
    differential_diagnosis: Vec<String>,   // 鉴别诊断
}

struct VitalPattern {
    // 每个字段: (min, max), None 表示不约束
    hr_range: Option<(f32, f32)>,
    rr_range: Option<(f32, f32)>,
    hr_trend: Option<TrendDirection>,
    rr_trend: Option<TrendDirection>,
    motion_states: Vec<String>,     // 匹配任一
    edge_alert_keywords: Vec<String>,  // 匹配任一
    // 计算匹配分: 满足的条件数 / 总条件数
    // 必须满足的才算 total_conditions
}

impl MedicalKB {
    /// 从 JSON 文件加载, 启动时调用一次
    pub fn load(path: &str) -> Result<Self, KbError>;
    /// 重新加载 (SIGHUP 或 API 触发), 不重启服务
    pub fn reload(&mut self, path: &str) -> Result<(), KbError>;

    /// 匹配体征模式, 返回匹配度 >= 0.4 的条目, 按分数降序, 最多 3 条
    /// 耗时: 20条目 × 6条件 = 120次比较, <0.1ms
    pub fn match_vitals(&self, vitals: &VitalSnapshot) -> Vec<KbMatchResult> {
        self.entries.iter()
            .filter_map(|entry| {
                let (score, matched) = entry.vital_pattern.score(vitals);
                if score >= 0.4 {
                    Some(KbMatchResult {
                        entry_id: entry.id.clone(),
                        condition: entry.condition.clone(),
                        match_score: score,
                        matched_conditions: matched,
                        risk_factors: entry.risk_factors.clone(),
                        triage_implication: entry.triage_implication.clone(),
                        monitoring_notes: entry.monitoring_notes.clone(),
                    })
                } else { None }
            })
            .sorted_by(|a, b| b.match_score.partial_cmp(&a.match_score).unwrap())
            .take(3)
            .collect()
    }
}

impl VitalPattern {
    fn score(&self, v: &VitalSnapshot) -> (f32, Vec<String>) {
        let mut conditions = 0u8;
        let mut total = 0u8;
        let mut matched = Vec::new();

        // hr_range
        if let Some((lo, hi)) = self.hr_range {
            total += 1;
            if let Some(hr) = v.heart_rate_bpm {
                if hr >= lo && hr <= hi { conditions += 1; matched.push(format!("心率 {:.0} ∈ [{},{}]", hr, lo, hi)); }
            }
        }
        // rr_range, hr_trend, rr_trend, motion_states, edge_alert_keywords 同理
        // ...

        let score = if total == 0 { 0.0 } else { conditions as f32 / total as f32 };
        (score, matched)
    }
}
```

**KB JSON 示例** (`data/medical_knowledge.json`):

```json
{
  "entries": [
    {
      "id": "tachycardia_001",
      "condition": "心动过速",
      "vital_pattern": {
        "hr_range": [100, 180],
        "rr_range": null,
        "hr_trend": "Rising",
        "rr_trend": null,
        "motion_states": ["present_still"],
        "edge_alert_keywords": ["arrhythmia", "cardiac"]
      },
      "risk_factors": ["失血性休克早期", "感染性发热", "心源性休克", "疼痛应激"],
      "triage_implication": "心率>120且持续上升 → 考虑升级为Immediate",
      "monitoring_notes": "密切监测心率趋势, 每5分钟记录一次, 关注血压变化",
      "differential_diagnosis": ["低血容量性休克", "脓毒症早期", "心律失常"]
    }
  ]
}
```

初始需要写 15-20 个条目，覆盖心血管/呼吸/休克/创伤/神经/其他六个类别。

### 4.3 `router.rs` — 路由决策

```rust
pub struct AnalysisRouter;

impl AnalysisRouter {
    /// 纯函数: 输入状态, 输出路由决策
    /// 耗时: <0.1ms (查表)
    pub fn decide(
        triage: TriageLevel,
        is_deteriorating: bool,
        network_reachable: bool,
        in_cooldown: bool,       // 同伤员5分钟内已分析过
    ) -> RouteDecision {
        use TriageLevel::*;
        use AnalysisRoute::*;

        if in_cooldown {
            return RouteDecision { route: CachedReplay, reason: "冷却期内,复用缓存", max_output_tokens: 0, priority: 0 };
        }

        let route = match (triage, is_deteriorating, network_reachable) {
            (Deceased, _, _) | (Unknown, _, _) => Skip,
            (Immediate, true, true)  => DeepLLM,
            (Immediate, false, true) => BriefLLM,
            (Delayed, true, true)    => DeepLLM,
            (Delayed, false, true)   => BriefLLM,
            (Minor, _, true)         => TemplateWithKB,
            (_, _, false)            => TemplateWithKB,
        };

        let max_output_tokens = match route {
            DeepLLM => 300,
            BriefLLM => 150,
            _ => 0,
        };

        RouteDecision { route, reason: format!("{:?}|恶化={}|有网={}", triage, is_deteriorating, network_reachable), max_output_tokens, priority: route_priority(&route) }
    }
}
```

**冷却期判断逻辑**: 在 Agent 主循环里维护 `HashMap<u32, Instant>` (patient_id → 上次分析时间), 5 分钟内不重复调 LLM。

### 4.4 `prompt.rs` — Prompt 编译器

```rust
pub struct PromptCompiler {
    system_template: String,
    deep_task_template: String,
    brief_task_template: String,
}

impl PromptCompiler {
    pub fn new(config: &PromptConfig) -> Self;

    pub fn compile(&self, ctx: &StructuredContext, route: &RouteDecision) -> Prompt {
        let system = self.system_template.clone();
        let context = self.serialize_context(ctx);  // → 精简 JSON, ~200 tokens
        let task = match route.route {
            AnalysisRoute::DeepLLM  => self.deep_task(ctx),
            AnalysisRoute::BriefLLM => self.brief_task(ctx),
            _ => String::new(),
        };
        let estimated_input = Self::estimate_tokens(&system) + Self::estimate_tokens(&context) + Self::estimate_tokens(&task);
        Prompt { system, context, task, estimated_input_tokens: estimated_input }
    }

    /// 序列化上下文: 只输出 LLM 需要的字段, 中文 key 节省 token
    fn serialize_context(&self, ctx: &StructuredContext) -> String {
        // 注意: JSON key 用中文缩写节省 token, 例如:
        // "心率": 72, "呼吸": 16, "分诊": "Delayed"
        // 不要输出冗余字段 (patient_id 只在日志用, 不传 LLM)
        serde_json::to_string(&CompactContext::from(ctx)).unwrap_or_default()
    }

    fn deep_task(&self, ctx: &StructuredContext) -> String;
    fn brief_task(&self, ctx: &StructuredContext) -> String;
}

// 传给 LLM 的精简版上下文, 不是完整 StructuredContext
#[derive(Serialize)]
struct CompactContext {
    #[serde(rename = "心率")]
    hr: Option<SerdeVital>,
    #[serde(rename = "呼吸")]
    rr: Option<SerdeVital>,
    #[serde(rename = "运动")]
    motion: Option<String>,
    #[serde(rename = "分诊")]
    triage: String,
    #[serde(rename = "趋势")]
    trends: CompactTrends,
    #[serde(rename = "病史")]
    history: Option<CompactHistory>,
    #[serde(rename = "疾病匹配")]
    kb_matches: Vec<CompactKbMatch>,
    #[serde(rename = "告警")]
    alerts: Vec<String>,
}
```

**System Prompt 模板** (`data/prompts/system.txt`):

```
你是方舱医院急救医疗助手。你的职责是基于提供的体征数据进行分析。
约束:
1. 只输出临床发现、监测建议、鉴别诊断
2. 不输出药物名称、剂量、手术方案
3. 无法确定时, 明确说明"建议由医生评估"
4. 输出格式: markdown, 分节
5. 末尾加一行 [AI分析, 仅供参考]
```

**Deep task 模板** (`data/prompts/deep_analysis.txt`):

```
请基于以上体征数据和疾病匹配结果进行深度分析:

1. 主要临床发现
   - 概述最关键的异常指标及其临床意义
   - 结合既往病史分析风险

2. 疾病特征分析
   - 评估匹配到的疾病特征的可能性
   - 列出需要排除的鉴别诊断 (最多3项)

3. 分诊建议 (第二意见)
   - 在当前 START 分诊基础上, 是否建议调整监测优先级
   - 请用此格式标注: [分诊建议: 升级/维持/降级, 置信度: 0-100%]
   - 简述理由

4. 监测要点
   - 具体指标和推荐监测频率
   - 需要特别关注的危险信号
```

**Brief task 模板** (`data/prompts/brief_analysis.txt`):

```
请基于以上体征数据进行简要分析:
1. 主要发现 (1-2句)
2. 分诊建议 [分诊建议: 升级/维持/降级, 置信度: 0-100%]
3. 监测要点 (1-2条)
```

### 4.5 `gateway.rs` — API 网关

```rust
pub struct LlmGateway {
    client: reqwest::Client,
    circuit_breaker: Arc<Mutex<CircuitBreaker>>,
    config: GatewayConfig,
}

impl LlmGateway {
    pub fn new(config: GatewayConfig) -> Self;

    /// 流式分析: POST → SSE → tokio Stream
    /// 调用方通过 StreamExt 逐 token 读取, 转发到 WebSocket
    pub async fn stream(
        &self,
        prompt: &Prompt,
        max_tokens: u16,
    ) -> Result<impl Stream<Item = Result<String, StreamError>>, GatewayError> {
        // 1. circuit breaker 检查
        self.circuit_breaker.lock().await.check()?;

        // 2. 构造 OpenAI 兼容请求体
        let body = json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": &prompt.system},
                {"role": "user", "content": format!("{}\n\n{}", prompt.context, prompt.task)}
            ],
            "max_tokens": max_tokens,
            "temperature": 0.3,
            "stream": true
        });

        // 3. POST, 设置 timeout
        let resp = self.client.post(&self.config.endpoint)
            .json(&body)
            .timeout(self.config.timeout)
            .send().await?;

        // 4. 流式解析 SSE: data: {"choices":[{"delta":{"content":"..."}}]}
        let stream = resp.bytes_stream()
            .map(|chunk| -> Result<String, StreamError> {
                // 解析 SSE line → extract delta.content
            });

        Ok(stream)
    }

    /// 非流式 (用于健康检查)
    pub async fn ping(&self) -> bool;
}

// 熔断器 (独立, 可单元测试)
pub struct CircuitBreaker {
    state: CircuitState,
    failures: u8,
    last_failure_time: Option<Instant>,
    opened_at: Option<Instant>,
    config: BreakerConfig,
}

enum CircuitState { Closed, Open, HalfOpen }

impl CircuitBreaker {
    pub fn check(&mut self) -> Result<(), BreakerError>;   // Open 时直接拒绝
    pub fn on_success(&mut self);                           // 恢复计数
    pub fn on_failure(&mut self);                           // 累计失败, 触发熔断
}

struct BreakerConfig {
    failure_threshold: u8,     // 3
    open_duration_secs: u64,   // 300 (5分钟)
    half_open_max_requests: u8, // 1
}
```

**Provider 配置** (环境变量):

```bash
# 通过环境变量注入, 不写入代码
LLM_ENDPOINT="https://api.openai.com/v1/chat/completions"
LLM_MODEL="gpt-4o-mini"   # 性价比首选
LLM_API_KEY="sk-xxx"
LLM_TIMEOUT_SECS=20
```

### 4.6 `validator.rs` — 输出校验

```rust
pub struct OutputValidator {
    blocked: Vec<Regex>,       // 编译时构建
    triage_contradiction: TriageValidator,  // 独立子模块
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
            triage_contradiction: TriageValidator::new(),
        }
    }

    /// 校验 LLM 输出
    /// 耗时: <1ms (纯正则 + 字符串操作)
    pub fn validate(
        &self,
        output: &str,
        current_triage: TriageLevel,
    ) -> ValidationResult {
        // 1. 空响应
        if output.trim().is_empty() {
            return ValidationResult::FailAndFallback(vec!["LLM返回空响应".into()]);
        }

        // 2. 危险内容拦截
        let mut cleaned = output.to_string();
        let mut warnings = Vec::new();
        for re in &self.blocked {
            if re.is_match(output) {
                cleaned = re.replace_all(&cleaned, "[内容已拦截: 超出AI助手权限]").to_string();
                warnings.push(format!("拦截: {}", re.as_str()));
            }
        }

        // 3. 分诊矛盾检测 ← 需求5 关联: 如果 LLM 建议升级但没给置信度
        if let Some(w) = self.triage_contradiction.check(output, current_triage) {
            warnings.push(w);
        }

        // 4. 追加免责声明 (如果没有)
        if !output.contains("[AI分析, 仅供参考]") {
            cleaned.push_str("\n\n---\n[AI分析, 仅供参考]");
        }

        if warnings.is_empty() {
            ValidationResult::Pass(cleaned)
        } else {
            ValidationResult::PassWithWarning(cleaned, warnings)
        }
    }
}

pub enum ValidationResult {
    Pass(String),
    PassWithWarning(String, Vec<String>),
    FailAndFallback(Vec<String>),
}
```

### 4.7 `risk_adjust.rs` — 风险调整 (需求5)

```rust
/// 从 LLM 输出中提取 RiskAdjustment
/// 依赖 prompt 中约定的格式: [分诊建议: 升级/维持/降级, 置信度: 0-100%]
pub struct RiskAdjustmentExtractor;

impl RiskAdjustmentExtractor {
    /// 从分析文本中解析分诊建议
    /// 正则匹配: \[分诊建议:\s*(升级|维持|降级),\s*置信度:\s*(\d+)%\]
    pub fn extract(text: &str) -> Option<RiskAdjustment> {
        let re = Regex::new(
            r"\[分诊建议:\s*(升级|维持|降级),\s*置信度:\s*(\d+)%\]"
        ).unwrap();

        let caps = re.captures(text)?;
        let direction = match &caps[1] {
            "升级" => AdjustDirection::Escalate,
            "维持" => AdjustDirection::Maintain,
            "降级" => AdjustDirection::Deescalate,
            _ => return None,
        };
        let confidence = caps[2].parse::<f32>().ok()? / 100.0;

        Some(RiskAdjustment {
            direction,
            confidence,
            reason_short: Self::extract_reason(text),
            detail: text.lines().take(3).collect::<Vec<_>>().join("\n"),
        })
    }

    fn extract_reason(text: &str) -> String;
}

// NOTE: RiskAdjustment 只是第二意见, 不改变 START 分诊
// 它在 triage.html 的分诊卡片上显示为:
//   "Immediate (START) ↑ LLM建议升级 [78%]"
// 不会自动触发分诊流程变更
```

### 4.8 `degrade.rs` — 降级状态机

```rust
pub struct DegradationManager {
    state: DegradationState,
    cooldowns: HashMap<u32, Instant>,   // patient_id → 上次分析时间
    analysis_cache: LruCache<u32, AnalysisResult>,  // patient_id → 最近分析
}

impl DegradationManager {
    /// 每次分析请求前调用
    pub fn assess(&mut self, patient_id: u32) -> DegradationLevel {
        // 1. 检查缓存
        if let Some(last) = self.cooldowns.get(&patient_id) {
            if last.elapsed() < Duration::from_secs(300) {
                // 如果有缓存结果, 返回 L4
                if self.analysis_cache.contains(&patient_id) {
                    return DegradationLevel::L4_CachedReplay;
                }
            }
        }
        // 2. 检查网络
        if !self.state.network_reachable {
            return DegradationLevel::L2_TemplateWithKB;
        }
        // 3. 检查熔断
        if self.state.circuit_breaker_open {
            return DegradationLevel::L2_TemplateWithKB;
        }
        // 4. 正常, 让 Router 决定 L0/L1
        DegradationLevel::L0_FullLLM
    }

    pub fn on_analysis_complete(&mut self, patient_id: u32, result: AnalysisResult) {
        self.cooldowns.insert(patient_id, Instant::now());
        self.analysis_cache.put(patient_id, result);
    }

    pub fn on_network_change(&mut self, reachable: bool);
    pub fn on_circuit_breaker_change(&mut self, open: bool);
}

struct DegradationState {
    network_reachable: bool,
    circuit_breaker_open: bool,
    consecutive_failures: u8,
}
```

### 4.9 `template.rs` — 模板引擎 (保留, 增强)

原有 `template-only` 代码保留。增强点：注入 KB 匹配结果。

```rust
// 增强: template 模式也能利用 KB
pub fn generate_with_kb(ctx: &StructuredContext) -> AnalysisResult {
    // 如果 ctx.kb_matches 非空, 在模板中插入疾病知识
    // 否则走原有纯模板逻辑
}
```

---

## 五、Agent 主循环 (lib.rs 中实现)

所有模块的编排入口：

```rust
pub struct MedicalAgent {
    context_collator: ContextCollator,
    router: AnalysisRouter,
    prompt_compiler: PromptCompiler,
    gateway: LlmGateway,
    validator: OutputValidator,
    degradation: DegradationManager,
    risk_extractor: RiskAdjustmentExtractor,
    template_engine: TemplateEngine,
}

impl MedicalAgent {
    /// 主入口: sensing-server 在事件发生时调用
    /// 返回 AnalysisResult, 调用方负责通过 WebSocket 推送
    pub async fn analyze(
        &mut self,
        patient_id: u32,
        trigger: TriggerSource,
        state: &AppState,
    ) -> AnalysisResult {
        // Step 1: 降级评估
        let degrade_level = self.degradation.assess(patient_id);
        if degrade_level == DegradationLevel::L4_CachedReplay {
            return self.degradation.analysis_cache.get(&patient_id).unwrap().clone();
        }

        // Step 2: 构建上下文 (持锁 <2ms)
        let ctx = match self.context_collator.build(patient_id, trigger.clone(), state).await {
            Ok(c) => c,
            Err(e) => return self.template_engine.generate_fallback(patient_id, &format!("上下文构建失败: {}", e)),
        };

        // Step 3: 路由决策
        let network_ok = self.gateway.ping().await;
        let in_cooldown = self.degradation.cooldowns.contains_key(&patient_id);
        let route = self.router.decide(
            ctx.triage_current,
            matches!(trigger, TriggerSource::Deterioration { .. }),
            network_ok,
            in_cooldown,
        );

        // Step 4: 执行分析
        match route.route {
            AnalysisRoute::DeepLLM | AnalysisRoute::BriefLLM => {
                self.analyze_via_llm(ctx, route, degrade_level).await
            }
            AnalysisRoute::TemplateWithKB => {
                self.template_engine.generate_with_kb(&ctx)
            }
            AnalysisRoute::TemplateOnly => {
                self.template_engine.generate_basic(patient_id)
            }
            AnalysisRoute::CachedReplay => {
                self.degradation.analysis_cache.get(&patient_id).unwrap().clone()
            }
            AnalysisRoute::Skip => {
                AnalysisResult::skipped(patient_id)
            }
        }
    }

    async fn analyze_via_llm(
        &mut self,
        ctx: StructuredContext,
        route: RouteDecision,
        degrade_level: DegradationLevel,
    ) -> AnalysisResult {
        // 1. 编译 prompt
        let prompt = self.prompt_compiler.compile(&ctx, &route);

        // 2. 调用 LLM (流式)
        let stream = match self.gateway.stream(&prompt, route.max_output_tokens).await {
            Ok(s) => s,
            Err(e) => {
                self.degradation.on_network_change(false);
                return self.template_engine.generate_with_kb(&ctx);
            }
        };

        // 3. 收集流式输出 (同时转发到 WebSocket, 在外层处理)
        let mut full_text = String::new();
        // ... stream 消费逻辑 ...

        // 4. 校验
        let validated = match self.validator.validate(&full_text, ctx.triage_current) {
            ValidationResult::Pass(text) | ValidationResult::PassWithWarning(text, _) => text,
            ValidationResult::FailAndFallback(reasons) => {
                return self.template_engine.generate_with_kb(&ctx);
            }
        };

        // 5. 提取 RiskAdjustment
        let risk_adjust = self.risk_extractor.extract(&validated);

        // 6. 记录冷却期
        let result = AnalysisResult { ... };
        self.degradation.on_analysis_complete(ctx.patient_id, result.clone());
        result
    }
}
```

---

## 六、sensing-server 集成点

### 6.1 AppState 新增字段

```rust
// sensing-server/src/types.rs 或 main.rs 的 AppState
pub struct AppStateInner {
    // ... 现有字段 ...
    pub medical_agent: MedicalAgent,           // 新增
    pub medical_kb: MedicalKB,                 // 新增, 启动时加载
    pub analysis_tx: broadcast::Sender<AnalysisResult>,  // 新增, 流式推送
}
```

### 6.2 事件触发点

```rust
// udp_receiver.rs — 在 TriageEngine::process() 之后
if let Some(triage_update) = &sensing_update.triage_update {
    for survivor in &triage_update.survivors {
        if survivor.just_deteriorated {
            let trigger = TriggerSource::Deterioration { ... };
            let agent = state.medical_agent.clone();  // Arc<Mutex<>>
            let result = agent.lock().await.analyze(survivor.id, trigger, &state).await;
            let _ = state.analysis_tx.send(result);
        }
    }
}

// 新增: 定时巡检任务 (tokio::spawn 在 main.rs)
// 每 30 秒扫描所有伤员, 对超过 5 分钟没分析的触发 PeriodicScan
```

### 6.3 WebSocket 推送

```rust
// handlers/ws.rs — 新增 analysis 推送
// 在现有的 sensing_update 推送旁, 监听 analysis_tx
tokio::spawn(async move {
    let mut rx = state.analysis_tx.subscribe();
    while let Ok(result) = rx.recv().await {
        let json = serde_json::to_string(&result).unwrap();
        // 通过 WebSocket broadcast 发送
        // triage.html 收到后渲染 AI 分析卡片
    }
});
```

---

## 七、分阶段实施

### Phase 1: 剥离 + 骨架 (2天)

**目标**: 代码能编译, 新模块骨架就位

- [ ] 读现有 `streaming.rs` / `lib.rs` / `types.rs`, 理解当前接口
- [ ] 修改 `Cargo.toml`: 删 candle 系, 加 reqwest/tokio-stream/backoff/regex
- [ ] 更新 `types.rs`: 删 Candle 相关类型, 加 Agent 类型 (用第四节的定义)
- [ ] 新建空骨架文件: `gateway.rs`, `degrade.rs`, `context.rs`, `medical_kb.rs`, `router.rs`, `prompt.rs`, `validator.rs`, `risk_adjust.rs`
- [ ] 修改 `lib.rs`: feature flag `llm`→`agent`, 导出新模块
- [ ] `cargo check -p wifi-densepose-llm` → 0 errors

### Phase 2: 本地模块 (3天)

**目标**: 所有不依赖网络的模块完成 + 单元测试通过

- [ ] `medical_kb.rs`: 完整实现 + `data/medical_knowledge.json` (15条目)
- [ ] `context.rs`: 实现, 依赖 MAT 管线数据读取
- [ ] `router.rs`: 实现 + 单元测试 (所有路由组合)
- [ ] `degrade.rs`: 实现 + 单元测试 (状态迁移)
- [ ] `risk_adjust.rs`: 实现 + 测试 (正则提取)
- [ ] `validator.rs`: 实现 + 测试 (拦截/通过/矛盾)
- [ ] `template.rs`: 增强 KB 注入
- [ ] `cargo test -p wifi-densepose-llm` → 全部通过

### Phase 3: Prompt + Gateway (2天)

**目标**: LLM 调用链路打通

- [ ] `prompt.rs`: 实现 + prompt 模板文件
- [ ] `gateway.rs`: 实现, 先用真实 LLM API 联调
- [ ] Agent 主循环 (`lib.rs`): 完整编排逻辑
- [ ] 集成测试: context → prompt → API → validate 全链路 (用 mock HTTP server)
- [ ] 调优 prompt 模板 (跑 10 个典型场景, 检查 LLM 输出质量)

### Phase 4: 集成 + UI (3天)

**目标**: sensing-server 集成, UI 适配, 端到端可用

- [ ] `AppState` 加 `MedicalAgent` 和 `MedicalKB`
- [ ] `udp_receiver.rs` 加事件触发点
- [ ] 定时巡检任务
- [ ] WebSocket 推送 analysis 消息
- [ ] `triage.html` AI 卡片适配: 流式渲染, 降级状态提示, 第二意见展示
- [ ] 端到端测试: 模拟 CSI 数据 → 分诊触发 → LLM 分析 → UI 展示
- [ ] 降级场景测试: 断网/LLM超时/LLM返回异常内容

**总计: 10天**

---

## 八、测试数据

### 8.1 单元测试用 VitalSnapshot 构造器

```rust
#[cfg(test)]
fn make_vitals(hr: f32, rr: f32, motion: &str) -> VitalSnapshot {
    VitalSnapshot {
        breathing_rate_bpm: Some(rr),
        heart_rate_bpm: Some(hr),
        breathing_confidence: 0.9,
        heartbeat_confidence: 0.85,
        signal_quality: 0.8,
        motion_class: Some(motion.to_string()),
        person_count_estimate: Some(1),
        rssi: Some(-45),
    }
}
```

### 8.2 测试场景

| 测试 | 输入 | 期望路由 | 期望 KB 匹配 |
|------|------|---------|-------------|
| 心动过速+恶化 | HR=135 RR=22 motion=still | DeepLLM | tachycardia |
| 心动过速+稳定 | HR=110 RR=18 motion=still | BriefLLM | tachycardia |
| 呼吸窘迫+恶化 | HR=95 RR=32 motion=active | DeepLLM | respiratory_distress |
| 体征正常 | HR=72 RR=14 motion=still | TemplateWithKB | 无 |
| Deceased | HR=0 RR=0 | Skip | — |
| 冷却期内 | (任意) | CachedReplay | — |

---

## 九、注意事项

1. **锁范围**: Context Collator 持 `state.inner.read()` 锁时间 <2ms。prompt 编译和 LLM 调用都在锁外执行。
2. **模板文件加载**: prompt 模板从 `data/prompts/*.txt` 读取，启动时加载，支持 `SIGHUP` 热重载。
3. **API Key 安全**: 通过环境变量 `LLM_API_KEY` 注入，不写死在代码或配置文件里。
4. **KB 热更新**: 提供 API 端点 `POST /api/v1/kb/reload` 重新加载知识库 JSON。
5. **START 分诊不受影响**: RiskAdjustment 只作为第二意见展示，不会自动修改分诊等级。
6. **流式 token**: LLM 的每个 SSE token 直接通过 WebSocket 推送到 triage.html，不等待完整响应。
