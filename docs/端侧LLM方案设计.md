# 端侧 LLM 伤病报告生成 — 方案设计

> 项目：基于WiFi CSI感知与端侧LLM的方舱生命体征感知与监护系统
> 硬件：瑞萨 RZ/V2H (Cortex-A55 ×4, 8GB RAM, DRP-AI 可选)

---

## 一、为什么需要端侧LLM

方舱医院场景中，CSI 系统输出的是**数字**（呼吸:32次/分、心率:115BPM、分诊:Immediate），但医护人员需要的是**可读的判断**：
```
没有LLM → 显示屏：      呼吸:32/min  心率:115BPM  START:Immediate
有LLM   → 伤病报告：    "伤员呼吸急促(32次/分)，心率115BPM，符合START Immediate
                      分诊标准。建议立即进行气道评估，给予氧疗，优先转运红色区域。"
                      注意监测是否出现呼吸衰竭早期征象。"
```

---

## 二、技术选型

### 2.1 模型对比

| 模型 | 参数量 | INT4 大小 | RAM 占用 | 中文能力 | 推理速度 (A55) |
|------|:---:|:---:|:---:|:---:|:---:|
| **Qwen2.5-0.5B-Instruct** | 0.5B | ~380MB | ~500MB | ⭐⭐⭐ 原生中文 | ~500ms/token |
| Qwen2.5-1.5B-Instruct | 1.5B | ~1.0GB | ~1.3GB | ⭐⭐⭐ | ~1.5s/token |
| Phi-3-mini-4k | 3.8B | ~2.2GB | ~2.5GB | ❌ 英文为主 | ~3s/token |
| Gemma-2-2B | 2B | ~1.4GB | ~1.7GB | ⭐⭐ | ~2s/token |
| SmolLM2-1.7B | 1.7B | ~1.1GB | ~1.4GB | ❌ | ~1.5s/token |

**推荐：Qwen2.5-0.5B-Instruct** — 最轻量 + 原生中文 + 阿里出品

### 2.2 推理框架对比

| 框架 | 语言 | 依赖 | aarch64 | 量化 | 大小 |
|------|:---:|------|:---:|:---:|:---:|
| **candle** | Rust | **零 C++ 依赖** | ✅ | ✅ GGUF/GGML | ~5MB |
| llama.cpp | C++ | 需交叉编译 | ✅ | ✅ GGUF | ~2MB |
| onnxruntime | C++ | 需交叉编译 | ✅ | ✅ ONNX | ~20MB |
| mistral.rs | Rust | 需 C++ 编译链 | ✅ | ✅ GGUF | ~10MB |

**推荐：candle** — 纯 Rust，与现有 sensing-server 技术栈一致，交叉编译最简单（无需 C++ 工具链）。

### 2.3 模型获取

```bash
# 下载 Qwen2.5-0.5B-Instruct GGUF (Q4_K_M 量化)
wget https://huggingface.co/bartowski/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/Qwen2.5-0.5B-Instruct-Q4_K_M.gguf \
     -O models/qwen2.5-0.5b-q4.gguf
```

---

## 三、架构设计
```
CSI 生命体征            LLM 推理引擎              输出
─────────────    ──────────────────────    ──────────────────
呼吸: 32/min        │                      "伤员呼吸急促(32次/分)，
心率: 115BPM   ────►│  Qwen2.5-0.5B        心率115BPM，符合
分诊: Immediate     │  (INT4 量化)          START Immediate分诊
运动: 静止          │  Candle 推理          标准。建议立即进行
位置: (1.2, 0.8)    │  ARM64 CPU            气道评估..."
                    │
                    │  生成策略:
                    │  + 模板提示词
                    │  + 温度 0.3 (稳定)    → WebSocket 推送到
                    │  + max_tokens=256      triage dashboard
                    │  + 500ms-2s/次          "伤病报告"卡片
```

### 3.1 集成到 sensing-server

```rust
// crates/wifi-densepose-llm/src/lib.rs

use candle_core::Device;
use candle_transformers::models::qwen2::{Config, ModelForCausalLM};
use candle_nn::VarBuilder;
use tokenizers::Tokenizer;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct LlmReporter {
    model: ModelForCausalLM,
    tokenizer: Tokenizer,
    device: Device,
    /// 模板提示词 (方舱医院场景)
    system_prompt: String,
}

impl LlmReporter {
    /// 加载量化模型
    pub fn load(model_path: &str) -> Result<Self> {
        let device = Device::Cpu; // RZ/V2H ARM64
        
        // 1. 加载 GGUF 量化模型
        let mut file = std::fs::File::open(model_path)?;
        let model = candle_transformers::quantized::gguf::ModelWeights::from_gguf(&mut file)?;
        
        // 2. 构建 Qwen2 模型
        let vb = VarBuilder::from_gguf(&model, &device)?;
        let config = Config::qwen2_0_5b(); // 0.5B 参数
        let model = ModelForCausalLM::new(&config, vb)?;
        
        // 3. 加载 tokenizer
        let tokenizer = Tokenizer::from_file("models/qwen2_tokenizer.json")?;
        
        Ok(Self {
            model, tokenizer, device,
            system_prompt: Self::build_system_prompt(),
        })
    }
    
    /// 生成伤病报告
    pub fn generate_report(
        &self,
        vitals: &VitalSignsInput,
        triage: &str,
    ) -> Result<String> {
        let prompt = format!(
            "{}\n\n## 伤员生命体征数据\n\
             - 呼吸率: {:.1} 次/分钟 (正常范围: 12-20)\n\
             - 心率: {:.0} 次/分钟 (正常范围: 60-100)\n\
             - 运动状态: {}\n\
             - START分诊: {}\n\n\
             请基于以上数据生成一份简短的方舱医院伤病评估报告，\
             包括: 1) 当前状态判断 2) 紧急程度 3) 建议处置措施。\
             只输出报告内容，不要额外解释。",
            self.system_prompt,
            vitals.breathing_rate_bpm.unwrap_or(0.0),
            vitals.heart_rate_bpm.unwrap_or(0.0),
            if vitals.motion_score > 0.5 { "可自主移动" } else { "静止/卧床" },
            triage,
        );
        
        // Tokenize
        let tokens = self.tokenizer.encode(prompt, true)?;
        let input_ids = Tensor::new(&tokens.get_ids()[..], &self.device)?
            .unsqueeze(0)?;
        
        // 生成 (256 tokens, temperature=0.3)
        let output = self.model.generate(
            &input_ids,
            256,    // max_new_tokens
            0.3,    // temperature (低→稳定)
            None,   // top_k
            None,   // top_p
        )?;
        
        // Decode
        let text = self.tokenizer.decode(&output.to_vec1::<u32>()?, true)?;
        
        // 清理输出 (去掉 prompt 部分)
        Ok(Self::clean_output(&text, &prompt))
    }
    
    fn build_system_prompt() -> String {
        "你是一名方舱医院的急救医师，负责根据WiFi CSI无接触监测系统\
         采集的生命体征数据，为医护人员生成伤病评估报告。报告应专业、\
         简洁、有可操作性建议。".to_string()
    }
}
```

### 3.2 资源管理

```rust
// sensing-server/src/main.rs 中的集成

use wifi_densepose_llm::LlmReporter;

struct AppStateInner {
    // ... 现有字段 ...
    
    /// LLM 伤病报告生成器 (可选, 按需加载)
    llm_reporter: Option<Arc<Mutex<LlmReporter>>>,
    /// LLM 报告缓存 (同一伤员 30 秒内不重复生成)
    llm_report_cache: HashMap<String, (String, Instant)>,
}

// HTTP API
// POST /api/v1/llm/report  { "survivor_id": "SURV-0001" }
// → { "report": "伤员呼吸急促(32次/分)...", "generated_ms": 850 }
```

### 3.3 内存预算

```
RZ/V2H 总 RAM: 8GB
───────────────────────────────
Linux OS:        ~1.5GB
sensing-server:  ~500MB  (Rust 二进制 + CSI 缓冲区)
Qwen2.5-0.5B:    ~500MB  (INT4 量化加载)
UI + WebSocket:  ~200MB
───────────────────────────────
空闲:            ~5.3GB  ✅ 充足
```

### 3.4 降级策略

LLM 是**辅助功能**，系统核心（CSI检测→分诊+告警）完全不依赖它：

```
优先级:
  1. CSI → VitalSigns → START分诊 → 告警  (必须, <50ms)
  2. CSI → VitalSigns → LLM报告生成       (可选, 500ms-2s)
  3. CSI → ONNX DensePose → 3D骨架        (可选, 100-200ms)
```

如果 LLM 加载失败或推理过慢 → 自动回退到模板报告：

```rust
fn fallback_report(vitals: &VitalSignsInput, triage: &str) -> String {
    format!(
        "【自动伤病评估】\n\
         呼吸率: {:.1}次/分钟 | 心率: {:.0}BPM\n\
         分诊等级: {}\n\
         建议: {}",
        vitals.breathing_rate_bpm.unwrap_or(0.0),
        vitals.heart_rate_bpm.unwrap_or(0.0),
        triage,
        match triage {
            "Immediate" => "立即气道评估+氧疗，优先转运红色区域\"",
            "Delayed" => "持续监测，准备转运黄色区域\"",
            "Minor" => "观察，可自主活动",
            _ => "进一步评估\"",
        }
    )
}
```

---

## 四、Cargo.toml 依赖

```toml
[package]
name = "wifi-densepose-llm"
version = "0.1.0"
edition = "2021"
description = "Edge LLM reporter for field hospital triage"

[dependencies]
candle-core = "0.4"
candle-nn = "0.4"
candle-transformers = "0.8"
tokenizers = "0.21"
tokio = { version = "1", features = ["sync"] }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
tracing = "0.1"

[features]
default = ["llm"]
llm = []        # 启用完整 LLM
template-only = []  # 仅用模板 (无需下载模型)
```

---

## 五、实施计划
| 阶段 | 内容 | 工作量 |
|:---:|------|:---:|
| 1 | 创建 `wifi-densepose-llm` crate + 模板报告功能 (不依赖 Candle) | 2-3h |
| 2 | 集成到 sensing-server, 添加 `/api/v1/llm/report` 端点 | 1-2h |
| 3 | triage.html 添加"伤病报告"卡片, 点击触发 LLM | 1-2h |
| 4 | 下载 Qwen2.5-0.5B 模型 + Candle 集成 (可选, 升级模板为真实 LLM) | 4-8h |
| 5 | RZ/V2H 上编译测试 + 性能优化 | 2-4h |

### 竞赛策略

```
初赛 (视频+报告):
  → 实现阶段 1-3 (模板报告)
  → 在设计报告中完整描述 LLM 方案设计
  → PPT 展示阶段 4 的架构设计

决赛 (现场演示):
  → 如果来得及 → 集成真实 LLM (阶段 4-5)
  → 如果来不及 → 模板报告已可演示 LLM 接口 + 降级逻辑
```

---

## 六、为什么选择这个方案

| 考量 | 决策 |
|------|------|
| 技术栈一致性 | candle 是纯 Rust，与 sensing-server 同一 `cargo build` |
| 交叉编译 | 无需 C++ 工具链，aarch64 编译零配置 |
| 中文能力 | Qwen2.5 阿里出品，原生中文，医疗场景适配强 |
| 内存安全 | 0.5B INT4 ~500MB，RZ/V2H 8GB 富余 |
| 隐私 | 全本地推理，无数据出方舱 |
| 降级保障 | 模板报告一键回退，竞赛演示不翻车 |

---

*文档版本: v1.0 | 2026-05-07*
