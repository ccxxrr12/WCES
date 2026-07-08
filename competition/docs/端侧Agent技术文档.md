# 端侧医疗 Agent 技术文档

## 1. 概述

端侧医疗 Agent 是 WCES 方舱医院生命体征监测系统的 AI 分析核心，运行在 RZ/G2L 边缘网关（2×A55 @1.2GHz + M33 @200MHz, 1GB DDR4）上。由于边缘设备算力受限（~6.4 DMIPS，约 1/8 树莓派 4），无法运行本地 LLM，因此采用 **Coordinator 模式**：RZ/G2L 负责信号处理 + 规则分诊，深度分析通过 OpenAI 兼容 API 卸载到云端。

### 核心设计原则

| 原则 | 描述 |
|------|------|
| **硬件感知** | 不依赖本地 LLM/embedding/向量搜索，端侧只做轻量规则匹配 |
| **优雅降级** | 网络中断或 API 故障时自动回退到本地模板分析，不丢失分析能力 |
| **流式响应** | 支持 SSE（Server-Sent Events）流式传输，逐 token 推送到 UI |
| **安全第一** | 输出校验拦截危险内容（药物/手术建议），强制附加 AI 免责声明 |
| **第二意见** | LLM 分析结果为"第二意见"（second opinion），不自动修改 START 分诊结果 |

---

## 2. 架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                     sensing-server (tokio + axum)                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ UDP Rx   │  │ WebSocket│  │ REST API │  │ Periodic Task │  │
│  │ (ESP32)  │  │ (/ws)    │  │ (/api)   │  │ (every 30s)   │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └───────┬───────┘  │
│       │              │              │                │           │
│       │    TriageEngine.process()   │                │           │
│       │    ──→ 分诊变化? ──→ Agent 触发               │           │
│       │              │              │                │           │
│       └──────────────┴──────────────┴────────────────┘           │
│                              │                                   │
│                     StructuredContext                            │
│                              │                                   │
└──────────────────────────────┼───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                     MedicalAgent (wifi-densepose-llm)            │
│                                                                  │
│  StructuredContext                                               │
│       │                                                          │
│  ┌────▼─────────────┐                                            │
│  │ 1. Degradation    │  冷却检查 / 网络检查 / 熔断检查              │
│  │    Manager        │                                            │
│  └────┬─────────────┘                                            │
│       │ degrade_level                                            │
│  ┌────▼─────────────┐                                            │
│  │ 2. AnalysisRouter │  分诊等级 + 恶化状态 + 网络 → RouteDecision │
│  │    (pure fn)      │                                            │
│  └────┬─────────────┘                                            │
│       │                                                          │
│  ┌────▼──────────────────────────────────────────────────────┐  │
│  │ 3. 路由分发                                               │  │
│  │                                                           │  │
│  │  DeepLLM ──→ PromptCompiler ──→ LlmGateway ──→ SSE Stream │  │
│  │  BriefLLM ──→ PromptCompiler ──→ LlmGateway ──→ SSE Stream│  │
│  │  TemplateWithKB ──→ FallbackAnalyzer + MedicalKnowledgeBase│  │
│  │  TemplateOnly ──→ 基础模板                                 │  │
│  │  CachedReplay ──→ 缓存回放                                 │  │
│  │  Skip ──→ 空响应                                          │  │
│  └────┬──────────────────────────────────────────────────────┘  │
│       │                                                          │
│  ┌────▼─────────────┐                                            │
│  │ 4. OutputValidator│  空响应检查 / 危险内容拦截 / 免责声明         │
│  └────┬─────────────┘                                            │
│       │                                                          │
│  ┌────▼─────────────┐                                            │
│  │ 5. RiskAdjustment │  正则提取 [分诊建议: 升级/维持/降级, XX%]    │
│  │    Extractor      │                                            │
│  └────┬─────────────┘                                            │
│       │                                                          │
│       ▼                                                          │
│  AnalysisResult ──→ WebSocket Broadcast ──→ UI                   │
│                                                                  │
│  ┌──────────────────┐                                            │
│  │ 6. Degradation    │  记录结果 / 缓存 / 冷却                    │
│  │    Manager        │                                            │
│  └──────────────────┘                                            │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. 核心数据结构

### 3.1 StructuredContext（分析输入）

一次分析请求的完整上下文快照：

```rust
pub struct StructuredContext {
    pub patient_id: u32,                         // 伤员 ID
    pub node_id: u8,                             // ESP32 节点 ID
    pub vitals_current: AgentVitalSnapshot,       // 当前生命体征
    pub vitals_trend_1min: TrendSummary,          // 1 分钟趋势
    pub vitals_trend_5min: TrendSummary,          // 5 分钟趋势
    pub triage_current: String,                   // 当前分诊等级
    pub triage_trajectory: Vec<TriageStep>,       // 分诊变化历史
    pub patient_history: Option<PatientHistory>,  // 病史
    pub recent_alerts: Vec<String>,               // 近期边缘告警
    pub kb_matches: Vec<KbMatchResult>,           // 知识库匹配结果
    pub triggered_by: TriggerSource,              // 触发来源
    pub built_at_ms: u64,                         // 快照时间戳
}
```

### 3.2 TriggerSource（触发来源）

```rust
pub enum TriggerSource {
    Deterioration { patient_id: u32, from: String, to: String },  // 分诊恶化
    NewPatient { patient_id: u32 },                                // 新伤员入院
    ManualRequest { patient_id: u32 },                             // 手动请求
    PeriodicScan,                                                   // 定时巡检
}
```

### 3.3 AnalysisResult（分析输出）

```rust
pub struct AnalysisResult {
    pub patient_id: u32,
    pub text: String,                              // 分析文本
    pub risk_adjustment: Option<RiskAdjustment>,    // 分诊第二意见
    pub source: AnalysisSource,                    // 来源标识
    pub degrade_level: DegradationLevel,           // 降级等级
    pub generated_at_ms: u64,                      // 生成时间
}

pub struct RiskAdjustment {
    pub direction: AdjustDirection,   // Escalate / Maintain / Deescalate
    pub confidence: f32,              // 0.0 - 1.0
    pub reason_short: String,         // 简短理由
    pub detail: String,               // 详细说明
}
```

---

## 4. 路由策略

`AnalysisRouter::decide()` 是一个纯函数，基于 4 个输入决定 6 种路由：

| 分诊 | 恶化 | 网络 | 冷却 | 路由 | Token 限制 | 优先级 |
|------|------|------|------|------|-----------|--------|
| Deceased/Unknown | * | * | * | **Skip** | 0 | 0 |
| * | * | * | ✓ | **CachedReplay** | 0 | 0 |
| * | * | ✗ | ✗ | **TemplateWithKB** | 0 | 1 |
| Immediate/Red | ✓ | ✓ | ✗ | **DeepLLM** | 300 | 3 |
| Immediate/Red | ✗ | ✓ | ✗ | **BriefLLM** | 150 | 2 |
| Delayed/Yellow | ✓ | ✓ | ✗ | **DeepLLM** | 300 | 3 |
| Delayed/Yellow | ✗ | ✓ | ✗ | **BriefLLM** | 150 | 2 |
| Minor/Green | * | ✓ | ✗ | **TemplateWithKB** | 0 | 1 |

### 路由说明

- **DeepLLM**：深度分析，300 token，包含临床发现、鉴别诊断、监测要点。用于恶化中的 Immediate/Delayed 伤员
- **BriefLLM**：简要分析，150 token，仅输出主要发现 + 分诊建议 + 1-2 条监测要点。用于稳定的 Immediate/Delayed 伤员
- **TemplateWithKB**：本地模板 + 知识库匹配。Minor/Green 伤员默认路由，也是断网时的降级路径
- **TemplateOnly**：纯模板，无 KB 增强。最高级别的降级
- **CachedReplay**：冷却期内直接返回上次缓存结果，避免重复 API 调用
- **Skip**：Deceased/Unknown 直接跳过

---

## 5. 降级阶梯

五级降级：网络中断或 API 连续失败时自动逐级下移，网络恢复后自动回升。

```
L0 FullLLM         ← 云端深度分析（默认）
 │  连续失败 3 次 → 熔断器打开
 │
L1 BriefLLM        ← 云端简要分析（L0 的子集，更少 token）
 │  网络不可达 或 熔断器开
 │
L2 TemplateWithKB  ← 本地规则分析 + 医学知识库匹配（8 种疾病模式）
 │  5 次连续失败
 │
L3 TemplateOnly    ← 纯模板（无 KB），最低保障输出
 │  冷却期命中
 │
L4 CachedReplay    ← 缓存回放（30 秒内同一伤员不重复分析）
```

### 降级触发条件

| 条件 | 动作 |
|------|------|
| API 超时 / 网络错误 | `on_failure()` → 失败计数 +1 |
| 连续失败 ≥ 3 | `CircuitBreaker` 打开 → 降级到 L2 |
| 连续失败 ≥ 5 | `DegradationManager` 标记网络不可达 → 降级到 L2 |
| `on_network_change(true)` | 失败计数归零，恢复正常 |
| 同一伤员 300 秒内重复请求 | 命中冷却 → L4 缓存回放 |

---

## 6. 各组件设计

### 6.1 LlmGateway + CircuitBreaker

OpenAI 兼容的 SSE 流式网关，带熔断保护。

```
┌──────────────┐     HTTP POST     ┌─────────────┐
│  LlmGateway  │ ────────────────→ │  LLM API    │
│              │ ←── SSE Stream ── │  (OpenAI)    │
│  .stream()   │                   └─────────────┘
│  .complete() │
└──────┬───────┘
       │
┌──────▼───────┐
│CircuitBreaker│   状态机: Closed → Open → HalfOpen → Closed
│              │   阈值: 3 次失败 → Open (300s)
│  .check()    │   HalfOpen: 测试一次 → 成功 Close / 失败 Open
│  .on_success()│
│  .on_failure()│
└──────────────┘
```

**SSE 解析流程**：
1. `reqwest` POST 到 `/chat/completions`，`stream: true`
2. `resp.bytes_stream()` → tokio `Stream<Item=Bytes>`
3. 按 `\n` 分割 buffer，提取 `data: {...}` 行
4. 跳过空行、注释行（`:` 开头）、`[DONE]`
5. 从 JSON 中解析 `choices[0].delta.content`
6. 通过 `tokio::sync::mpsc` channel 传递给调用方

### 6.2 PromptCompiler

三段式 prompt 编译：

```
┌─────────────┐   ┌───────────────┐   ┌──────────────┐
│ System      │   │ Context (JSON)│   │ Task         │
│ (固定角色)  │ + │ (序列化体征)  │ + │ (deep/brief) │
└─────────────┘   └───────────────┘   └──────────────┘
     │                  │                   │
     └──────────────────┴───────────────────┘
                        │
                   OpenAI API
              messages: [
                {role: "system", content: system},
                {role: "user", content: "context\n\ntask"}
              ]
```

**Token 优化**：Context 使用中文 JSON key（`心率`, `呼吸`, `分诊等级`...），比英文 key 节省约 15-20% token。

**模板文件**（位于 `data/prompts/`）：
- `system.txt` — 系统角色设定 + 约束（不输出药物/手术、附加 AI 声明）
- `deep_analysis.txt` — 深度分析任务（临床发现 + 鉴别诊断 + 分诊建议 + 监测要点）
- `brief_analysis.txt` — 简要分析任务（主要发现 + 分诊建议 + 监测要点）

### 6.3 OutputValidator

输出安全校验器，运行在 LLM 返回 → 推送到 UI 之间：

| 检查项 | 正则 | 动作 |
|--------|------|------|
| 空响应 | `trim().is_empty()` | FailAndFallback → 回退模板 |
| 药物建议 | `建议使用.{0,10}药` | 替换为 `[内容已拦截]` |
| 注射建议 | `推荐.{0,10}注射` | 替换为 `[内容已拦截]` |
| 剂量 | `剂量.{0,5}\d+mg` | 替换为 `[内容已拦截]` |
| 手术 | `手术\|切开\|缝合\|处方\|开具` | 替换为 `[内容已拦截]` |
| 缺免责声明 | 未包含 `[AI分析, 仅供参考]` | 自动附加 |

### 6.4 RiskAdjustmentExtractor

从 LLM 输出中提取分诊第二意见：

```
输入: "伤员心率持续上升...\n[分诊建议: 升级, 置信度: 78%]\n理由: 心动过速持续恶化"

正则: \[分诊建议:\s*(升级|维持|降级),\s*置信度:\s*(\d+)%\]

输出: RiskAdjustment {
    direction: Escalate,
    confidence: 0.78,
    reason_short: "理由: 心动过速持续恶化",
    detail: "伤员心率持续上升...\n[分诊建议: 升级, 置信度: 78%]\n..."
}
```

**注意**：RiskAdjustment 仅为"第二意见"（advisory only），**永远不会自动修改** START 分诊结果。分诊修改必须由医护人员在 UI 上手动确认。

### 6.5 MedicalKb

轻量级生命体征模式匹配引擎（替代向量搜索）：

```
JSON 条目示例:
{
  "id": "tachycardia",
  "condition": "心动过速",
  "vital_pattern": {
    "hr_range": [100.0, 180.0],
    "rr_range": null,
    "motion_states": ["present_still"]
  },
  "risk_factors": ["低血容量", "感染"],
  "triage_implication": "如持续 >10min，建议升级至 Delayed",
  "monitoring_notes": "每 5 分钟监测心率变化"
}
```

**匹配算法**：
1. 对每条 KB entry，计算 `score = matched_conditions / total_conditions`
2. 仅保留 `score >= 0.4` 的匹配结果
3. 按 score 降序排列，取 top-3

**性能**：16 条目 × 6 条件 = 96 次比较，< 0.1ms。

### 6.6 TemplateEngine

封装现有 `FallbackAnalyzer`，输出为序列化 JSON 的 `LlmAnalysisResult`：

```
FallbackContext → FallbackAnalyzer::analyze() → LlmAnalysisResult
                                                      │
                                          serde_json::to_string_pretty
                                                      │
                                               AnalysisResult
```

---

## 7. 与 sensing-server 集成

### 7.1 状态管理

```rust
// AppStateInner 中的 Agent 相关字段
struct AppStateInner {
    // ... 其他字段 ...
    medical_agent: Arc<tokio::sync::Mutex<MedicalAgent>>,  // Agent 实例
    medical_kb: MedicalKb,                                   // 知识库
}
```

`MedicalAgent` 包装在 `Arc<Mutex<>>` 中，因为其 `analyze()` 方法需要 `&mut self`。分析调用被串行化——在方舱场景下（少数伤员 + 30s 间隔）这是可以接受的。

### 7.2 触发路径

**路径 1 — 分诊恶化触发**（`tasks/udp_receiver.rs`）：
```
ESP32 CSI 帧 → TriageEngine.process() → 分诊等级变化?
                                            │ Yes
                                    TriggerSource::Deterioration
                                            │
                                    medical_agent.analyze(ctx)
                                            │
                                    WebSocket Broadcast
```

**路径 2 — 定时巡检**（`main.rs`）：
```
每 30s → 读取最新 vitals + triage + alerts
              │
       TriggerSource::PeriodicScan
              │
       medical_agent.analyze(ctx)
              │
       WebSocket Broadcast
```

**路径 3 — 手动请求**（WebSocket / REST）：
```
UI 点击"Agent 分析" → ws.send({type: "agent_analyze_request"})
                          │
                  TriggerSource::ManualRequest
                          │
                  medical_agent.analyze(ctx)
                          │
                  {"type": "agent_analysis", ...} → UI 渲染
```

### 7.3 WebSocket 消息格式

**agent_analysis 消息**（分析完成时广播）：
```json
{
  "type": "agent_analysis",
  "patient_id": 1,
  "text": "伤员心率 135bpm, 呼吸 32/min...",
  "source": "llm",
  "degrade_level": "L0FullLLM",
  "risk_adjustment": {
    "direction": "escalate",
    "confidence": 0.78,
    "reason_short": "心动过速持续恶化",
    "detail": "伤员心率持续上升...\n[分诊建议: 升级, 置信度: 78%]"
  },
  "generated_at_ms": 1716938400123
}
```

### 7.4 REST API

| 方法 | 路径 | 描述 |
|------|------|------|
| `POST` | `/api/v1/agent/analyze` | 手动触发 Agent 分析 |
| `GET` | `/api/v1/agent/status` | Agent 状态（熔断器状态等） |

---

## 8. 部署模式

### 8.1 云端 LLM 模式（`agent` feature，默认）

```bash
# 设置 API Key 启用云端 LLM
export LLM_API_KEY="sk-xxx"
export LLM_MODEL="gpt-4o-mini"          # 可选，默认 gpt-4o-mini
export LLM_ENDPOINT="https://api.openai.com/v1/chat/completions"  # 可选

cargo run -p wifi-densepose-sensing-server
```

### 8.2 模板模式（无需 API Key）

```bash
# 不设置 LLM_API_KEY 或设为空
unset LLM_API_KEY
cargo run -p wifi-densepose-sensing-server
# Agent 自动使用 new_template_only()，所有分析走本地模板
```

### 8.3 编译时禁用 agent feature

```toml
# 在 Cargo.toml 中
[dependencies]
wifi-densepose-llm = { version = "0.3.0", path = "../wifi-densepose-llm", default-features = false, features = ["template-only"] }
```

此时 `gateway`、`prompt`、`validator`、`risk_adjust` 模块都不会编译，二进制体积更小，零外部依赖。

---

## 9. 配置参考

### GatewayConfig

```rust
pub struct GatewayConfig {
    pub endpoint: String,          // 默认: env LLM_ENDPOINT 或 OpenAI
    pub model: String,             // 默认: env LLM_MODEL 或 gpt-4o-mini
    pub api_key: String,           // 默认: env LLM_API_KEY
    pub timeout_secs: u64,         // 默认: 20
    pub max_retries: u8,           // 默认: 2
    pub failure_threshold: u8,     // 默认: 3（熔断器阈值）
    pub breaker_open_secs: u64,    // 默认: 300（熔断恢复时间）
}
```

### DegradationManager 常量

| 常量 | 默认值 | 说明 |
|------|--------|------|
| `COOLDOWN_SECS` | 300 | 同一伤员冷却时间 |
| `MAX_CACHE_SIZE` | 32 | LRU 缓存上限 |
| 网络不可达阈值 | 5 次连续失败 | 触发 L2 降级 |

---

## 10. 测试覆盖

| 测试类别 | 数量 | 位置 |
|----------|------|------|
| CircuitBreaker 状态转换 | 3 | gateway.rs |
| SSE 解析 | 4 | gateway.rs |
| Router 路由决策 | 6 | router.rs |
| Degradation 降级 | 11 | degrade.rs |
| Validator 安全校验 | 4 | validator.rs |
| RiskAdjustment 提取 | 3 | risk_adjust.rs |
| MedicalKb 匹配 | 2 | medical_kb.rs |
| Agent 集成测试 | 5 | tests/integration.rs |
| **合计** | **38** | |

运行测试：
```bash
cargo test -p wifi-densepose-llm
```

---

## 11. 文件清单

```
rust-server/crates/wifi-densepose-llm/
├── src/
│   ├── agent.rs          # MedicalAgent 编排器（主入口）
│   ├── router.rs         # 路由决策（纯函数）
│   ├── degrade.rs        # 降级管理 + 缓存 + 冷却
│   ├── gateway.rs        # LLM API 网关 + 熔断器 + SSE 解析
│   ├── prompt.rs         # Prompt 编译器 + CompactContext
│   ├── validator.rs      # 输出安全校验
│   ├── risk_adjust.rs    # 分诊第二意见提取
│   ├── context.rs        # ContextCollator + 趋势计算
│   ├── template.rs       # 模板引擎（封装 FallbackAnalyzer）
│   ├── medical_kb.rs     # 体征模式匹配知识库
│   ├── lib.rs            # 模块声明 + 公开导出
│   ├── types.rs          # 共享类型定义
│   ├── config.rs         # LlmConfig
│   ├── engine.rs         # LlmAnalysisEngine（旧，保留兼容）
│   ├── fallback.rs       # FallbackAnalyzer（规则分析）
│   ├── medical_knowledge.rs  # MedicalKnowledgeBase（疾病匹配）
│   ├── patient_record.rs # 伤员档案
│   ├── prompt_builder.rs # PromptBuilder
│   └── sliding_window.rs # 滑动窗口 + 趋势检测
├── data/
│   ├── agent_kb.json     # Agent 知识库（16 种疾病模式）
│   ├── medical_knowledge.json  # 传统知识库
│   └── prompts/
│       ├── system.txt
│       ├── deep_analysis.txt
│       └── brief_analysis.txt
└── tests/
    └── integration.rs    # 集成测试
```
