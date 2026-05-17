# 端侧 LLM 智能分析引擎 — 方案设计

> 项目：WCES — 基于WiFi CSI感知与端侧LLM的方舱生命体征感知与监护系统
> 硬件：瑞萨 RZ/V2H (Cortex-A55 ×4, 8GB RAM, DRP-AI 可选)
> 文档版本: v2.1 | 2026-05-17

---

## 一、定位升级：从"报告生成器"到"智能分析引擎"

### 1.1 旧定位（v1.0）

LLM 只做一件事——把数字转成人话：

```
输入:  呼吸:32/min, 心率:115BPM, START:Immediate
输出:  "伤员呼吸急促(32次/分)，心率115BPM，符合START Immediate分诊标准..."
```

本质上是**模板渲染器的智能版**，不参与真正的分析决策。

### 1.2 新定位（v2.0）

LLM 作为**智能分析引擎**，深度参与数据理解：

1. **结合既往病史与当前病史** — 检索患者历史记录，融入分析上下文
2. **多体征联合推断** — 呼吸率 + 心率 + 运动状态 + 骨架姿态 + 边缘模块告警 联合分析
3. **疾病特征匹配** — 将当前体征模式与结构化医学知识库中的疾病特征做匹配
4. **趋势感知** — 不只是当前值，而是时间窗口内的变化趋势
5. **优先级细化** — 在 START 规则引擎基础上，提供第二层更细粒度的风险评估
6. **处置建议生成** — 基于匹配到的疾病知识和当前状态给出可操作建议

### 1.3 价值对比

| 维度 | v1.0 报告生成器 | v2.0 智能分析引擎 |
|------|:---:|:---:|
| 数据输入 | 单帧体征 | 多帧趋势 + 病史 + 边缘告警 |
| 知识来源 | 模型内化 (无) | RAG 外挂医学知识 |
| 分析深度 | 描述性 | 诊断性 + 预测性 |
| 输出产物 | 文本报告 | 分析报告 + 风险评分 + 建议 |
| 触发方式 | 手动调用 | 事件/周期/手动 三种 |
| 安全边界 | 无定义 | 规则引擎保底，LLM 只做增强 |

---

## 二、核心硬件约束

RZ/V2H 是 ARM64 嵌入式计算平台，推理速度是核心瓶颈：

| 约束项 | 数值 | 影响 |
|--------|------|------|
| 推理速度 | **~500ms/token** (Cortex-A55, Qwen2.5-0.5B INT4) | 不能逐帧推理 |
| 100 token 响应 | **~50秒** | 必须流式输出 |
| 500 token 深度分析 | **~4分钟** | 只能周期触发 |
| 可用 RAM | ~5.3GB (8GB - OS/sensing-server) | 一个模型 + 充足缓存 |
| CPU 核心 | Cortex-A55 ×4 | 推理占用 1-2 核，其他核可处理 CSI |

**关键设计原则：**
- **异步而非同步** — 不走实时逐帧，走事件/周期触发
- **RAG 而非内化** — 医学知识外挂注入，不指望 0.5B 模型自带
- **流式输出** — token 逐个推送 UI，50秒等待变成逐字显示
- **双层安全** — 规则引擎（START 分诊 + 告警）是硬保底层，LLM 只做增强分析

---

## 三、技术选型

### 3.1 模型对比

| 模型 | 参数量 | INT4 大小 | RAM 占用 | 中文能力 | 推理速度 (A55) |
|------|:---:|:---:|:---:|:---:|:---:|
| **Qwen2.5-0.5B-Instruct** | 0.5B | ~380MB | ~500MB | ⭐⭐⭐ 原生中文 | ~500ms/token |
| Qwen2.5-1.5B-Instruct | 1.5B | ~1.0GB | ~1.3GB | ⭐⭐⭐ | ~1.5s/token |
| Phi-3-mini-4k | 3.8B | ~2.2GB | ~2.5GB | ❌ 英文为主 | ~3s/token |
| Gemma-2-2B | 2B | ~1.4GB | ~1.7GB | ⭐⭐ | ~2s/token |
| SmolLM2-1.7B | 1.7B | ~1.1GB | ~1.4GB | ❌ | ~1.5s/token |

**推荐：Qwen2.5-0.5B-Instruct** — 最轻量 + 原生中文 + 阿里出品

> 注：0.5B 模型不具备内化的医学知识，需要依赖外部 RAG 注入。它的核心能力是
> **在给定上下文中做结构化推断**，而非从记忆中调取知识。方案设计以此为前提。

### 3.2 推理框架对比

| 框架 | 语言 | 依赖 | aarch64 | 量化 | 大小 |
|------|:---:|------|:---:|:---:|:---:|
| **candle** | Rust | **零 C++ 依赖** | ✅ | ✅ GGUF/GGML | ~5MB |
| llama.cpp | C++ | 需交叉编译 | ✅ | ✅ GGUF | ~2MB |
| onnxruntime | C++ | 需交叉编译 | ✅ | ✅ ONNX | ~20MB |
| mistral.rs | Rust | 需 C++ 编译链 | ✅ | ✅ GGUF | ~10MB |

**推荐：candle** — 纯 Rust，与现有 sensing-server 技术栈一致，交叉编译最简单（无需 C++ 工具链）。

### 3.3 模型获取

```bash
# 下载 Qwen2.5-0.5B-Instruct GGUF (Q4_K_M 量化)
wget https://huggingface.co/bartowski/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/Qwen2.5-0.5B-Instruct-Q4_K_M.gguf \
     -O models/qwen2.5-0.5b-q4.gguf
```

---

## 四、整体架构

```
═══════════════════════════════════════════════════════════════════
                        数据采集层 (已有)
───────────────────────────────────────────────────────────────────
  ESP32-C5 ×3 → UDP:5005 → CSI解析 → VitalSignDetector → VitalSigns
                                ↓
                         Motion + Position + 17点COCO骨架
═══════════════════════════════════════════════════════════════════
                        规则引擎层 (已有，保底)
───────────────────────────────────────────────────────────────────
  START分诊 (红/黄/绿/黑)  +  边缘模块告警 (13个WASM模块)
  ┌─────────────────────────────────────────────────────────┐
  │  绝不依赖LLM  |  延迟 <50ms  |  硬安全边界              │
  └─────────────────────────────────────────────────────────┘
═══════════════════════════════════════════════════════════════════
                        LLM 智能分析层 (P10 新建)
───────────────────────────────────────────────────────────────────
         ┌──────────────┐
         │ 触发调度器    │ ← 事件触发 / 周期触发 / 手动触发
         └──────┬───────┘
                ▼
  ┌─────────────────────────────────────────────┐
  │              Context Builder                 │
  │                                              │
  │  ┌──────────┐ ┌───────────┐ ┌────────────┐  │
  │  │ 病史检索  │ │ 知识匹配   │ │ 趋势提取   │  │
  │  │ Patient  │ │ Medical   │ │ Sliding    │  │
  │  │ Record   │ │ Knowledge │ │ Window     │  │
  │  │   DB     │ │   Base    │ │ Analyzer   │  │
  │  └────┬─────┘ └─────┬─────┘ └─────┬──────┘  │
  │       └──────────────┼─────────────┘         │
  │                      ▼                       │
  │               Prompt Builder                 │
  └──────────────────────┬──────────────────────┘
                         ▼
         ┌───────────────────────────┐
         │  Qwen2.5-0.5B (candle)    │
         │  ┌─────────────────────┐  │
         │  │ 流式生成引擎         │  │
         │  │ token → WebSocket   │  │
         │  └─────────────────────┘  │
         └───────────┬───────────────┘
                     ▼
═══════════════════════════════════════════════════════════════════
                        输出层
───────────────────────────────────────────────────────────────────
  WebSocket → triage.html "AI辅助分析"卡片
  ├─ 风险趋势判断 (恶化/稳定/好转)
  ├─ 病史关联提示 (COPD→呼吸阈值下调等)
  ├─ 优先级修正建议 (START黄→建议升级红)
  ├─ 疾病模式匹配 (体征模式符合XXX特征)
  └─ 处置建议 (气道评估/氧疗/转运建议)
═══════════════════════════════════════════════════════════════════
```

### 4.1 为什么是双层架构

```
第一层 (规则引擎, <1ms):
  START 分诊 → 红/黄/绿/黑
  边缘告警 → 呼吸窘迫/心律失常/癫痫/...
  ↑ 这是硬安全边界，绝不依赖 LLM

第二层 (LLM 增强, 异步 30-120s):
  结合病史+趋势+多数据 → 细化优先级 + 风险提示
  ↑ 这是辅助增强，永远不影响核心分诊决策
```

**安全红线：LLM 永远不对分诊做最终决策。** 它可以提供"第二意见"，但规则的判定不可被 LLM 覆盖。

---

## 五、核心组件设计

### 5.1 患者病历数据库 (PatientRecord DB)

**目的**：存储每个伤员的基本信息、既往病史、当前病史、药物记录、分诊历史。

```rust
// crates/wifi-densepose-llm/src/patient_record.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientRecord {
    pub patient_id: String,          // "PAT-0001"
    pub name: Option<String>,        // 伤员姓名 (可选)
    pub age: Option<u8>,             // 年龄
    pub gender: Option<Gender>,
    /// 既往病史 (如: ["COPD", "2型糖尿病", "高血压"])
    pub pre_existing: Vec<String>,
    /// 当前主诉 (如: "胸部疼痛3小时，呼吸困难")
    pub chief_complaint: Option<String>,
    /// 过敏史
    pub allergies: Vec<String>,
    /// 长期用药 (如: ["沙美特罗替卡松 50/250μg bid"])
    pub medications: Vec<String>,
    /// 关联的监测节点ID
    pub node_id: Option<u8>,
    /// 入舱时间
    pub admission_time: Option<chrono::DateTime<chrono::Utc>>,
    /// 备注
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
    Other,
}
```

**存储方案**：使用 JSON 文件或 `sled` 嵌入式 KV 数据库（零配置、纯 Rust）：

```rust
pub struct PatientRecordDB {
    /// sled 嵌入式数据库
    db: sled::Db,
}

impl PatientRecordDB {
    pub fn open(path: &str) -> Result<Self> { /* ... */ }
    pub fn put(&self, record: &PatientRecord) -> Result<()> { /* ... */ }
    pub fn get(&self, patient_id: &str) -> Result<Option<PatientRecord>> { /* ... */ }
    pub fn get_by_node_id(&self, node_id: u8) -> Result<Option<PatientRecord>> { /* ... */ }
    pub fn list_all(&self) -> Result<Vec<PatientRecord>> { /* ... */ }
    pub fn delete(&self, patient_id: &str) -> Result<()> { /* ... */ }
}
```

**API 接口**：

```
GET    /api/v1/patients              → 列出所有伤员
POST   /api/v1/patients              → 创建伤员记录
GET    /api/v1/patients/:id          → 获取伤员详情
PUT    /api/v1/patients/:id          → 更新伤员信息
DELETE /api/v1/patients/:id          → 删除伤员记录
```

### 5.2 结构化医学知识库 (Medical Knowledge Base)

**目的**：将方舱常见伤病/急症的体征特征、鉴别诊断、紧急程度、处置建议结构化存储，
作为 RAG 检索源注入 LLM prompt，弥补 0.5B 模型医学知识不足的问题。

```json
{
  "conditions": [
    {
      "id": "respiratory_failure_acute",
      "name": "急性呼吸衰竭",
      "key_indicators": {
        "breathing_rate": { "condition": ">30 或 <10", "trend": "持续恶化" },
        "heart_rate": { "condition": ">120 或 <50", "trend": "随缺氧加重" },
        "signal_quality": "可能下降 (体动增加)",
        "motion_pattern": "呼吸急促 → 呼吸浅慢 → 停止",
        "edge_alerts": ["med_respiratory_distress", "med_sleep_apnea"]
      },
      "differential": ["COPD急性加重", "肺栓塞", "心源性肺水肿"],
      "risk_factors": ["COPD病史", "高龄(>65)", "胸外伤", "吸烟史"],
      "urgency": "critical",
      "triage_recommendation": "Immediate (红)",
      "actions": [
        "立即气道评估，检查有无气道阻塞",
        "给予高流量氧疗 (10-15L/min 储氧面罩)",
        "准备球囊面罩辅助通气",
        "优先转运至红色区域，持续监测SpO2",
        "如呼吸停止，立即开始BVM通气"
      ],
      "monitoring_focus": [
        "呼吸频率变化趋势 (每5分钟)",
        "心率变异性",
        "运动评分 (警惕由躁动→静止的转变)"
      ]
    },
    {
      "id": "sepsis_early",
      "name": "脓毒症 (早期)",
      "key_indicators": {
        "breathing_rate": { "condition": ">22", "trend": "逐渐加快" },
        "heart_rate": { "condition": ">90", "trend": "持续升高" },
        "signal_quality": "可能波动 (寒战引起体动)",
        "motion_pattern": "可能出现寒战样高频微动",
        "edge_alerts": ["med_cardiac_arrhythmia"]
      },
      "differential": ["严重感染", "低血容量休克", "过敏反应"],
      "risk_factors": ["开放性伤口", "免疫力低下", "留置导管", "高龄"],
      "urgency": "high",
      "triage_recommendation": "Immediate (红) — 如合并低血压或意识改变",
      "actions": [
        "立即评估感染源 (检查伤口、导管)",
        "建立静脉通路，开始液体复苏",
        "密切监测心率趋势",
        "考虑升级为红色优先级"
      ],
      "monitoring_focus": [
        "心率上升速率",
        "呼吸频率与心率的比值 (正常约1:4，异常升高提示脓毒症)",
        "运动评分变化 (意识状态间接反映)"
      ]
    },
    {
      "id": "arrhythmia_acute",
      "name": "急性心律失常",
      "key_indicators": {
        "heart_rate": { "condition": ">130 或 <45", "trend": "突然变化" },
        "breathing_rate": { "condition": "可能正常或代偿性加快" },
        "signal_quality": "可能下降 (心率不规整影响CSI信号)",
        "motion_pattern": "可能伴随意识丧失后的静止",
        "edge_alerts": ["med_cardiac_arrhythmia"]
      },
      "differential": ["心肌梗死", "电解质紊乱", "药物毒性"],
      "risk_factors": ["心脏病史", "高血压", "糖尿病", "高龄"],
      "urgency": "critical",
      "triage_recommendation": "Immediate (红)",
      "actions": [
        "立即评估意识状态和脉搏",
        "如无脉搏，启动CPR",
        "如意识清楚，给予吸氧，尽快心电图评估",
        "优先转运红色区域"
      ],
      "monitoring_focus": [
        "心率突然变化幅度",
        "HRV (心率变异性) 是否异常",
        "是否伴随运动评分的突然下降"
      ]
    },
    {
      "id": "copd_exacerbation",
      "name": "COPD急性加重",
      "key_indicators": {
        "breathing_rate": { "condition": ">25 (对COPD患者，基线可能已偏高)", "trend": "逐渐恶化" },
        "heart_rate": { "condition": ">100", "trend": "代偿性升高" },
        "signal_quality": "可能下降 (呼吸模式紊乱)",
        "motion_pattern": "呼吸费力 → 辅助呼吸肌参与 → 端坐呼吸姿势变化",
        "edge_alerts": ["med_respiratory_distress", "vital_trend (呼吸过速告警)"]
      },
      "differential": ["急性呼吸衰竭", "肺炎", "气胸"],
      "risk_factors": ["COPD病史", "近期感染", "吸烟", "空气污染暴露"],
      "urgency": "high",
      "triage_recommendation": "Immediate (红) — 如RR>30或出现意识改变",
      "actions": [
        "对比患者基线呼吸率 (COPD患者静息RR常已偏高)",
        "给予控制性氧疗 (目标SpO2 88-92%，避免高氧)",
        "使用支气管扩张剂",
        "密切监测呼吸趋势，警惕CO2潴留"
      ],
      "monitoring_focus": [
        "呼吸率与基线的偏差",
        "呼吸模式是否变浅变快 (提示呼吸肌疲劳)",
        "运动评分趋势 (由端坐→平卧可能提示疲劳加重)"
      ]
    },
    {
      "id": "seizure",
      "name": "癫痫发作",
      "key_indicators": {
        "motion_pattern": "突然出现规律性高幅度运动，持续30-120秒后突然停止",
        "breathing_rate": "发作期间可能暂停或极不规则",
        "heart_rate": "发作期间显著升高",
        "signal_quality": "发作期间剧烈下降 (大幅运动干扰CSI)",
        "edge_alerts": ["med_seizure_detect (强直/阵挛/发作后)"]
      },
      "differential": ["心源性晕厥", "低血糖", "假性发作"],
      "risk_factors": ["癫痫病史", "脑外伤", "停药", "睡眠剥夺"],
      "urgency": "high",
      "triage_recommendation": "Immediate (红) — 如发作持续>5分钟或连续发作",
      "actions": [
        "保护伤员，防止二次伤害",
        "不要强行约束",
        "发作停止后评估意识和呼吸",
        "如发作>5分钟 (癫痫持续状态)，紧急医疗干预",
        "记录发作持续时间和特征"
      ],
      "monitoring_focus": [
        "运动评分突增突降模式",
        "发作后呼吸恢复速度",
        "发作后心率的恢复趋势",
        "连续发作间隔"
      ]
    }
  ]
}
```

**匹配策略**：不要求精确匹配，而是计算**体征相似度得分**，选出 Top-3 最匹配的疾病条目注入 prompt：

```rust
pub struct MedicalKnowledgeBase {
    conditions: Vec<MedicalCondition>,
}

impl MedicalKnowledgeBase {
    /// 加载知识库 JSON
    pub fn load(path: &str) -> Result<Self> { /* ... */ }

    /// 根据当前体征 + 病史 + 边缘告警，匹配最相关的疾病条目 (Top-3)
    pub fn match_conditions(
        &self,
        vitals: &VitalSignsSnapshot,
        history: &PatientRecord,
        edge_alerts: &[EdgeAlert],
    ) -> Vec<(MedicalCondition, f64)> {
        // 计算每条疾病条目的匹配得分:
        //   - 体征匹配: RR/HR 是否在疾病指示范围内 (+40%)
        //   - 趋势匹配: 变化方向是否符合疾病特征 (+20%)
        //   - 风险因子匹配: 病史中是否有风险因子 (+25%)
        //   - 边缘告警匹配: 是否触发了相关告警 (+15%)
        // 返回 top-3
    }
}
```

### 5.3 滑动窗口趋势分析器 (Sliding Window Analyzer)

**目的**：不从原始数据做 LLM 推理（token 消耗爆炸），而是先把时序数据压缩成统计摘要，
降低 prompt token 消耗，同时保留趋势信息。

```rust
// crates/wifi-densepose-llm/src/sliding_window.rs

#[derive(Debug, Clone, Serialize)]
pub struct VitalTrendSummary {
    /// 观测窗口长度 (秒)
    pub window_seconds: f64,
    /// 采样点数
    pub sample_count: usize,

    // 呼吸率趋势
    pub rr_mean: f64,
    pub rr_min: f64,
    pub rr_max: f64,
    pub rr_trend: TrendDirection,    // Rising / Stable / Falling
    pub rr_change_pct: f64,          // 窗口内变化百分比
    pub rr_volatility: f64,          // 变异系数 (CV) — 衡量波动程度

    // 心率趋势
    pub hr_mean: f64,
    pub hr_min: f64,
    pub hr_max: f64,
    pub hr_trend: TrendDirection,
    pub hr_change_pct: f64,
    pub hr_volatility: f64,

    // 运动状态趋势
    pub motion_mean: f64,
    pub motion_pattern: MotionPattern,  // 持续静止 / 间歇运动 / 持续运动 / 突增突降

    // 信号质量趋势
    pub signal_quality_mean: f64,
    pub signal_quality_trend: TrendDirection,
}

#[derive(Debug, Clone, Serialize)]
pub enum TrendDirection {
    Rising,    // ↑ 上升
    Stable,    // → 稳定
    Falling,   // ↓ 下降
}

#[derive(Debug, Clone, Serialize)]
pub enum MotionPattern {
    ContinuousStill,
    IntermittentMotion,
    ContinuousMotion,
    SpikeAndDrop,    // 突增突降 — 可能提示癫痫/惊厥
    GradualDecline,  // 逐渐减少 — 可能提示意识下降
}
```

**使用示例**：

```rust
// 在 sensing-server 主循环中维护滑动窗口
let mut rr_window = SlidingWindow::new(Duration::from_secs(300)); // 5分钟

// 每收到一帧数据
rr_window.push(now, vitals.breathing_rate_bpm);

// 触发分析时提取摘要
let summary = rr_window.summarize();
// → rr_mean: 31.2, rr_trend: Rising(+15%), rr_volatility: 0.08
```

**窗口策略**：
- **短期窗口 (1分钟)**：用于即时告警的二阶确认（如 START 分诊等级变化时立即触发）
- **中期窗口 (5分钟)**：标准分析窗口（如周期触发 30-60s 一次）
- **长期窗口 (30分钟)**：提供基线对比（如判断 COPD 患者现在的呼吸率是否偏离其基线）

### 5.4 Prompt Builder — 上下文组装

**目的**：将病史检索结果 + 知识匹配结果 + 趋势摘要 + 当前体征 组装成 LLM prompt。

```rust
// crates/wifi-densepose-llm/src/prompt_builder.rs

pub fn build_analysis_prompt(
    patient: &PatientRecord,           // 含既往病史
    vitals: &VitalSignsSnapshot,       // 当前体征
    trends: &VitalTrendSummary,        // 5分钟趋势
    matched_conditions: &[(MedicalCondition, f64)], // 匹配到的疾病(含得分)
    edge_alerts: &[EdgeAlert],         // 当前活跃的边缘告警
    current_triage: &str,              // START 分诊等级
) -> String {
    format!(
        r#"# 角色
你是一名方舱医院的急救医师，负责根据WiFi CSI无接触监测系统采集的生命体征数据，
为医护人员提供辅助分析。你的分析仅供参考，最终决策由医护人员做出。

# 伤员基本信息
- ID: {patient_id}
- 年龄: {age}岁
- 性别: {gender}
{pre_existing_section}
{chief_complaint_section}
{medication_section}

# 当前体征数据
- 呼吸率: {rr} 次/分钟 (正常: 12-20, 危急: <10或>30)
- 心率: {hr} 次/分钟 (正常: 60-100)
- 信号质量: {sq}
- 运动状态: {motion}
- START分诊: {triage}

# 趋势分析 (过去5分钟)
- 呼吸率: 均值{rr_mean}, 趋势{rr_trend}({rr_change}%), 波动{rr_vol}
- 心率: 均值{hr_mean}, 趋势{hr_trend}({hr_change}%), 波动{hr_vol}
- 运动模式: {motion_pattern}

# 活跃告警
{edge_alerts_section}

# 参考知识 (体征模式匹配结果)
{matched_conditions_section}

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

只输出 JSON，不要额外解释。"#
    , /* ... 参数填充 ... */)
}
```

**设计要点**：
- 输出用结构化 JSON — 便于 UI 解析渲染，而非纯文本
- 病史/知识/趋势 全部作为 prompt 上下文注入 — 不依赖模型记忆
- 明确标注"仅供参考，最终决策由医护人员做出" — 安全边界
- 体温 0.3 确保输出稳定，格式可控

### 5.5 流式生成引擎

**目的**：避免用户等 50-120 秒才能看到结果，token 逐字推送到前端。

```rust
// crates/wifi-densepose-llm/src/streaming.rs

use tokio::sync::broadcast;

pub struct StreamingGenerator {
    model: ModelForCausalLM,
    tokenizer: Tokenizer,
    device: Device,
}

pub struct StreamToken {
    pub survivor_id: String,
    pub token_index: u32,
    pub text: String,        // 增量文本 (逐 token)
    pub is_complete: bool,
}

impl StreamingGenerator {
    pub async fn generate_stream(
        &self,
        prompt: &str,
        survivor_id: &str,
        tx: broadcast::Sender<StreamToken>,
    ) -> Result<()> {
        // Tokenize
        let tokens = self.tokenizer.encode(prompt, true)?;
        let mut input_ids = Tensor::new(&tokens.get_ids()[..], &self.device)?
            .unsqueeze(0)?;

        // 逐 token 生成
        for i in 0..max_new_tokens {
            let logits = self.model.forward(&input_ids)?;
            let next_token = sample_token(&logits, temperature)?;

            // 解码为文本
            let text = self.tokenizer.decode(&[next_token], false)?;

            // 通过 broadcast channel 推送到 WebSocket
            let _ = tx.send(StreamToken {
                survivor_id: survivor_id.to_string(),
                token_index: i,
                text,
                is_complete: false,
            });

            // 追加 token
            input_ids = append_token(&input_ids, next_token)?;

            // 检查 EOS
            if next_token == eos_token_id { break; }
        }

        // 发送完成信号
        let _ = tx.send(StreamToken {
            survivor_id: survivor_id.to_string(),
            token_index: u32::MAX,
            text: String::new(),
            is_complete: true,
        });

        Ok(())
    }
}
```

**WebSocket 新增消息类型**：

```json
// 服务端 → 前端 (流式)
{
  "type": "llm_stream",
  "survivor_id": "SURV-0001",
  "token_index": 15,
  "text": "呼吸",
  "is_complete": false
}

// 服务端 → 前端 (分析完成)
{
  "type": "llm_analysis_complete",
  "survivor_id": "SURV-0001",
  "analysis_json": { /* 完整 JSON 分析结果 */ },
  "generated_tokens": 187,
  "elapsed_ms": 93500
}
```

---

## 六、触发策略

```
触发类型          条件                           延迟     用途
────────────────────────────────────────────────────────────────
🔴 事件触发       分诊等级变化 (绿→黄/黄→红)      即时     第一时间评估恶化
🔴 事件触发       边缘模块新告警 (呼吸窘迫/心律    即时     对告警做医学解读
                 失常/癫痫/武器检测...)
🟡 周期触发       每 30-60 秒                     周期     趋势分析 + 稳定伤员复查
🟡 周期触发       新伤员入舱后 5 分钟             首次     建立基线分析
🟢 手动触发       医护人员点击 triage.html 卡片    即时     按需深度分析
```

**示例流程**：

```
T=0s   伤员 SURV-0001 入舱，START=Delayed (黄)
T=5s   → 触发首次分析 (事件: 新伤员入舱)
        LLM: "患者生命体征基本正常，START黄区判定合理。
              建议每30秒监测呼吸趋势..."
T=30s  → 周期触发
        LLM: "体征稳定，无恶化趋势。"
T=60s  → 周期触发
        LLM: "呼吸率从16升至22(↑37.5%)，心率从72升至95(↑32%)，
              根据病史COPD，建议关注呼吸窘迫风险..."
T=75s  呼吸率突破30 → START=Immediate (红)
T=75s  → 事件触发 (分诊等级变化!)
        LLM: "⚠️ 呼吸率已超过30次/分，结合COPD病史和持续上升趋势，
              符合急性呼吸衰竭早期表现。建议立即气道评估+控制性氧疗，
              推荐优先级升级为红色区域最高关注级..."
```

---

## 七、数据流（完整）

```
ESP32-C5 ×3
  │ UDP:5005
  ▼
sensing-server (Rust, RZ/V2H)
  │
  ├─→ [规则引擎层] ──────────────────────────────
  │    ├─ VitalSignDetector → RR, HR
  │    ├─ TriageEngine → START 分诊 + 告警
  │    └─ EdgeModuleEngine → 13个模块检测
  │         │
  │         └─→ SensingUpdate (每帧, <50ms)
  │              ├─ vital_signs
  │              ├─ triage_update
  │              ├─ wasm_alerts
  │              └─ pose_keypoints
  │                   │
  │                   └─ WebSocket /ws/sensing → triage.html (实时)
  │
  ├─→ [LLM 智能分析层] ─────────────────────────
  │    │
  │    ├─ SlidingWindow (持续更新)
  │    │    └─ 维护每个伤员的 1min/5min/30min 窗口
  │    │
  │    ├─ TriggerScheduler
  │    │    ├─ 监听分诊等级变化 → 事件触发
  │    │    ├─ 监听边缘告警 → 事件触发
  │    │    └─ 定时器 30-60s → 周期触发
  │    │
  │    ├─ ContextBuilder (触发时执行)
  │    │    ├─ PatientRecordDB.get(patient_id)
  │    │    ├─ MedicalKnowledgeBase.match(vitals, history, alerts)
  │    │    ├─ SlidingWindow.summarize(patient_id)
  │    │    └─ PromptBuilder.build(...)
  │    │
  │    └─ StreamingGenerator.generate_stream(prompt)
  │         │
  │         └─ WebSocket /ws/sensing (新增 llm_stream 事件)
  │              → triage.html "AI分析"卡片 (逐字显示)
  │
  └─→ [HTTP API]
       ├─ GET  /api/v1/patients → 伤员列表
       ├─ POST /api/v1/patients → 创建伤员
       ├─ GET  /api/v1/patients/:id → 伤员详情
       ├─ PUT  /api/v1/patients/:id → 更新伤员信息
       ├─ POST /api/v1/llm/analyze → 手动触发分析
       └─ GET  /api/v1/llm/analysis/:id → 获取最近分析结果
```

---

## 八、内存预算

```
RZ/V2H 总 RAM: 8GB
───────────────────────────────────────────
Linux OS:                  ~1.5GB
sensing-server (Rust):     ~500MB   (二进制 + CSI 缓冲区 + MAT + Edge)
Qwen2.5-0.5B (INT4):       ~500MB   (模型加载 + KV Cache)
PatientRecordDB (sled):    ~50MB    (嵌入式 KV 存储)
MedicalKnowledge KB:       ~5MB     (JSON 知识库, 内存中)
SlidingWindow buffers:     ~20MB    (每个伤员 3 个窗口)
WebSocket + UI assets:     ~200MB
───────────────────────────────────────────
空闲:                      ~5.2GB   ✅ 充足
```

---

## 九、降级策略

LLM 是**辅助功能**，系统核心（CSI检测→分诊+告警）完全不依赖它：

```
优先级 (硬安全):
  1. CSI → VitalSigns → START分诊 → 告警    (必须, <50ms)
  2. 13× 边缘模块 → 异常检测 → 告警         (必须, <10ms/模块)
  3. LLM 智能分析 → 增强报告 + 建议          (可选, 30-120s)
  4. ONNX DensePose → 3D 骨架              (可选, 100-200ms)
```

**三级降级**：

| 级别 | 触发条件 | 行为 |
|:---:|------|------|
| L1 正常 | LLM 加载成功，推理正常 | 完整 AI 分析 |
| L2 降级 | LLM 推理超时 (>120s) 或模型未加载 | 回退到模板报告 (结构化的数据摘要，无 LLM 推理) |
| L3 最小 | LLM 加载失败 | 无 AI 分析卡片，仅显示规则引擎结果 |

```rust
fn fallback_analysis(
    patient: &PatientRecord,
    vitals: &VitalSignsSnapshot,
    trends: &VitalTrendSummary,
    triage: &str,
    matched: &[(MedicalCondition, f64)],
) -> LlmAnalysisResult {
    // 使用匹配到的知识库条目 + 趋势数据构建结构化分析
    // 比纯模板更有信息量，但不需要 LLM 推理
    LlmAnalysisResult {
        risk_assessment: RiskAssessment {
            overall_level: match triage {
                "Immediate" => "critical",
                "Delayed" => "high",
                "Minor" => "low",
                _ => "moderate"
            }.into(),
            primary_concern: matched.first()
                .map(|(c, _)| c.name.clone())
                .unwrap_or("无明确模式匹配".into()),
            deteriorating: matches!(trends.rr_trend, TrendDirection::Rising)
                || matches!(trends.hr_trend, TrendDirection::Rising),
            deterioration_evidence: format!(
                "呼吸率{}%, 心率{}%",
                trends.rr_change_pct, trends.hr_change_pct
            ),
        },
        // ... 其他字段基于规则填充
        generated_by: "fallback_template".into(),
    }
}
```

---

## 十、实施计划

| 阶段 | 内容 | 产出 | 工作量 |
|:---:|------|------|:---:|
| **P10-1** | **基础设施搭建** | | **4-6h** |
| | 创建 `wifi-densepose-llm` crate | Cargo.toml + lib.rs 框架 | 0.5h |
| | PatientRecordDB (sled) | 嵌入式病历存储 + CRUD API | 2h |
| | MedicalKnowledgeBase | JSON 知识库 (5+疾病条目) + 匹配算法 | 2h |
| | triage.html 伤员信息录入表单 | 医护人员可录入/编辑伤员病历 | 1.5h |
| **P10-2** | **分析管线** | | **5-7h** |
| | SlidingWindow 趋势提取器 | 3个时间窗口 + 统计摘要 | 2h |
| | PromptBuilder 上下文组装 | 病史+知识+趋势 拼装逻辑 | 1.5h |
| | 触发调度器 (事件/周期/手动) | 监听分诊变化+告警+定时器 | 1.5h |
| | 模板回退分析 (L2降级) | fallback_analysis 实现 | 1h |
| **P10-3** | **LLM 推理集成** | | **4-6h** |
| | Candle + Qwen2.5-0.5B 加载 | GGUF 模型加载 + tokenizer | 3h |
| | 流式生成引擎 | token-by-token → broadcast channel | 1.5h |
| | WebSocket llm_stream 事件 | 新增消息类型 + triage.html 渲染 | 1.5h |
| **P10-4** | **前端 + 联调** | | **5-7h** |
| | triage.html "AI分析"卡片 | 流式显示 + JSON 解析渲染 | 2h |
| | 手动触发 + 分析历史 | 医护人员交互 | 1.5h |
| | 端到端联调 + 边界测试 | 降级/超时/并发 测试 | 2h |
| | RZ/V2H 交叉编译 + 性能测试 | aarch64 编译，实测 token/s | 1.5h |
| **合计** | | | **18-26h** |

### 竞赛策略

```
初赛 (视频+报告):
  → 实现 P10-1 + P10-2 (基础设施 + 模板回退分析)
  → P10-3 下载模型 + 本地测试 (不要求在RZ/V2H上跑)
  → 在设计报告中完整描述架构 + PPT展示流式推理方案

决赛 (现场演示):
  → P10-3 + P10-4 (真实 LLM + 联调)
  → 优先保证 L2 模板回退分析稳定可靠
  → LLM 作为锦上添花，而非必须依赖
```

---

## 十一、Cargo.toml 依赖

```toml
[package]
name = "wifi-densepose-llm"
version = "0.1.0"
edition = "2021"
description = "Edge LLM intelligent analysis engine for WCES field hospital triage"

[dependencies]
# 推理框架
candle-core = "0.8"
candle-nn = "0.8"
candle-transformers = "0.8"

# Tokenizer
tokenizers = "0.21"

# 嵌入式数据库 (病历存储)
sled = "0.34"

# 异步运行时
tokio = { version = "1", features = ["sync", "time", "rt"] }

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# 时间
chrono = { version = "0.4", features = ["serde"] }

# 工具
anyhow = "1"
tracing = "0.1"
log = "0.4"

[features]
default = ["llm"]
llm = []             # 启用完整 LLM (需下载模型)
template-only = []   # 仅用模板回退 (无需模型, 竞赛初赛可用)
```

### workspace 集成

在 `rust-server/Cargo.toml` 中添加：

```toml
[workspace]
members = [
    "crates/wifi-densepose-core",
    "crates/wifi-densepose-signal",
    "crates/wifi-densepose-vitals",
    "crates/wifi-densepose-hardware",
    "crates/wifi-densepose-nn",
    "crates/wifi-densepose-mat",
    "crates/wifi-densepose-config",
    "crates/wifi-densepose-sensing-server",
    "crates/wifi-densepose-llm",           # ← 新增
]
```

---

## 十二、Rust Crate 结构

```
crates/wifi-densepose-llm/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # crate 入口, re-exports
│   ├── config.rs                 # LlmConfig 配置结构
│   ├── patient_record.rs         # PatientRecord + PatientRecordDB
│   ├── medical_knowledge.rs      # MedicalKnowledgeBase + 匹配算法
│   ├── sliding_window.rs         # SlidingWindow + VitalTrendSummary
│   ├── prompt_builder.rs         # build_analysis_prompt()
│   ├── streaming.rs              # StreamingGenerator
│   ├── engine.rs                 # LlmAnalysisEngine (协调器)
│   └── fallback.rs               # fallback_analysis (L2降级)
└── data/
    ├── medical_knowledge.json    # 方舱常见伤病知识库
    └── system_prompt.txt         # 系统提示词模板
```

---

## 十三、安全设计总结

| 安全措施 | 说明 |
|------|------|
| **双层架构** | 规则引擎是硬保底层，LLM 只做增强 |
| **不可覆盖** | LLM 分析结果不可修改 START 分诊等级 |
| **显式免责** | 所有 LLM 输出标注"仅供参考，最终决策由医护人员做出" |
| **3级降级** | LLM 故障时自动回退到模板分析，不丢功能 |
| **全本地推理** | 模型+数据均在 RZ/V2H 本地，无网络依赖，无隐私泄露 |
| **超时保护** | 单次推理超时 120s 自动中断，回退到 L2 |
| **输出校验** | JSON 解析失败时使用 fallback，不崩溃 |

---

*文档版本: v2.1 | 2026-05-17*
*变更: 从"报告生成器"升级为"智能分析引擎"，增加 RAG、病史、多数据联合分析、流式输出*
