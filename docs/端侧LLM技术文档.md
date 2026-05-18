# 端侧 LLM 智能分析引擎 — 接口·技术·功能·原理

> 项目：WCES — 基于WiFi CSI感知与端侧LLM的方舱生命体征感知与监护系统
> 模块：P10d — wifi-densepose-llm
> 文档版本：v1.1 | 2026-05-17

---

## 一、概述

端侧 LLM 智能分析引擎是 WCES 系统的人工智能分析核心。它将 LLM 的定位从传统的"报告生成器"升级为"智能分析引擎"——不只做数据到文本的格式化输出，而是深度参与医学数据分析：

- **结合患者既往病史**进行个性化风险评估
- **多体征联合推断**（呼吸率 + 心率 + 运动状态 + 骨架姿态 + 边缘模块告警）
- **疾病特征匹配**（体征模式 vs 结构化医学知识库）
- **时序趋势感知**（短/中/长三窗口滑动统计）
- **优先级细化**（在 START 规则引擎基础上提供第二层风险评估）

引擎运行在瑞萨 RZ/V2H 主控（ARM64 Cortex-A55 ×4, 8GB RAM）上，使用 INT4 量化的 Qwen2.5-0.5B 模型进行全本地推理，无数据出舱，保障方舱医院隐私安全。

---

## 二、技术架构

### 2.1 双层安全架构

```
┌─────────────────────────────────────────────────────────────┐
│  L1 规则引擎层 (硬安全边界)                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ START分诊 + 19个边缘模块告警                            │  │
│  │ · 延迟 < 50ms                                          │  │
│  │ · 绝不依赖 LLM                                          │  │
│  │ · LLM 输出不可覆盖此层决策                              │  │
│  └───────────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│  L2 LLM 增强分析层 (辅助增强)                                │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 病史检索 + 知识匹配 + 趋势分析 + LLM推理 + 流式输出    │  │
│  │ · 异步触发 (事件/周期/手动)                            │  │
│  │ · 30-120 秒完成                                        │  │
│  │ · 三级降级保障                                         │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Rust Crate 结构

```
crates/wifi-densepose-llm/
├── Cargo.toml                    # 依赖: sled, candle 0.8, tokenizers, tokio
├── data/
│   └── medical_knowledge.json    # 8种方舱伤病知识条目
├── src/
│   ├── lib.rs                    # 模块入口 + re-exports
│   ├── config.rs                 # LlmConfig (3种预设配置)
│   ├── types.rs                  # 公共类型 (StreamToken/LlmGenerationResult)
│   ├── patient_record.rs         # PatientRecord + sled 嵌入式病历DB
│   ├── medical_knowledge.rs      # MedicalKnowledgeBase + 4维匹配算法
│   ├── sliding_window.rs         # 滑动窗口趋势分析器
│   ├── prompt_builder.rs         # LLM Prompt 上下文组装器
│   ├── fallback.rs               # L2 模板回退分析 (完整规则管线)
│   ├── engine.rs                 # LlmAnalysisEngine 总协调器
│   └── streaming.rs              # Qwen2 GGUF 流式推理引擎 (llm feature)
└── tests/
    └── integration.rs            # 10个端到端集成测试
```

---

## 三、技术选型

### 3.1 模型：Qwen2.5-0.5B-Instruct

| 维度 | 选型理由 |
|------|----------|
| **参数量** | 0.5B — RZ/V2H 8GB RAM 可舒适加载 |
| **量化** | INT4 Q4_K_M (~380MB 磁盘, ~500MB RAM) |
| **中文能力** | ⭐⭐⭐ 阿里出品，原生中文训练 |
| **医疗适配** | Instruct 微调版本，指令遵循能力强 |
| **格式** | GGUF — llama.cpp 生态标准，candle 原生支持 |

### 3.2 推理框架：candle 0.8

| 维度 | 选型理由 |
|------|----------|
| **语言** | 纯 Rust — 与 sensing-server 同一 `cargo build` |
| **依赖** | 零 C++ 依赖 — aarch64 交叉编译无需 C++ 工具链 |
| **量化** | GGUF 原生支持，INT4/INT8 量化 |
| **大小** | ~5MB 编译产物 |
| **API** | `candle_transformers::models::quantized_qwen2::ModelWeights` |

### 3.3 嵌入式数据库：sled

| 维度 | 选型理由 |
|------|----------|
| **语言** | 纯 Rust |
| **依赖** | 零配置，无服务端进程 |
| **性能** | 嵌入式 KV 存储，内存映射文件 |
| **用途** | 病历数据库 (Patient ID → 既往病史/主诉/用药) |

### 3.4 技术对比

| 组件 | 方案A (采用) | 方案B (未采用) | 不选理由 |
|------|:---:|:---:|------|
| 模型 | Qwen2.5-0.5B | Phi-3-mini / Gemma-2 | 中文能力差 / RAM 超标 |
| 框架 | candle | llama.cpp / onnxruntime | 需 C++ 交叉编译 |
| 数据库 | sled | SQLite / PostgreSQL | 过度 / 需外部进程 |
| 知识注入 | RAG 外挂 | 模型内化 | 0.5B 无法内化医学知识 |

---

## 四、核心功能模块

### 4.1 病历管理 (PatientRecordDB)

**功能**：嵌入式存储伤员基本信息与病史，支持 CRUD 操作。

**数据结构**：
```
PatientRecord {
    patient_id: "PAT-0001",          // 唯一标识
    age: 68,                          // 年龄
    gender: Male,                     // 性别
    pre_existing: ["COPD", "高血压"], // 既往病史
    chief_complaint: "呼吸困难3小时", // 当前主诉
    medications: ["沙美特罗替卡松"],   // 长期用药
    allergies: ["青霉素"],            // 过敏史
    node_id: 2,                       // 关联ESP32节点
    admission_time: UTC时间戳,        // 入舱时间
}
```

**API**：
| 方法 | 说明 |
|------|------|
| `put(record)` | 存储/更新病历 |
| `get(patient_id)` | 按ID查询病历 |
| `get_by_node_id(node_id)` | 按ESP32节点ID反查 |
| `list_all()` | 列出全部伤员 |
| `delete(patient_id)` | 删除病历 |

### 4.2 医学知识库 (MedicalKnowledgeBase)

**功能**：8种方舱常见伤病知识条目，通过 RAG（检索增强生成）注入 LLM prompt，弥补 0.5B 模型医学知识不足。

**知识条目**：
| 条目 | 紧急度 | 用途 |
|------|:---:|------|
| 急性呼吸衰竭 | critical | 呼吸率 >30 或 <10，趋势恶化 |
| 脓毒症(早期) | high | 心率+呼吸率同步上升，早期识别 |
| 急性心律失常 | critical | 心率突然大幅变化 |
| COPD急性加重 | high | COPD患者呼吸恶化，基线偏移检测 |
| 癫痫发作 | high | 运动评分突增→骤降模式 |
| 急性心脏事件 | critical | 心率突异常+运动静止 |
| 低血容量性休克 | critical | 心率代偿升高+运动逐渐减少 |
| 急性焦虑/恐慌 | moderate | 排除器质性疾病后的过度换气 |

**匹配算法**（4维评分）：
```
总分 = 体征匹配(40%) + 趋势匹配(20%) + 风险因子匹配(25%) + 边缘告警匹配(15%)

体征匹配: 当前 RR/HR 是否在疾病的指示范围内
趋势匹配: RR/HR 变化方向是否符合疾病特征
风险因子匹配: 患者既往病史 vs 疾病已知风险因子
边缘告警匹配: 活跃的模块告警 vs 疾病关联的告警类型
```

**API**：
| 方法 | 说明 |
|------|------|
| `load(path)` | 从 JSON 加载知识库 |
| `match_conditions(input, top_n)` | 多维度匹配，返回 Top-N 疾病条目+得分 |

### 4.3 趋势分析器 (SlidingWindow)

**功能**：三个时间窗口并行维护每个伤员的体征时序数据，压缩为统计摘要后送入 LLM，避免原始 token 爆炸。

**窗口配置**：
| 窗口 | 时长 | 用途 |
|------|:--:|------|
| 短期 (Short) | 1 分钟 | 即时告警的二阶确认 |
| 中期 (Medium) | 5 分钟 | **标准分析窗口**（主要使用） |
| 长期 (Long) | 30 分钟 | 基线对比（如 COPD 患者偏离基线） |

**统计输出**：
```
VitalTrendSummary {
    rr_mean: 31.2, rr_trend: Rising, rr_change_pct: +15.5%, rr_volatility: 0.12
    hr_mean: 105.0, hr_trend: Rising, hr_change_pct: +10.0%, hr_volatility: 0.08
    motion_pattern: IntermittentMotion | SpikeAndDrop | GradualDecline | ...
    signal_quality_mean: 0.85, signal_quality_trend: Stable
}
```

**检测能力**：
- 线性回归斜率 → 趋势方向 (Rising/Stable/Falling)
- 前后半均值比 → 变化百分比
- 变异系数 (CV) → 波动程度
- 运动模式分类 → ContinuousStill / IntermittentMotion / ContinuousMotion / SpikeAndDrop / GradualDecline

### 4.4 Prompt 组装器 (PromptBuilder)

**功能**：将病史、知识匹配、趋势摘要、当前体征组装为结构化 LLM prompt。

**Prompt 结构**：
```
# 角色 - 方舱医院急救医师
# 伤员基本信息 - ID/年龄/性别/既往病史/主诉/用药
# 当前体征数据 - 呼吸率/心率/信号质量/运动状态/START分诊
# 趋势分析 - 过去5分钟的RR/HR均值/趋势/波动/运动模式
# 活跃告警 - 当前触发的边缘模块告警
# 参考知识 - 匹配到的疾病条目+关键指标+风险因子+建议行动
# 分析要求 - 输出结构化JSON (风险评估/体征匹配/分诊意见/趋势/病史/建议)
```

**输出格式**（JSON）：
```json
{
  "risk_assessment": { "overall_level": "critical", "primary_concern": "...", "deteriorating": true },
  "condition_match": { "most_likely": "COPD急性加重", "confidence": "high" },
  "triage_opinion": { "agrees_with_start": false, "suggested_level": "Immediate", "reason": "..." },
  "trend_analysis": { "respiratory": "...", "cardiac": "...", "combined": "..." },
  "history_relevance": { "relevant_conditions": ["COPD"], "impact_on_assessment": "..." },
  "recommendations": ["立即气道评估", "控制性氧疗", "..."],
  "monitoring_priority": ["呼吸频率变化趋势", "..."]
}
```

### 4.5 模板回退分析 (FallbackAnalyzer)

**功能**：当 LLM 不可用时（模型未加载/超时/加载失败），使用规则驱动的分析管线生成结构化分析输出。输出格式与 LLM 完全一致，前端无需差别处理。

**规则逻辑**：
- **综合风险评估**：START分诊等级 + 恶化趋势 + 高置信度匹配 → critical/high/moderate/low
- **主要担忧**：高得分疾病 > 异常体征 > 趋势警告
- **恶化检测**：RR ↑>10% 或 HR ↑>10% 或 运动评分SpikeAndDrop/GradualDecline
- **分诊意见**：只做升级建议，不做降级（安全红线）
- **处置建议**：匹配疾病行动 > 趋势建议 > 通用分诊建议

### 4.6 流式推理引擎 (StreamingGenerator)

**功能**：加载 Qwen2.5-0.5B GGUF 模型，逐 token 生成分析文本，通过 broadcast channel 推送给 WebSocket → 前端逐字渲染。

**技术实现**：
- `candle_core::quantized::gguf_file::Content::read()` — 解析 GGUF 文件
- `candle_transformers::models::quantized_qwen2::ModelWeights::from_gguf()` — 构建量化模型
- `candle_transformers::generation::LogitsProcessor` — 温度采样 (temperature=0.3)
- `tokenizers::Tokenizer` — Qwen2 tokenizer (vocab ~152K)
- EOS 检测：`<|im_end|>` token (id=151643)

**性能估算**（RZ/V2H Cortex-A55）：
| 指标 | 数值 |
|------|------|
| 模型加载 | ~5-8 秒 |
| 推理速度 | ~500ms/token |
| 100 token 分析 | ~50 秒 |
| 200 token 详细分析 | ~100 秒 |
| RAM 占用 | ~500MB |

### 4.7 协调引擎 (LlmAnalysisEngine)

**功能**：将所有模块统一为分析管线，提供对外 API。

**核心 API**：
| 方法 | 说明 |
|------|------|
| `new(config)` | 创建引擎，自动加载病历/知识库/LLM模型 |
| `register_patient(record)` | 登记伤员信息 |
| `push_vitals(node_id, rr, hr, motion, sq)` | 喂入体征数据（自动映射节点→患者） |
| `trigger_analysis(patient_id, ...)` | 触发同步分析（返回完整结果） |
| `trigger_analysis_streaming(patient_id, ...)` | 触发流式分析（返回 broadcast::Receiver） |
| `build_prompt(patient_id, ...)` | 构建 LLM prompt（调试用） |
| `status()` | 引擎状态（患者数/知识条目/LLM加载状态） |

---

## 五、接口定义

### 5.1 触发策略

| 触发类型 | 条件 | 延迟 | 用途 |
|----------|------|:--:|------|
| 🔴 事件触发 | START分诊等级变化 (绿→黄/黄→红) | 即时 | 第一时间评估恶化 |
| 🔴 事件触发 | 边缘模块新告警 (呼吸窘迫/心律失常/癫痫...) | 即时 | 对告警做医学解读 |
| 🟡 周期触发 | 每 30-60 秒定时器 | 周期 | 趋势分析 + 稳定伤员复查 |
| 🟢 手动触发 | 医护人员点击 triage.html 卡片 | 即时 | 按需深度分析 |

### 5.2 WebSocket 消息类型（新增）

#### 客户端 → 服务端

**触发 LLM 分析**：
```json
{
  "type": "llm_analyze_request",
  "patient_id": "SURV-0001",
  "node_id": 2
}
```

**登记伤员**：
```json
{
  "type": "patient_register",
  "patient_id": "PAT-0001",
  "age": 68,
  "gender": "male",
  "node_id": 2,
  "pre_existing": ["COPD", "高血压"],
  "chief_complaint": "呼吸困难3小时",
  "medications": ["沙美特罗替卡松 50/250μg bid"],
  "allergies": ["青霉素"],
  "notes": ""
}
```

#### 服务端 → 客户端

**流式 token**（逐字推送）：
```json
{
  "type": "llm_stream",
  "survivor_id": "SURV-0001",
  "token_index": 15,
  "text": "呼吸",
  "is_complete": false
}
```

**分析完成**（完整 JSON 结果）：
```json
{
  "type": "llm_analysis_complete",
  "survivor_id": "SURV-0001",
  "analysis_json": { /* 完整分析结果 */ },
  "generated_by": "llm",
  "generated_tokens": 187,
  "elapsed_ms": 93500
}
```

**模板回退**（LLM 不可用时的回退结果）：
```json
{
  "type": "llm_fallback",
  "patient_id": "SURV-0001",
  "analysis_json": { /* 完整分析结果 */ },
  "generated_by": "fallback_template",
  "analysis_time_ms": 0
}
```

### 5.3 HTTP API（已集成到 sensing-server ✅）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/patients` | 列出所有伤员 |
| POST | `/api/v1/patients` | 创建/更新伤员记录 |
| POST | `/api/v1/llm/analyze` | 手动触发 LLM 流式分析 (WebSocket upgreade) |
| GET | `/api/v1/llm/status` | LLM 引擎状态 (患者数/知识条目/LLM加载) |

---

## 六、工作原理

### 6.1 完整数据流

```
ESP32-C5 ×3
  │ UDP:5005 (CSI 数据帧)
  ▼
sensing-server (RZ/V2H)
  │
  ├─→ VitalSignDetector → 呼吸率/心率
  ├─→ TriageEngine → START分诊 (红/黄/绿/黑)
  ├─→ EdgeModuleEngine → 19个模块告警
  │
  │   ┌─── 每帧推送 (所有数据) ────────────────────────┐
  │   ▼                                                ▼
  │  SlidingWindow.push()                      WebSocket /ws/sensing
  │   (维护3窗口趋势)                             → triage.html (实时)
  │
  ├─→ [事件/周期/手动触发]
  │   │
  │   ├─ PatientRecordDB.get()          → 检索病史
  │   ├─ MedicalKnowledgeBase.match()   → 疾病匹配 (Top-3)
  │   ├─ SlidingWindow.medium_summary() → 趋势摘要
  │   ├─ PromptBuilder.build()          → 组装 LLM Prompt
  │   │
  │   ├─ [LLM 可用?]
  │   │   YES → StreamingGenerator.generate_stream()
  │   │          └→ token → broadcast::channel
  │   │                       └→ WebSocket llm_stream 事件
  │   │                            → triage.html AI卡片 (逐字)
  │   │
  │   │   NO  → FallbackAnalyzer.analyze()
  │   │          └→ LlmAnalysisResult
  │   │               └→ WebSocket llm_fallback 事件
  │   │                    → triage.html AI卡片 (即时)
  │
  └─→ triage.html
       ├─ 实时面板: 伤员卡片/统计/告警/边缘事件/3D骨架
       ├─ AI分析卡片: 流式文本→结构化JSON渲染
       └─ 伤员登记: 模态框→localStorage+WebSocket
```

### 6.2 触发流程示例

```
T=0s    伤员 SURV-0001 入舱, START=Delayed (黄)
T=5s    → 事件触发 (新伤员入舱)
         LLM: "生命体征正常, START黄区判定合理"
T=30s   → 周期触发
         LLM: "体征稳定, 无恶化趋势"
T=60s   → 周期触发
         LLM: "呼吸率16→22(↑37.5%), 心率72→95(↑32%),
               COPD病史需关注, 建议升级监测频率"
T=75s   呼吸率突破30 → START=Immediate (红)
T=75s   → 事件触发 (分诊等级变化!)
         LLM: "⚠️ 呼吸率>30, COPD病史+持续上升,
               符合急性呼吸衰竭早期, 建议立即气道评估+氧疗"
```

### 6.3 三级降级策略

```
L1 正常模式
  ├─ 条件: LLM 模型加载成功 + 推理正常
  ├─ 输出: 完整 LLM 推理分析
  └─ 延迟: 30-120s

L2 回退模式
  ├─ 条件: LLM 推理超时(>120s) 或 模型未加载
  ├─ 输出: 模板规则分析 (结构化数据摘要+规则推断)
  └─ 延迟: <1ms

L3 最小模式
  ├─ 条件: LLM 加载失败
  ├─ 输出: 无 AI 分析卡片，仅显示规则引擎结果
  └─ 保底: START分诊+边缘告警 完全不受影响
```

---

## 七、安全设计

| 安全措施 | 实现方式 |
|----------|----------|
| **双层架构** | L1 规则引擎 (START分诊) 是硬保底层，L2 LLM 只做增强分析 |
| **不可覆盖** | LLM 分析结果永远不修改 START 分诊等级，只提供"第二意见" |
| **显式免责** | 所有 LLM 输出标注"仅供参考，最终决策由医护人员做出" |
| **3级降级** | LLM 故障时自动回退到模板分析，核心分诊功能不丢失 |
| **全本地推理** | 模型+数据均运行在 RZ/V2H 本地，无网络依赖，无隐私泄露风险 |
| **超时保护** | 单次推理超时 120s 自动中断，回退到 L2 |
| **输出校验** | JSON 解析失败时使用 fallback，不崩溃 |
| **冷却机制** | 同一伤员 30s 内不重复触发分析，防止资源占用 |

---

## 八、使用方式

### 8.1 配置

```toml
# wces.config.toml
[server.llm]
enabled = true
feature = "llm"                        # llm | template-only
model_path = "data/models/qwen2.5-0.5b-q4.gguf"
tokenizer_path = "data/models/qwen2_tokenizer.json"
max_tokens = 256
temperature = 0.3
periodic_analysis_secs = 30
fallback_to_template = true
patient_db_path = "data/patients"
medical_kb_path = "data/medical_knowledge.json"
```

### 8.2 编译

```bash
# 模板模式 (无需模型文件，初赛可用)
cargo build -p wifi-densepose-llm

# 完整 LLM 模式 (需要下载模型和 tokenizer)
cargo build -p wifi-densepose-llm --features llm
```

### 8.3 Rust API 使用

```rust
use wifi_densepose_llm::{LlmAnalysisEngine, LlmConfig, PatientRecord};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建引擎
    let engine = LlmAnalysisEngine::new(LlmConfig::competition()).await?;

    // 登记伤员
    let mut patient = PatientRecord::new("PAT-0001");
    patient.age = Some(68);
    patient.pre_existing = vec!["COPD".into()];
    patient.node_id = Some(2);
    engine.register_patient(patient).await?;

    // 喂入体征数据
    engine.push_vitals(2, 32.0, 115.0, 0.3, 0.85).await;

    // 流式分析
    let mut rx = engine.trigger_analysis_streaming(
        "PAT-0001", Some(32.0), Some(115.0),
        0.3, 0.85, "Immediate", &[]
    ).await.unwrap();

    while let Ok(token) = rx.recv().await {
        if token.is_complete { break; }
        print!("{}", token.text); // 逐字输出
    }

    Ok(())
}
```

### 8.4 前端使用 (triage.html)

1. 点击 **"+ 登记伤员"** 按钮打开登记表单
2. 填写伤员 ID、年龄、性别、既往病史、主诉等信息
3. 伤员数据保存到 localStorage 并通过 WebSocket 发送到服务端
4. 选择伤员卡片后点击 **"生成AI分析报告"** 按钮
5. AI 分析卡片实时显示流式生成的分析文本
6. 生成完成后渲染为结构化分析卡片（风险评估/体征匹配/分诊意见/处置建议）

---

## 九、性能与资源

### 9.1 内存预算 (RZ/V2H 8GB)

```
Linux OS:                   ~1.5GB
sensing-server (Rust):      ~500MB   (CSI缓冲区 + MAT + Edge Modules)
Qwen2.5-0.5B (INT4):        ~500MB   (模型权重 + KV Cache)
PatientRecordDB (sled):     ~50MB    (嵌入式KV存储)
MedicalKnowledge KB:        ~5MB     (JSON知识库)
SlidingWindow buffers:      ~20MB    (每伤员3窗口)
WebSocket + UI assets:      ~200MB
─────────────────────────────────
空闲:                       ~5.2GB   ✅ 充足
```

### 9.2 测试覆盖

| 测试类型 | 数量 | 覆盖 |
|:---:|:---:|------|
| 单元测试 | 14 | 所有模块核心逻辑 |
| 集成测试 | 10 | 端到端分析管线 |
| 编译验证 | 2 | template-only / llm 两个 feature |
| **合计** | **24** | 0 失败 |

---

## 十、与同类方案对比

| 维度 | 本方案 | 传统云LLM方案 | 纯规则引擎方案 |
|------|:---:|:---:|:---:|
| **部署位置** | RZ/V2H 本地 | 云服务器 | RZ/V2H 本地 |
| **隐私安全** | ✅ 全本地，无数据出舱 | ❌ 数据上传云端 | ✅ 全本地 |
| **网络依赖** | ✅ 离线可用 | ❌ 需稳定网络 | ✅ 离线可用 |
| **医学知识** | ✅ RAG外挂知识库 | ✅ 模型内化 | ❌ 规则硬编码 |
| **分析深度** | ✅ 多维度联合推断 | ✅ 深度分析 | ⚠️ 仅规则覆盖范围 |
| **推理延迟** | 30-120s (0.5B) | 2-5s (云端GPU) | <1ms |
| **灵活性** | ✅ 更新知识库即可 | ⚠️ 需重新部署 | ❌ 修改代码 |
| **成本** | ✅ 零 API 费用 | ❌ 按 token 计费 | ✅ 零费用 |

---

*文档版本: v1.0 | 2026-05-15*
*对应实现: wifi-densepose-llm crate (P10d), triage.html AI卡片, wces.config.toml*
