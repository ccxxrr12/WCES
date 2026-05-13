# WCES — 基于WIFI CSI感知与端侧LLM的方舱生命体征感知与监护系统

> 第九届全国大学生嵌入式芯片与系统设计竞赛 · 瑞萨赛道
> 硬件：瑞萨 RZ/V2H + 3× ESP32-C5-DevKitC-1-N8R8
> 状态：P0-P10a 完成 ✅ | MAT 分诊 + 10 边缘模块 + 模拟演示 | 端侧 LLM 待实现 🔧

---

## 快速开始

### 硬件连接

```
                     TP-Link 千兆路由器 (192.168.1.0/24)
                              │
          ┌───────────────────┼───────────────────┐
          │                   │                   │
    ┌─────▼─────┐      ┌──────▼──────┐      ┌─────▼─────┐
    │ ESP32-C5  │      │ 瑞萨 RZ/V2H │      │ ESP32-C5  │
    │  节点 #2  │      │  (主控+AI)  │      │  节点 #3  │
    │ .1.11     │      │  192.168.1.1│      │ .1.12     │
    └───────────┘      │             │      └───────────┘
                       │  7" HDMI 触屏│
    ┌─────▼─────┐      └─────────────┘
    │ ESP32-C5  │
    │  节点 #1  │
    │ .1.10     │
    └───────────┘
```

### 一键启动

```bash
# 1. 烧录固件（每个节点修改 node_id）
cd firmware/esp32-c5-csi-node
python provision.py --chip esp32c5 --node-id 1 --port COM3
# 节点 2: --node-id 2 --port COM4
# 节点 3: --node-id 3 --port COM5

# 2. 编译服务端（在 rust-server 目录内）
cd rust-server
cargo build --release
# RZ/V2H 交叉编译：
# cargo build --target aarch64-unknown-linux-gnu --release

# 3. 返回项目根目录，一键部署
cd ..
./deploy.sh

# 4. 浏览器打开仪表盘
# http://192.168.1.1:8080/ui/triage.html    ← 分诊仪表盘
# http://192.168.1.1:8080/                  ← 3D 可视化
```

### 无硬件模拟运行（开发/演示）

```bash
cd rust-server
cargo run -p wifi-densepose-sensing-server -- --source simulate --ui-path ../docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080

# 浏览器打开 http://localhost:8080/ui/triage.html
# 即可看到完整分诊仪表盘（伤员地图 + 生命体征 + START 分诊 + 边缘模块告警）
```

---

## 系统架构

```
CSI 感知层               AI 计算层                   展示层
─────────────────────    ──────────────────────    ────────────────
ESP32-C5 ×3              RZ/V2H                    7" 触屏 / Web
  │                        │                         │
  ├─ CSI 采集 (484子载波)  │                         │
  ├─ 2.4/5GHz 双频         │                         │
  │                        │                         │
  ├─ UDP 5005 ────────────►├─ Rust Signal Pipeline   │
  │  (ADR-018 二进制帧)    │  • FFT 呼吸率/心率      │
  │                        │  • TriageEngine START   │
  │                        │    分诊 + 伤员追踪      │
  │                        │  • WiFi 三角定位         │
  │                        │  • 告警生成              │
  │                        │  • ONNX DensePose (可选) │
  │                        │                         │
  │                        ├─ WebSocket /ws/sensing ─►├─ Triage Dashboard
  │                        │  SensingUpdate JSON      │  • 2D 伤员地图
  │                        │  ├─ vital_signs          │  • 生命体征卡片
  │                        │  └─ triage_update         │  • 分诊统计 + 告警
  │                        │                         │
  │                        │                         ├─ 3D Visualization
  │                        │                         │  • Three.js 骨架
  │                        │                         │  • 实时运动追踪
```

---

## 核心功能

| 功能 | 实现 | 状态 |
|------|------|:--:|
| WiFi CSI 采集 | ESP32-C5 固件 (WiFi 6, 484 子载波, 2.4/5GHz) | ✅ |
| 呼吸率检测 | FFT 频域分析 (0.1-0.5Hz → 6-30 BPM) | ✅ |
| 心率检测 | 相位方差频谱 (0.8-2.0Hz → 40-120 BPM) | ✅ |
| 人体存在检测 | CSI 振幅方差 + 自适应阈值 + 5帧消抖 | ✅ |
| 运动分级 | active/present_still/idle + 自适应基线校准 | ✅ |
| 多人估计 | EMA 平滑 + 迟滞上/下阈值 | ✅ |
| 信号场生成 | 20×20 网格顶视图，子载波方差→方向热点 | ✅ |
| **START 分诊** | 红/黄/绿/黑/灰 + 自动优先级评估 | ✅ |
| **伤员追踪** | 创建/匹配/更新 + 生命体征历史平滑 + 超时清理 | ✅ |
| **恶化检测** | 分诊等级连续下降 + 自动触发告警 | ✅ |
| **群体伤情评估** | 严重等级(Minimal→Critical) + 救援人员需求估算 | ✅ |
| **伤员年龄估算** | 基于呼吸率/心率推断 (Infant→Child→Adult→Elderly) | ✅ |
| **实时告警** | 自动生成 + 优先级排序 + 时间戳 | ✅ |
| **分诊仪表盘** | Canvas 2D 伤员地图 + 统计栏 + 卡片 + 告警列表 | ✅ |
| 3D 骨架重建 | ONNX DensePose (可选按钮) | ✨ |
| 10 个医疗 WASM 模块 | 睡眠呼吸暂停/心律失常/呼吸窘迫/跌倒/入侵等 | ✅ |
| 模拟运行模式 | 正弦波合成 CSI，完整数据流通，无需硬件 | ✅ |
| **端侧 LLM** | 生命体征→自然语言伤病报告 (方案设计中) | 🔧 |

---

## 逐层功能详解

### 第 1 层：CSI 数据采集（ESP32-C5 固件）

| 模块 | 功能 | 文件 |
|------|------|------|
| CSI 采集 | ESP-IDF `esp_wifi_set_csi_rx_cb()` 回调，WiFi 6 484 子载波，2.4/5GHz 双频 | `csi_collector.c` |
| ADR-018 序列化 | 20 字节头 + IQ 数据对，Magic `0xC511_0001` | `csi_collector.c` |
| UDP 发送 | lwIP socket → 主节点 UDP:5005，含 ENOMEM 退避保护 | `stream_sender.c` |
| 通道跳跃 | 定时器驱动 ch1/6/11 多频段切换 | `csi_collector.c` |
| 边缘预处理 | 子载波选择 + 幅度归一化 | `edge_processing.c` |
| WASM 热加载 | 10 个医疗模块 OTA，无需重烧固件 | `wasm_runtime.c` |
| 竞赛配置 | `sdkconfig.defaults.competition` 专用 | 固件根目录 |

### 第 2 层：CSI 帧解析（服务端入口）

| 模块 | 功能 | 文件 |
|------|------|------|
| UDP 接收器 | Tokio `UdpSocket::bind("0.0.0.0:5005")` | `main.rs:udp_receiver_task()` |
| ADR-018 解析 | Magic 验证、node_id/u16子载波/u32频率/rssi/noise_floor 提取、IQ→幅度+相位 | `main.rs:parse_esp32_frame()` |
| 帧历史缓冲 | 环形缓冲区 N 帧，用于时序分析 | `AppStateInner.frame_history` |

### 第 3 层：信号处理 + 特征提取

| 模块 | 功能 | 文件 |
|------|------|------|
| 运动检测 | 帧间幅度/相位方差 → motion_score [0,1] | `main.rs:extract_features_from_frame()` |
| 运动分级 | 自适应阈值：active/still/idle + 5帧消抖 | `main.rs:smooth_and_classify()` |
| 特征向量 | mean_rssi, variance, motion_band_power, breathing_band_power, dominant_freq, change_points, spectral_power | `main.rs` |
| 多人估计 | EMA 平滑 + 迟滞阈值 | `main.rs:compute_person_score()` |

### 第 4 层：生命体征检测

| 模块 | 功能 | 文件 |
|------|------|------|
| 呼吸率 | 0.1-0.5Hz 带通 FFT → 峰值频率 × 60 → BPM + Goertzel 置信度 | `vital_signs.rs:extract_breathing()` |
| 心率 | 0.8-2.0Hz 带通 FFT → BPM + 相位方差特征 | `vital_signs.rs:extract_heartbeat()` |
| 信号质量 | SNR（RSSI-噪声底）+ 子载波一致性 → [0,1] | `vital_signs.rs:compute_signal_quality()` |
| 平滑 | EMA + 中值滤波 + trimmed mean 异常值剔除 | `main.rs:smooth_vitals()` |
| 输出 | `VitalSigns { breathing_rate_bpm, heart_rate_bpm, breathing_confidence, heartbeat_confidence, signal_quality }` | `vital_signs.rs` |

### 第 5 层：START 分诊 + 伤员追踪（MAT Pipeline ⭐）

| 模块 | 功能 | 文件 |
|------|------|------|
| START 分诊 | Immediate(红) RR>30/<10\|HR>120/<40; Delayed(黄) 中等异常; Minor(绿) 体征正常; Deceased(黑) 无体征; Unknown(灰) 信号不足 | `mat_pipeline.rs:calculate_triage()` |
| 伤员匹配 | person_id + node_id 匹配，5s 内同节点视为同一人 | `mat_pipeline.rs:match_or_create()` |
| 生命体征历史 | 30 帧滑动窗口 EMA 平滑 | `mat_pipeline.rs` |
| 位置估算 | RSSI → 距离 → 三角坐标 | `mat_pipeline.rs:rssi_to_distance()` |
| 恶化检测 | 分诊等级连续下降 ≥2 级 → DETERIORATION 告警 | `mat_pipeline.rs` |
| 群体评估 | 统计 total/immediate/delayed/minor/deceased + severity + rescuer_estimate | `mat_pipeline.rs:build_update()` |
| 告警系统 | 自动生成 + 优先级排序 + 时间戳 | `mat_pipeline.rs` |
| 年龄估算 | 呼吸率/心率 → Infant(<2y)/Child(2-12y)/Adult/Elderly(60y+) | `mat_pipeline.rs:estimate_age()` |
| 集成方式 | `TriageEngine::process()` 在 udp_receiver_task + simulated_data_task 中调用，结果写入 `SensingUpdate.triage_update` | `main.rs` |

### 第 6 层：Web 可视化（展示层）

| 模块 | 功能 | 文件 |
|------|------|------|
| 2D 伤员地图 | Canvas 顶视图，C5 节点蓝色固定标记，伤员彩色圆点 | `triage.html:draw()` |
| 实时统计栏 | 总计/紧急/延迟/轻伤/死亡 五色卡片 | `triage.html:renderFromServer()` |
| 伤员卡片 | ID、追踪时长、节点号、年龄、呼吸率、心率、分诊标签、恶化警告 | `triage.html` |
| 告警列表 | 时间倒序、颜色编码、最近 20 条 | `triage.html` |
| 群体评估 | 伤情等级 + 救援人员需求 | `triage.html` |
| **边缘模块引擎** | 10 个医疗WASM模块原生编译，零额外依赖，RZ/V2H硬件FPU加速 | `edge_module_engine.rs` |
| WebSocket | `/ws/sensing` 实时推送 `SensingUpdate` JSON | `main.rs` |
| 3D 可视化 | Three.js 实时姿态渲染 (可选 ONNX DensePose) | `ui/index.html` |

### 边缘模块引擎性能优化

竞赛演示期，10 个 WASM 边缘模块以精简原生 Rust 编译到 sensing-server，
无需 WASM 解释器开销，直接利用 RZ/V2H 硬件 FPU：

| 优化 | 说明 | 提升 |
|------|------|:--:|
| 原生 FPU 计算 | 替代 WASM `libm` 软浮点库，使用硬件 `f32::sqrt()` | 5-10× |
| 单编译单元 | 所有模块内联到单一 struct，编译器激进内联+LTO | ~2× |
| 缓存友好 | 10 个模块共享连续内存布局，减少 cache miss | ~1.5× |
| 零 FFI 开销 | 无 `csi_*` 导入函数跨 WASM 边界调用 | 消除延迟 |

**算法等价**：每个模块的核心逻辑（ring buffer、阈值检测、debounce、Lyapunov 指数）
与原 WASM 实现完全一致。量产后 ESP32 固件烧录 `.wasm` 二进制，
服务端通过 UDP `magic 0xC511_0004` 接收 WASM 输出包，
`EdgeAlert` 格式兼容，无需修改 triage.html。

---

## 数据流（端到端）

```
ESP32-C5 ×3                    RZ/V2H (sensing-server)                 浏览器
─────────────    ──────────────────────────────────────    ──────────────────
CSI 采集          UDP:5005 →
                  parse_esp32_frame()
                    → Esp32Frame { amplitudes, phases, rssi, node_id... }
                  
                  extract_features_from_frame()
                    → 运动检测、存在检测、特征提取
                  
                  VitalSignDetector::process_frame()
                    → VitalSigns { breathing_rate_bpm, heart_rate_bpm,
                                   confidence, signal_quality }
                  
                  TriageEngine::process()
                    → TriageUpdate { survivors, assessment, alerts }

                  EdgeModuleEngine::process_frame()
                    → Vec<EdgeAlert> (10 个边缘模块并行)

                  构造 SensingUpdate {
                    vital_signs,
                    triage_update,      ← MAT 分诊
                    wasm_alerts,         ← 边缘模块告警
                    features,
                    classification,
                    signal_field
                  }
                                                          WebSocket /ws/sensing →
                                                          triage.html 渲染
                                                            ├─ 伤员地图
                                                            ├─ 统计卡片
                                                            ├─ 伤员卡片
                                                            └─ 告警列表
```

---

## 目录结构

```
├── README.md                          ← 本文件
├── deploy.sh                          ← 一键部署脚本
├── firmware/
│   └── esp32-c5-csi-node/            ← C5 CSI 固件 (完整, 含竞赛配置)
├── rust-server/
│   ├── Cargo.toml                     ← Rust workspace (8 crates)
│   └── crates/
│       ├── wifi-densepose-core/       ← 基础类型
│       ├── wifi-densepose-signal/     ← CSI 信号处理
│       ├── wifi-densepose-vitals/     ← 生命体征提取
│       ├── wifi-densepose-hardware/   ← CSI 帧解析
│       ├── wifi-densepose-nn/         ← ONNX 推理 (可选)
│       ├── wifi-densepose-mat/        ← 分诊系统 ⭐
│       ├── wifi-densepose-sensing-server/ ← 主服务 (含 MAT 集成)
│       ├── wifi-densepose-config/     ← 系统配置
│       └── wifi-densepose-wasm-edge/  ← 边缘 WASM 模块 (10 医疗)
├── ui/                                ← Web 3D 可视化 (210 files)
├── scripts/
│   └── provision.py                   ← C5 烧录脚本
└── docs/                              ← 竞赛设计文档
    ├── README_COMPETITION.md          ← 竞赛版 README
    ├── PROGRESS.md                    ← 构建进度 (实时更新)
    ├── 竞赛改造方案.md                 ← 完整改造计划 (A/B/C/D/E类)
    ├── 竞赛差距分析.md                 ← 需求 vs 能力对比
    ├── 竞赛准备清单.md                 ← PPT/视频/展板等材料清单
    ├── ML架构详解.md                   ← CSI→姿态 ML 架构
    ├── 端侧LLM方案设计.md              ← LLM 伤病报告方案
    ├── ESP32-C5 移植审计报告.md        ← 39 处修改审计
    ├── ESP32-C5 移植指南.md            ← C5 移植指南
    ├── 瑞萨 RZV2H 移植计划.md          ← RZ/V2H 移植计划
    ├── 目录审计报告.md                 ← 目录完整性审计
    └── triage-ui/
        └── triage.html                ← 分诊仪表盘
```

---

## 技术亮点

- **WiFi 6 CSI**: ESP32-C5 484 子载波，4× 传统 S3 方案精度
- **端侧 LLM**（方案设计中）: 生命体征→自然语言伤病报告 (Qwen2.5-0.5B / Candle)
- **Rust 高性能**: 54,000 帧/秒信号处理管道，比 Python 快 810 倍
- **START 分诊**: 标准战场分诊协议，自动伤员优先级评估
- **端到端打通**: CSI 采集→信号处理→生命体征→分诊→追踪→可视化，完整管道
- **全本地部署**: 数据不出方舱，隐私安全，无互联网依赖
- **瑞萨 DRP-AI**: 可选硬件推理加速
- **模拟模式**: 无需硬件即可启动完整演示（`cargo run -- --source simulate`）

---

## 比赛文档

| 文档 | 内容 |
|------|------|
| `docs/竞赛改造方案.md` | 从开源项目到竞赛版本的完整改造计划 |
| `docs/竞赛差距分析.md` | 竞赛需求 vs 项目现有能力对比 |
| `docs/竞赛准备清单.md` | PPT/视频/展板等竞赛材料清单 |
| `docs/ML架构详解.md` | DensePose 模型架构 + 训练 + 推理 |
| `docs/ESP32-C5 移植审计报告.md` | C5 移植 39 处修改审计 |
| `docs/ESP32-C5 移植指南.md` | C5 移植完整指南 |
| `docs/瑞萨 RZV2H 移植计划.md` | RZ/V2H 主控移植计划 |
| `docs/端侧LLM方案设计.md` | 端侧 LLM 伤病报告方案设计 |
| `docs/目录审计报告.md` | 目录完整性审计 |
| `docs/PROGRESS.md` | 构建进度追踪（实时更新） |

---

## 许可证

MIT OR Apache-2.0
