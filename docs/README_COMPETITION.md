# WCES — 基于 WiFi 6 CSI 的极端条件人员监护系统

> 第九届全国大学生嵌入式芯片与系统设计竞赛 · 瑞萨赛道
> 硬件: 瑞萨 RZ/V2H + 3× ESP32-C5-DevKitC-1-N8R8

---

## 快速开始

### 硬件连线

```
                  ┌──────────────────┐
                  │  TP-Link 千兆路由 │  (192.168.1.0/24)
                  │  SSID: WCES    │
                  └──┬────┬────┬─────┘
                     │    │    │
          ┌──────────┼────┼────┼──────────┐
          │          │    │    │          │
     ┌────▼────┐ ┌──▼────▼────▼──┐ ┌────▼────┐
     │ ESP32-C5│ │  瑞萨 RZ/V2H │ │ 7" 触屏 │
     │ 节点 #1 │ │  (主控+AI)   │ │  HDMI   │
     │ 192.168 │ │192.168.1.1   │ │ 显示    │
     │ .1.10   │ │              │ │         │
     └─────────┘ └──────────────┘ └─────────┘
     ESP32-C5 #2 (.1.11)    ESP32-C5 #3 (.1.12)
```

### 一键启动

```bash
# 1. 烧录 3 个 C5 节点 (分别设置 node_id=1,2,3)
cd firmware/esp32-c5-csi-node
python provision.py --chip esp32c5 --node-id 1 --port COM3
python provision.py --chip esp32c5 --node-id 2 --port COM4
python provision.py --chip esp32c5 --node-id 3 --port COM5

# 2. RZ/V2H 上启动服务
ssh root@192.168.1.1
cd /opt/WCES
./deploy.sh

# 3. 浏览器打开仪表盘
# http://192.168.1.1:8080/ui/triage.html
```

---

## 系统架构

```
CSI 感知层            AI 计算层              展示层
─────────────────    ─────────────────    ─────────────────
ESP32-C5 ×3          RZ/V2H               7" 触屏 / Web
  │                    │                     │
  ├─ CSI 采集          │                     │
  │  484 子载波        │                     │
  │  2.4/5GHz 双频     │                     │
  │                    │                     │
  ├─ UDP 5005 ────────►├─ Rust Signal Pipe   │
  │                    │  · 呼吸率/心率       │
  │                    │  · TriageEngine      │
  │                    │    START 分诊        │
  │                    │  · 伤员追踪+告警     │
  │                    │                     │
  │                    ├─ WebSocket 8765 ────►├─ Triage Dashboard
  │                    │                     │  · 伤员地图
  │                    │                     │  · 生命体征卡片
  │                    │                     │  · 分诊告警
  │                    │                     │
  │                    │                     ├─ [可选] 3D 骨架
  │                    │                     │  ONNX DensePose
```

---

## 目录

```
├── firmware/esp32-c5-csi-node/   # C5 CSI 固件
├── rust-server/crates/           # Rust 服务端
│   ├── wifi-densepose-core/      # 基础类型
│   ├── wifi-densepose-signal/    # CSI 信号处理
│   ├── wifi-densepose-vitals/    # 生命体征
│   ├── wifi-densepose-hardware/  # CSI 帧解析
│   ├── wifi-densepose-nn/        # ONNX 推理 (可选)
│   ├── wifi-densepose-mat/       # 分诊系统 ⭐
│   └── wifi-densepose-sensing-server/  # 主服务 (含MAT集成)
├── docs/                         # 竞赛文档
│   ├── triage-ui/triage.html     # 分诊仪表盘 ⭐
│   ├── PROGRESS.md               # 构建进度
│   ├── README_COMPETITION.md     # 竞赛 README
│   ├── 竞赛改造方案.md            # 完整改造计划
│   ├── 竞赛差距分析.md            # 需求差距分析
│   ├── 竞赛准备清单.md            # 准备清单
│   ├── ML架构详解.md              # ML 架构
│   ├── 端侧LLM方案设计.md         # LLM 方案设计
│   ├── ESP32-C5 移植指南.md       # C5 移植指南
│   ├── ESP32-C5 移植审计报告.md   # C5 审计报告
│   ├── 瑞萨 RZV2H 移植计划.md     # RZ/V2H 移植
│   └── 目录审计报告.md            # 目录审计
├── ui/                           # 3D Web 可视化
├── scripts/provision.py          # C5 烧录脚本
└── deploy.sh                     # 一键部署
```

---

## 核心功能

| 功能 | 实现方式 | 状态 |
|------|----------|:----:|
| WiFi CSI 采集 | ESP32-C5 固件 (484子载波) | ✅ |
| 呼吸率检测 | Fresnel 模型 + FFT | ✅ |
| 心率检测 | BVP + 带通滤波 | ✅ |
| 人体存在检测 | CSI 振幅方差 + 自适应阈值 | ✅ |
| 多人区分 | MinCut 子载波分区 | ✅ |
| 人员定位 | WiFi 三角测量 | ✅ |
| **START 分诊** | 规则引擎 (红/黄/绿/黑) | ✅ |
| **伤员追踪** | Kalman + 生命周期管理 | ✅ |
| **群体伤情评估** | 严重程度 + 资源需求计算 | ✅ |
| **告警系统** | 自动生成 + 优先级排序 | ✅ |
| 3D 骨架重建 | ONNX DensePose (可选) | ✨ |
| WASM 边缘医疗模块 | 10 个医疗检测模块 | ✅ |

---

## 竞赛文档

| 文档 | 说明 |
|------|------|
| `docs/竞赛改造方案.md` | 完整改造计划 |
| `docs/ML架构详解.md` | 机器学习架构 |
| `docs/竞赛差距分析.md` | 需求差距分析 |
| `docs/竞赛准备清单.md` | 竞赛准备清单 |
| `docs/ESP32-C5 移植审计报告.md` | C5 移植审计 |
| `docs/PROGRESS.md` | 构建进度追踪 |
| `docs/端侧LLM方案设计.md` | 端侧 LLM 方案 |
| `docs/瑞萨 RZV2H 移植计划.md` | RZ/V2H 移植计划 |
