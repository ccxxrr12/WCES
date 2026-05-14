# 瑞萨 RZ/V2H 移植计划

## 📋 项目概述

将 Rader_WIFI 项目的 Rust 推理服务移植到瑞萨 RZ/V2H ARM 平台，利用 DRP-AI 加速器进行神经网络推理。

---

## 🎯 硬件规格

### 瑞萨 RZ/V2H (R9A09G057)

| 规格 | 参数 |
|------|------|
| **CPU** | ARM Cortex-A55 × 4 (1.0 GHz) |
| **AI 加速器** | DRP-AI (深度学习加速 IP) |
| **RAM** | 最大 4GB DDR4 |
| **存储** | eMMC / SD / SPI Flash |
| **操作系统** | Linux (Yocto/Buildroot) |
| **GPU** | Mali-G31 MP2 |
| **视频编解码** | H.264/H.265 4K@30fps |

---

## 🏗️ 软件架构设计

### 目标架构

```
┌─────────────────────────────────────────────────────┐
│                 瑞萨 RZ/V2H 主节点                   │
├─────────────────────────────────────────────────────┤
│                                                     │
│ ┌─────────────────────────────────────────────┐    │
│ │  轻量级 Sensing Daemon (Rust)               │    │
│ │  ├─ CSI 数据接收 (UDP 5005)                 │    │
│ │  ├─ 信号处理 (RuvSense)                     │    │
│ │  ├─ DRP-AI 推理引擎                         │    │
│ │  └─ WebSocket 服务 (ws://:8765)             │    │
│ └─────────────────────────────────────────────┘    │
│                       │                             │
│ ┌─────────────────────────────────────────────┐    │
│ │  医疗检测服务 (Rust WASM)                   │    │
│ │  ├─ 生命体征监测                            │    │
│ │  ├─ 休克早期预警                            │    │
│ │  ├─ START 分诊                              │    │
│ │  └─ 警报管理                                │    │
│ └─────────────────────────────────────────────┘    │
│                       │                             │
│ ┌─────────────────────────────────────────────┐    │
│ │  嵌入式 Web UI (简化版)                     │    │
│ │  ├─ 伤员列表                                │    │
│ │  ├─ 生命体征曲线                            │    │
│ │  └─ 警报显示                                │    │
│ └─────────────────────────────────────────────┘    │
│                                                     │
└─────────────────────────────────────────────────────┘
         │
         │ WiFi
         │
┌─────────────────────────────────────────────────────┐
│        ESP32-C5 × 3 (CSI 感知节点)                  │
└─────────────────────────────────────────────────────┘
```

---

## 🛠️ 移植步骤

### 阶段 1：环境搭建（1 周）

#### 1.1 获取瑞萨开发工具
```bash
# 1. 下载 RZ/V2H BSP (Board Support Package)
# 访问：https://www.renesas.com/rzv2h-bsp

# 2. 下载 DRP-AI 工具
# 访问：https://www.renesas.com/drp-ai-tool

# 3. 安装交叉编译工具链
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# 4. 验证安装
aarch64-linux-gnu-gcc --version
```

#### 1.2 安装 Rust 交叉编译支持

```bash
# 添加 ARM64 目标
rustup target add aarch64-unknown-linux-gnu

# 配置 .cargo/config.toml
cat >> .cargo/config.toml << EOF
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF
```

#### 1.3 获取 DRP-AI 模型编译器
联系瑞萨获取以下工具：
- DRP-AI Model Compiler
- DRP-AI Runtime Library
- DRP-AI 文档

**瑞萨联系方式：**
- 官网：https://www.renesas.com/cn/zh
- 技术支持：https://www.renesas.com/cn/zh/support
- 邮箱：cn.support@renesas.com

---

### 阶段 2：模型转换（2 周）

#### 2.1 导出 ONNX 模型

```bash
cd D:\CODING\Repository\Rader_WIFI\rust-port\wifi-densepose-rs

# 导出 DensePose 模型到 ONNX 格式
cargo run --bin export-onnx -- \
  --input models/densepose-v1.rvf \
  --output models/densepose.onnx \
  --opset 13

# 验证 ONNX 模型
python -m onnxruntime.capi.onnxruntime_pybind11_state validate models/densepose.onnx
```

#### 2.2 使用 DRP-AI 编译器转换
```bash
# 使用瑞萨工具转换模型
drpai_model_compiler \
  --input models/densepose.onnx \
  --output models/densepose.drpai \
  --target RZV2H \
  --precision FP16 \
  --optimize performance

# 检查转换报告
cat models/densepose.drpai/report.txt
```

**可能遇到的问题：**

| 问题 | 解决方案 |
|------|---------|
| **不支持的算子** | 使用 DRP-AI 支持的算子重写，或回退到 CPU |
| **内存超限** | 降低精度 (FP32→FP16→INT8)，或剪枝模型 |
| **性能不达标** | 调整网络结构，使用 DRP-AI 优化配置 |

#### 2.3 模型量化（可选）

```bash
# 如果需要 INT8 量化以减小模型大小
drpai_quantizer \
  --input models/densepose.onnx \
  --output models/densepose_int8.drpai \
  --calibration-data calibration_data/ \
  --target RZV2H
```

---

### 阶段 3：推理引擎开发（2-3 周）

#### 3.1 创建 DRP-AI 后端

新建文件：`crates/wifi-densepose-nn/src/drpai_backend.rs`

```rust
//! DRP-AI Backend for Renesas RZ/V2H
//! 
//! 使用瑞萨 DRP-AI 硬件加速器进行神经网络推理

use crate::error::{NnError, NnResult};
use crate::tensor::Tensor;
use crate::inference::Backend;

#[cfg(target_arch = "aarch64")]
use drpai_runtime::{DrpaiModel, DrpaiRuntime, DrpaiConfig};

pub struct DrpaiBackend {
    #[cfg(target_arch = "aarch64")]
    model: DrpaiModel,
    
    #[cfg(target_arch = "aarch64")]
    runtime: DrpaiRuntime,
    
    // CPU 回退方案
    fallback_backend: Option<crate::onnx::OnnxBackend>,
}

impl DrpaiBackend {
    pub fn new(model_path: &str) -> NnResult<Self> {
        #[cfg(target_arch = "aarch64")]
        {
            let config = DrpaiConfig::default();
            let model = DrpaiModel::load(model_path)?;
            let runtime = DrpaiRuntime::new(&model, &config)?;
            
            Ok(Self {
                model,
                runtime,
                fallback_backend: None,
            })
        }
        
        #[cfg(not(target_arch = "aarch64"))]
        {
            // 非 ARM64 平台，使用 CPU 回退
            eprintln!("DRP-AI only available on ARM64, using CPU fallback");
            Ok(Self {
                fallback_backend: Some(crate::onnx::OnnxBackend::from_file(model_path)?),
            })
        }
    }
}

impl Backend for DrpaiBackend {
    fn infer(&self, input: &Tensor) -> NnResult<Tensor> {
        #[cfg(target_arch = "aarch64")]
        {
            // 尝试使用 DRP-AI
            match self.runtime.run(&self.model, input) {
                Ok(output) => Ok(output),
                Err(e) => {
                    eprintln!("DRP-AI inference failed: {:?}, falling back to CPU", e);
                    // 回退到 CPU
                    if let Some(ref fallback) = self.fallback_backend {
                        fallback.infer(input)
                    } else {
                        Err(NnError::inference("DRP-AI failed and no fallback available"))
                    }
                }
            }
        }
        
        #[cfg(not(target_arch = "aarch64"))]
        {
            // 使用 CPU 回退
            self.fallback_backend
                .as_ref()
                .ok_or_else(|| NnError::inference("No backend available"))?
                .infer(input)
        }
    }
}
```

#### 3.2 修改 Cargo.toml

```toml
[dependencies]
# 添加 DRP-AI 运行时（仅在 ARM64）
[target.'cfg(target_arch = "aarch64")'.dependencies]
drpai-runtime = { path = "/path/to/renesas/drpai-runtime" }

# 保留 ONNX 作为回退
onnxruntime = { version = "0.0.14", features = ["download-binaries"] }
```

#### 3.3 交叉编译

```bash
# 在 x86_64 开发机上交叉编译
cargo build --target aarch64-unknown-linux-gnu --release

# 或在瑞萨设备上原生编译
ssh renesas-board
cd /opt/wifi-densepose
cargo build --release
```

---

### 阶段 4：服务拆分（1-2 周）

#### 4.1 创建轻量级 Sensing Daemon

新建文件：`crates/wifi-densepose-sensing-daemon/src/main.rs`

```rust
//! 轻量级感知守护进程
//! 
//! 功能：
//! - 接收 ESP32 UDP 数据
//! - 信号处理
//! - DRP-AI 推理
//! - WebSocket 数据推送

use wifi_densepose_hardware::Esp32Parser;
use wifi_densepose_signal::RuvSenseProcessor;
use wifi_densepose_nn::{InferenceEngine, DrpaiBackend};
use tokio::net::WebSocketServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 初始化 DRP-AI 推理引擎
    let backend = DrpaiBackend::new("/opt/models/densepose.drpai")?;
    let inference_engine = InferenceEngine::new(backend)?;
    
    // 2. 启动 UDP 监听器（接收 ESP32 数据）
    let udp_socket = tokio::net::UdpSocket::bind("0.0.0.0:5005").await?;
    println!("Listening for ESP32 data on UDP 5005");
    
    // 3. 启动 WebSocket 服务器
    let ws_server = WebSocketServer::bind("0.0.0.0:8765").await?;
    println!("WebSocket server started on ws://0.0.0.0:8765");
    
    // 4. 主处理循环
    loop {
        // 接收 CSI 数据
        let mut buf = vec![0u8; 4096];
        let (len, addr) = udp_socket.recv_from(&mut buf).await?;
        
        // 解析 CSI 帧
        let csi_frame = Esp32Parser::parse(&buf[..len])?;
        
        // 信号处理
        let features = RuvSenseProcessor::process(&csi_frame)?;
        
        // 神经网络推理
        let pose = inference_engine.infer(&features)?;
        
        // 通过 WebSocket 推送
        ws_server.broadcast(&pose)?;
    }
}
```

#### 4.2 修改 Cargo.toml 添加 binary

```toml
[[bin]]
name = "sensing-daemon"
path = "crates/wifi-densepose-sensing-daemon/src/main.rs"

[features]
embedded = ["drpai-backend", "no-ui"]
```

---

### 阶段 5：UI 简化（1 周）

#### 方案 A：轻量级 Web UI（推荐）

创建简化版 UI：`ui-embedded/`

```html
<!-- ui-embedded/index.html -->
<!DOCTYPE html>
<html>
<head>
    <title>医疗监护系统 - 瑞萨 RZ/V2H</title>
    <style>
        body { font-family: Arial; background: #0d1117; color: #e0e0e0; }
        .casualty-list { margin: 20px; }
        .casualty-card { 
            background: #161b22; 
            border: 1px solid #30363d;
            border-radius: 8px;
            padding: 15px;
            margin: 10px 0;
        }
        .priority-red { border-left: 4px solid #ef4444; }
        .priority-yellow { border-left: 4px solid #fbbf24; }
        .priority-green { border-left: 4px solid #00cc88; }
        .vitals { display: flex; gap: 20px; margin-top: 10px; }
        .vital { font-size: 14px; }
        .alert { background: rgba(239, 68, 68, 0.2); padding: 10px; border-radius: 4px; }
    </style>
</head>
<body>
    <h1>🏥 医疗监护系统</h1>
    
    <div class="casualty-list" id="casualtyList">
        <!-- 伤员卡片由 JS 动态生成 -->
    </div>
    
    <div id="alerts"></div>
    
    <script>
        const ws = new WebSocket('ws://' + window.location.host + '/ws/sensing');
        
        ws.onmessage = (event) => {
            const data = JSON.parse(event.data);
            updateCasualtyList(data.survivors);
            updateAlerts(data.alerts);
        };
        
        function updateCasualtyList(survivors) {
            const list = document.getElementById('casualtyList');
            list.innerHTML = survivors.map(s => `
                <div class="casualty-card priority-${s.triage.toLowerCase()}">
                    <h3>伤员 #${s.id} - ${s.triage}</h3>
                    <div class="vitals">
                        <div class="vital">❤️ 心率：${s.vitals.hr} BPM</div>
                        <div class="vital">💨 呼吸：${s.vitals.rr} RPM</div>
                        <div class="vital">📍 位置：( ${s.location.x}, ${s.location.y})</div>
                    </div>
                </div>
            `).join('');
        }
        
        function updateAlerts(alerts) {
            const div = document.getElementById('alerts');
            div.innerHTML = alerts.map(a => `
                <div class="alert">⚠️ ${a.message}</div>
            `).join('');
        }
    </script>
</body>
</html>
```

---

## 📊 性能优化

### 内存优化

```rust
// 使用对象池减少内存分配
use object_pool::Pool;

static CSI_FRAME_POOL: Pool<CsiFrame> = Pool::new(100);

fn process_frame() {
    let frame = CSI_FRAME_POOL.pull(CsiFrame::default);
    // 使用 frame...
    // 自动归还到池
}
```

### 功耗优化
```bash
# 降低 CPU 频率（如果实时性要求不高）
echo 800000 > /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq

# 关闭未使用的 CPU 核心
echo 0 > /sys/devices/system/cpu/cpu3/online
echo 0 > /sys/devices/system/cpu/cpu2/online
```

---

## 🧪 测试计划

### 单元测试

```bash
# 在 x86_64 上运行（使用 CPU 回退）
cargo test --package wifi-densepose-nn

# 在瑞萨设备上运行
ssh renesas-board
cd /opt/wifi-densepose
cargo test --release
```

### 性能测试

```bash
# 测量推理延迟
cargo bench --package wifi-densepose-nn --bench drpai_benchmark

# 目标指标：
# - DRP-AI 推理：< 50ms
# - CPU 回退：< 200ms
# - 端到端延迟：< 500ms
```

### 集成测试

```bash
# 模拟 3 个 ESP32 节点
python scripts/simulate_esp32_nodes.py --count 3 --target renesas-board-ip

# 运行 24 小时稳定性测试
cargo run --bin sensing-daemon -- --stress-test --duration 24h
```

---

## 📅 时间规划

| 阶段 | 任务 | 时间 | 交付物 |
|------|------|------|--------|
| **阶段 1** | 环境搭建 | 1 周 | 可交叉编译的开发环境 |
| **阶段 2** | 模型转换 | 2 周 | DRP-AI 格式模型 |
| **阶段 3** | 推理引擎 | 2-3 周 | 含 DRP-AI 后端的 Rust crate |
| **阶段 4** | 服务拆分 | 1-2 周 | 轻量级 sensing-daemon |
| **阶段 5** | UI 简化 | 1 周 | 嵌入式 Web UI |
| **阶段 6** | 集成测试 | 1 周 | 性能测试报告 |

**总计：8-10 周**

---

## 🔧 故障排查

### 问题 1：DRP-AI 编译失败

**解决：** 检查模型中是否有不支持的算子
```bash
drpai_model_compiler --list-unsupported-ops model.onnx
```

### 问题 2：交叉编译链接错误
**解决：** 确保安装了正确的工具链
```bash
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libssl-dev:arm64
```

### 问题 3：运行时 DRP-AI 初始化失败
**解决：** 检查设备驱动是否加载
```bash
lsmod | grep drpai
sudo modprobe drpai
```

---

## 📞 瑞萨技术支持
准备以下信息联系瑞萨：
```
主题：RZ/V2H DRP-AI 模型转换咨询

内容：
1. 项目：医疗监护系统参赛项目
2. 模型：DensePose 人体姿态估计
3. 问题：[具体错误]
4. 已尝试：[列出步骤]

附件：
- ONNX 模型文件
- DRP-AI 编译日志
- 错误截图
```

**联系方式：**
- 官网：https://www.renesas.com/cn/zh
- 技术支持：https://www.renesas.com/cn/zh/support/contact
- 电话：400-670-3399（中国）

---

## 📚 参考资料
- RZ/V2H 数据手册：https://www.renesas.com/rzv2h-datasheet
- DRP-AI 用户手册：联系瑞萨获取
- Rust 交叉编译：https://rust-lang.github.io/rustup/cross-compilation.html
