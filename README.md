# WCES — 基于WiFi CSI感知与端侧Agent的方舱生命体征感知与监护系统

> 第九届全国大学生嵌入式芯片与系统设计竞赛 · 瑞萨赛道
> 硬件：瑞萨 RZ/G2L + 3× ESP32-C5-DevKitC-1-N8R8
> 状态：P0-P10f 完成 ✅ | MAT 分诊 + 19 边缘模块 + Medical Agent + 代码重构 + UI 全面优化 | main.rs 3868→1331 行（-66%），拆分为 31 个模块

---

## 快速开始

### 硬件连接

```
                     TP-Link 千兆路由器 (192.168.1.0/24)
                              │
          ┌───────────────────┼───────────────────┐
          │                   │                   │
    ┌─────▼─────┐      ┌──────▼──────┐      ┌─────▼─────┐
    │ ESP32-C5  │      │ 瑞萨 RZ/G2L │      │ ESP32-C5  │
    │  节点 #2  │      │  (主控+AI)  │      │  节点 #3  │
    │ .1.11     │      │192.168.1.100│      │ .1.12     │
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
# RZ/G2L 交叉编译：
# cargo build --target aarch64-unknown-linux-gnu --release

# 3. 返回项目根目录，一键部署
cd ..
./deploy.sh

# 4. 浏览器打开仪表盘
# http://192.168.1.100:8080/ui/triage.html    ← 分诊仪表盘
# http://192.168.1.100:8080/                  ← 3D 可视化
```

### ⚡ 使用统一配置 (推荐)

所有子系统配置集中在 `wces.config.toml` 一个文件中：

```powershell
# 1. 编辑配置 (WiFi SSID/密码/信道/节点ID/端口等)
notepad wces.config.toml

# 2. 应用配置到各节点 (自动生成 sdkconfig + 更新 deploy.sh)
.\apply-config.ps1 -NodeId 1
.\apply-config.ps1 -NodeId 2
.\apply-config.ps1 -NodeId 3

# 3. 编译 + 烧录
cd firmware\esp32-c5-csi-node
idf.py set-target esp32c5 && idf.py build && idf.py -p COM3 flash

# 4. 启动服务端
cd ..\..\rust-server
cargo run -p wifi-densepose-sensing-server -- --source auto --ui-path ../docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080
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
ESP32-C5 ×3              RZ/G2L                    7" 触屏 / Web
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
  │                        │  • DensePose 3D 骨架生成   │
  │                        │  • Medical Agent 分析     │
  │                        │                         │
  │                        ├─ WebSocket :8765 ─►├─ Triage Dashboard
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
| WiFi CSI 采集 | ESP32-C5 固件 (WiFi 6, HE40 484子载波, 2.4/5GHz) | ✅ |
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
| **分诊仪表盘** | Canvas 2D 伤员地图 + 热力图 + 3D 骨架 + 生命体征趋势图 + EHR 面板 + 暗色/亮色主题 | ✅ |
| 3D 骨架重建 | Three.js 胶囊几何体蒙皮骨架 + ONNX DensePose (可选按钮) | ✅ |
| 19 个边缘模块 | 步态/心律失常/呼吸窘迫/癫痫/徘徊/振动/LTL守卫/元学习/稀疏恢复等，原生 Rust 编译 | ✅ |
| 模拟运行模式 | 正弦波合成 CSI，完整数据流通，无需硬件 | ✅ |
| **Medical Agent** | 云端 LLM 深度分析 + 本地模板降级 (Coordinator 模式) | ✅ |
| **统一入口页** | 暗色/亮色主题门户，6 张应用卡片 + 实时系统状态检测 | ✅ |

---

## 逐层功能详解

### 第 1 层：CSI 数据采集（ESP32-C5 固件）

| 模块 | 功能 | 文件 |
|------|------|------|
| CSI 采集 | ESP-IDF `esp_wifi_set_csi_rx_cb()` 回调，WiFi 6 HE40 484子载波，2.4/5GHz 双频 | `csi_collector.c` |
| ADR-018 序列化 | 20 字节头 + IQ 数据对，Magic `0xC511_0001` | `csi_collector.c` |
| UDP 发送 | lwIP socket → 主节点 UDP:5005，含 ENOMEM 退避保护 | `stream_sender.c` |
| 通道跳跃 | 定时器驱动 ch1/6/11 多频段切换 | `csi_collector.c` |
| 边缘预处理 | 子载波选择 + 幅度归一化 | `edge_processing.c` |
| WASM 热加载 | 最多 4 个 WASM 模块 OTA 热加载 (ESP32 Tier 3 运行时) | `wasm_runtime.c` |
| 竞赛配置 | 由 apply-config.ps1 生成 `sdkconfig.defaults` | 固件根目录 |

### 第 2 层：CSI 帧解析（服务端入口）

| 模块 | 功能 | 文件 |
|------|------|------|
| UDP 接收器 | Tokio `UdpSocket::bind("0.0.0.0:5005")`，写锁范围优化为两阶段（状态修改 + 纯计算分离） | `tasks/udp_receiver.rs` |
| ADR-018 解析 | Magic 验证、node_id/u16子载波/u32频率/rssi/noise_floor 提取、IQ→幅度+相位 | `parser.rs:parse_esp32_frame()` |
| 帧历史缓冲 | 环形缓冲区 N 帧，用于时序分析 | `AppStateInner.frame_history` |

### 第 3 层：信号处理 + 特征提取

| 模块 | 功能 | 文件 |
|------|------|------|
| 运动检测 | 帧间幅度/相位方差 → motion_score [0,1] | `signal_processing.rs:extract_features_from_frame()` |
| 运动分级 | 自适应阈值：active/still/idle + 5帧消抖 | `state_ops.rs:smooth_and_classify()` |
| 特征向量 | mean_rssi, variance, motion_band_power, breathing_band_power, dominant_freq, change_points, spectral_power | `signal_processing.rs` |
| 多人估计 | EMA 平滑 + 迟滞阈值 | `signal_processing.rs:compute_person_score()` |

### 第 4 层：生命体征检测

| 模块 | 功能 | 文件 |
|------|------|------|
| 呼吸率 | 0.1-0.5Hz 带通 FFT → 峰值频率 × 60 → BPM + Goertzel 置信度 | `vital_signs.rs:extract_breathing()` |
| 心率 | 0.8-2.0Hz 带通 FFT → BPM + 相位方差特征 | `vital_signs.rs:extract_heartbeat()` |
| 信号质量 | SNR（RSSI-噪声底）+ 子载波一致性 → [0,1] | `vital_signs.rs:compute_signal_quality()` |
| 平滑 | EMA + 中值滤波 + trimmed mean 异常值剔除 | `state_ops.rs:smooth_vitals()` |
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
| 集成方式 | `TriageEngine::process()` 在 `tasks/udp_receiver.rs` + `tasks/simulated_data.rs` 中调用，结果写入 `SensingUpdate.triage_update` | `tasks/` |

### 第 6 层：Web 可视化（展示层）

| 模块 | 功能 | 文件 |
|------|------|------|
| 统一入口页 | 暗色/亮色主题门户，6 张应用卡片 (Observatory/分诊/3D骨架/PoseFusion/移动端/测试)，实时系统状态检测 | `ui/index.html` |
| 2D 伤员地图 | Canvas 顶视图，C5 节点蓝色固定标记，伤员彩色圆点，信号场热力图叠加层 | `triage.html:drawMap()` |
| 实时统计栏 | 总计/紧急/延迟/轻伤/死亡 五色卡片 | `triage.html:renderFromServer()` |
| 伤员卡片 | ID、追踪时长、节点号、年龄、呼吸率、心率、分诊标签、恶化警告，可折叠侧栏 | `triage.html` |
| 告警列表 | 时间倒序、颜色编码、最近 20 条，可折叠 | `triage.html` |
| 群体评估 | 伤情等级 + 救援人员需求 | `triage.html` |
| 生命体征趋势 | 60 秒环形缓冲 sparkline 呼吸率/心率趋势小图 | `triage.html` |
| 3D 骨架 | Three.js 胶囊几何体蒙皮骨架，OrbitControls 旋转/缩放 | `triage.html` |
| EHR 面板 | 伤员电子病历滑出面板，含体征趋势图、登记信息、LLM 分析 | `triage.html` |
| 暗色/亮色主题 | CSS 变量切换 + localStorage 持久化 | `triage.html` + `index.html` |
| 响应式布局 | @media 900px/600px 断点，ResizeObserver 画布自适应 | `triage.html` + `index.html` |
| **边缘模块引擎** | 19 个医疗WASM模块原生编译，零额外依赖，RZ/G2L硬件FPU加速 | `edge_module_engine.rs` |
| WebSocket | `/ws/sensing` 实时推送 `SensingUpdate` JSON | `handlers/ws.rs` |
| Three.js 本地库 | r140 UMD 模块 (three.min.js + OrbitControls.js)，离线可用 | `ui/lib/` |

### 边缘模块引擎性能优化

竞赛演示期，19 个 WASM 边缘模块以精简原生 Rust 编译到 sensing-server，
无需 WASM 解释器开销，直接利用 RZ/G2L 硬件 FPU：

| 优化 | 说明 | 提升 |
|------|------|:--:|
| 原生 FPU 计算 | 替代 WASM `libm` 软浮点库，使用硬件 `f32::sqrt()` | 5-10× |
| 单编译单元 | 所有模块内联到单一 struct，编译器激进内联+LTO | ~2× |
| 缓存友好 | 19 个模块共享连续内存布局，减少 cache miss | ~1.5× |
| 零 FFI 开销 | 无 `csi_*` 导入函数跨 WASM 边界调用 | 消除延迟 |

**算法等价**：每个模块的核心逻辑（ring buffer、阈值检测、debounce、Lyapunov 指数）
与原 WASM 实现完全一致。量产后 ESP32 固件烧录 `.wasm` 二进制，
服务端通过 UDP `magic 0xC511_0005` 接收 WASM 输出包，
`EdgeAlert` 格式兼容，无需修改 triage.html。

---

## 数据流（端到端）

```
ESP32-C5 ×3                    RZ/G2L (sensing-server)                浏览器
─────────────    ──────────────────────────────────────    ──────────────────
CSI 采集          UDP:5005 →
                  parser::parse_esp32_frame()
                    → Esp32Frame { amplitudes, phases, rssi, node_id... }
                  
                  signal_processing::extract_features_from_frame()
                    → 运动检测、存在检测、特征提取
                  
                  state_ops::smooth_and_classify()
                    → 自适应基线 + 消抖分级
                  
                  VitalSignDetector::process_frame()
                    → VitalSigns { breathing_rate_bpm, heart_rate_bpm,
                                   confidence, signal_quality }
                  
                  state_ops::smooth_vitals()
                    → 中值滤波 + EMA 平滑
                  
                  TriageEngine::process()
                    → TriageUpdate { survivors, assessment, alerts }

                  EdgeModuleEngine::process_frame()
                    → Vec<EdgeAlert> (19 个边缘模块并行)

                  signal_processing::generate_signal_field()
                    → 20×20 信号场热力图

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
├── wces.config.toml                  ← ⭐ 统一配置文件
├── apply-config.ps1                  ← Windows 配置应用脚本
├── apply-config.sh                   ← Linux 配置应用脚本
├── deploy.sh                          ← 一键部署脚本
├── firmware/
│   └── esp32-c5-csi-node/            ← C5 CSI 固件 (完整, 含竞赛配置)
├── rust-server/
│   ├── Cargo.toml                     ← Rust workspace (9 crates + 1 wasm32 独立编译)
│   └── crates/
│       ├── wifi-densepose-core/       ← 基础类型 (2596行)
│       ├── wifi-densepose-signal/     ← CSI 信号处理 (15176行)
│       ├── wifi-densepose-vitals/     ← 生命体征提取 (1863行)
│       ├── wifi-densepose-hardware/   ← CSI 帧解析 (4007行)
│       ├── wifi-densepose-llm/        ← Medical Agent 分析引擎 ⭐ (5807行)
│       ├── wifi-densepose-nn/         ← ONNX 推理 (DensePose 3D 骨架) (2959行)
│       ├── wifi-densepose-mat/        ← 分诊系统 ⭐ (19614行)
│       ├── wifi-densepose-sensing-server/ ← 主服务 (2026-05 重构模块化)
│       │   ├── src/main.rs                 ← 入口 + CLI + 状态初始化 (1331行)
│       │   ├── src/lib.rs                  ← crate 入口
│       │   ├── src/types.rs                ← 数据类型 + 常量
│       │   ├── src/signal_processing.rs    ← 14 个纯信号处理函数
│       │   ├── src/state_ops.rs            ← 有状态操作 (smooth/classify)
│       │   ├── src/parser.rs               ← ADR-018 二进制帧解析
│       │   ├── src/server.rs               ← HTTP/WS 服务器启动
│       │   ├── src/vital_signs.rs          ← FFT 生命体征检测 (呼吸/心率)
│       │   ├── src/mat_pipeline.rs         ← START 分诊 + 伤员追踪
│       │   ├── src/edge_module_engine.rs   ← 19 边缘模块引擎
│       │   ├── src/handlers/               ← 路由处理器 (7 files: mod, ws, routes, model, recording, llm, path_util)
│       │   ├── src/tasks/                  ← 后台任务 (4 files: mod, udp_receiver, simulated_data, broadcast_tick)
│       │   ├── src/app_config.rs            ← 应用配置管理
│       │   ├── src/rvf_container.rs        ← RVF 模型容器
│       │   ├── src/rvf_pipeline.rs         ← RVF 推理管道
│       │   ├── src/adaptive_classifier.rs  ← 自适应分类器
│       │   ├── src/dataset.rs              ← 数据集管理
│       │   ├── src/embedding.rs            ← 嵌入层
│       │   ├── src/graph_transformer.rs    ← 图神经网络
│       │   ├── src/sona.rs                 ← SONA 配置文件
│       │   ├── src/sparse_inference.rs     ← 稀疏推理
│       │   ├── src/trainer.rs              ← 模型训练
│       ├── wifi-densepose-wasm-edge/  ← WASM 边缘模块 (68 .rs, wasm32 独立编译, workspace 排除)
│       └── wifi-densepose-config/     ← 系统配置 namespace 占位 (deprecated, 配置在 app_config.rs)
├── ui/                                ← Web 可视化
│   ├── index.html                     ← 统一入口门户页
│   ├── triage.html                    ← 分诊仪表盘 (新版, 暗色/亮色主题)
│   ├── lib/
│   │   ├── three.min.js               ← Three.js r140 UMD (离线可用)
│   │   └── OrbitControls.js           ← OrbitControls r140 UMD
│   ├── mobile/                        ← React Native Expo 移动端
│   ├── observatory/                   ← 3D 信号观测站
│   └── tests/                         ← 自动化测试
├── scripts/
│   └── provision.py                   ← C5 烧录脚本 (固件内也有副本)
└── docs/                              ← 竞赛设计文档 (21 个 .md 文件)
    ├── README_COMPETITION.md          ← 竞赛版 README
    ├── 项目全览.md                     ← 全项目技术全览
    ├── PROGRESS.md                    ← 构建进度 (实时更新)
    ├── 竞赛改造方案.md                 ← 完整改造计划 (A/B/C/D/E类)
    ├── 竞赛差距分析.md                 ← 需求 vs 能力对比
    ├── 竞赛准备清单.md                 ← PPT/视频/展板等材料清单
    ├── ML架构详解.md                   ← CSI→姿态 ML 架构
    ├── 端侧Agent开发计划.md             ← Medical Agent 开发计划
    ├── 端侧Agent技术文档.md             ← Agent 架构/接口/技术文档
    ├── 端侧LLM方案设计.md              ← (历史) LLM 伤病报告方案
    ├── 端侧LLM技术文档.md              ← (历史) LLM 接口/技术文档
    ├── 项目完整分析报告.md             ← 项目完整分析
    ├── ESP32-C5 移植审计报告.md        ← 39 处修改审计
    ├── ESP32-C5 移植指南.md            ← C5 移植指南
    ├── 瑞萨 RZ_G2L 移植计划.md         ← RZ/G2L 移植计划
    ├── 固件官方文档审计报告.md         ← 固件 vs 官方 API 审计
    ├── 目录审计报告.md                 ← 目录完整性审计
    ├── API_REFERENCE.md               ← WebSocket 数据接口文档
    ├── 硬件部署与使用指南.md           ← 硬件部署完整指南
    └── triage-ui/
        ├── triage.html                ← 分诊仪表盘 (暗色/亮色主题, 热力图, 3D骨架, EHR面板)
        └── triage-v1.html             ← 旧版分诊仪表盘 (备份)
```

---

## 技术亮点

- **WiFi 6 CSI**: ESP32-C5 HE40 484 子载波，4× 传统 S3 方案精度
- **Medical Agent**（已集成）: 云端 LLM 深度分析 + 本地模板降级 + 熔断保护，支持流式输出
- **Rust 高性能**: 全异步 Tokio 运行时，零拷贝解析，比 Python 方案快数十倍
- **START 分诊**: 标准战场分诊协议，自动伤员优先级评估
- **端到端打通**: CSI 采集→信号处理→生命体征→分诊→追踪→可视化，完整管道
- **全本地部署**: 核心信号处理+分诊全本地，数据不出方舱；Agent 分析可选云端 LLM 增强
- **瑞萨 RZ/G2L SBC**: ARM64 边缘计算平台 (Cortex-A55 ×2 + M33, 1GB DDR4)
- **模拟模式**: 无需硬件即可启动完整演示（`cargo run -- --source simulate`）
- **代码质量**: 2026-05 完成大规模重构，消除锁竞态死锁隐患，写锁持有时间从 135 行压缩为两阶段锁，main.rs 拆分为 31 个模块文件

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
| `docs/固件官方文档审计报告.md` | 固件 vs 官方 API 对照审计 |
| `docs/瑞萨 RZ_G2L 移植计划.md` | RZ/G2L 主控移植计划 |
| `docs/端侧Agent开发计划.md` | Medical Agent 开发计划 |
| `docs/端侧Agent技术文档.md` | Agent 架构/接口/技术文档 |
| `docs/项目全览.md` | 全项目技术全览 |
| `docs/API_REFERENCE.md` | WebSocket 数据接口完整文档 |
| `docs/目录审计报告.md` | 目录完整性审计 |
| `docs/硬件部署与使用指南.md` | 硬件部署完整指南 |
| `docs/PROGRESS.md` | 构建进度追踪（实时更新） |

---

## 许可证

MIT OR Apache-2.0
