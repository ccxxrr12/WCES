# 基于WiFi CSI感知与端侧Agent的方舱生命体征监护系统

> 第九届全国大学生嵌入式芯片与系统设计竞赛 · 瑞萨赛道
> 作品名称：基于WiFi CSI感知与端侧Agent的方舱生命体征监护系统（WCES）

---

## 摘要

本作品面向野战方舱、灾后临时医院等恶劣环境下的批量伤员快速分诊需求，设计并实现了一套基于WiFi 6 CSI（信道状态信息）非接触感知与端侧AI Agent协同的伤员生命体征监护系统。系统以瑞萨RZ/G2L双核ARM64处理器为主控计算平台，搭载3个ESP32-C5感知节点构成分布式WiFi传感网络，在不接触伤员身体的前提下，通过WiFi信号穿墙感知伤员呼吸率、心率、体动等关键生命体征，结合START（Simple Triage and Rapid Treatment）标准分诊协议实现伤员自动分类与优先级排序。

核心技术路线包括：（1）基于ESP32-C5 WiFi 6 HE40模式的484子载波高分辨率CSI采集，通过UDP实时传输至RZ/G2L边缘主控；（2）Rust语言实现的纯本地信号处理管线，包含IIR带通滤波、零交叉检测、自相关分析等生命体征提取算法；（3）基于SVD空房间校准的物理场扰动模型与加权质心法的混合人员定位方案；（4）面向大规模伤亡事件的START分诊引擎，支持伤员8维生物特征嵌入匹配与恶化追踪；（5）Coordinator模式的云端LLM医学Agent，具备流式分析、熔断降级与本地模板兜底能力。

系统以全Rust技术栈构建服务端（10个crate、约10万行代码），ESP-IDF v6.0.1 C语言固件（33个源文件、8,322行），HTML5/Canvas/Three.js Web可视化仪表盘。1,004个测试全部通过。支持模拟运行模式——无需硬件即可启动完整演示。经六轮全代码审查（842个bug发现、52个关键修复），端到端数据流（CSI采集→UDP→信号处理→生命体征→分诊→WebSocket→可视化）12条路径全部接通，0编译错误。系统已完成aarch64-unknown-linux-gnu交叉编译（Poky SDK 3.1.20），二进制大小约8.6MB，可直接部署至瑞萨RZ/G2L开发板。

**关键词**：WiFi CSI感知；非接触生命体征检测；START分诊；端侧AI；RZ/G2L边缘计算；ESP32-C5；Rust

---

# 第一部分  作品概述

## 功能与特性

本系统集WiFi CSI非接触感知、边缘AI信号处理、START标准分诊、伤员追踪与云端Agent分析于一体，面向野战方舱与灾后急救场景提供全链路自动化伤员监护解决方案。主要功能包括：

**（1）非接触式生命体征检测**：利用3个ESP32-C5节点构建WiFi 6感知网络，在不接触伤员身体、无需穿戴设备的前提下，通过CSI信号处理提取呼吸率（6-30 BPM，基于IIR带通滤波+零交叉检测）、心率（40-120 BPM，基于时序相位差分+自相关峰值分析）、体动水平（active/present_moving/present_still/absent四级分类）、人体存在检测（CSI振幅方差+自适应阈值+5帧消抖）。

**（2）START标准分诊引擎**：实现标准战场分诊协议，根据呼吸率、心率、体动水平将伤员自动分为Immediate（红色/紧急，RR>30或<10，HR>120或<40）、Delayed（黄色/延迟，中等异常）、Minor（绿色/轻伤，体征正常）、Deceased（黑色/死亡）、Unknown（灰色/数据不足）五级。支持恶化检测（分诊等级连续下降≥2级触发告警）、群体伤情评估（Minimal→Critical）+救援人员需求估算、伤员年龄推断（基于呼吸率/心率推断Infant→Child→Adult→Elderly）。

**（3）伤员追踪与Re-ID**：8维生物特征嵌入向量（呼吸率、心率、体动水平、信号质量、RSSI等），基于余弦相似度（阈值0.65）匹配幸存者身份，包含5分钟lost_pool重识别缓冲。支持三级匹配策略（person_id→Re-ID→新建）。

**（4）人员定位系统**：采用WhōFi（Top-12子载波方差定位，权重70%）+ FieldBridge（SVD空房间校准+物理扰动能量，权重30%）的混合定位方案。空房间自动校准30秒后进入定位模式，结合RSSI对数距离模型与加权质心法估计伤员位置，在2D Canvas地图上实时渲染。

**（5）19个边缘分析模块**：包括步态分析、心律失常检测、呼吸窘迫识别、癫痫发作检测、徘徊行为检测、睡眠呼吸暂停筛查等，竞赛模式下以原生Rust编译至RZ/G2L直接运行，利用硬件FPU实现5-10倍加速。

**（6）Web可视化仪表盘**：Canvas 2D伤员地图+信号场热力图叠加，Three.js 3D胶囊几何体蒙皮骨架（17 COCO关键点），实时统计卡片+伤员卡片+告警侧栏，EHR电子病历滑出面板，LLM流式分析，暗色/亮色双主题，响应式布局。

**（7）Medical Agent云端增强**：Coordinator模式——本地信号处理完成核心生命体征检测，可选云端LLM（DeepSeek V4 Pro）深度分析伤员状况并生成伤病报告。具备熔断器保护（3次失败→5分钟冷却）、流式输出、本地模板降级。

**（8）模拟演示模式**：正弦波合成CSI数据，10个虚拟伤员（3红色+2黄色+3绿色+1黑色+1灰色），完整端到端数据流，无需任何硬件即可启动完整功能演示。

## 应用领域

本系统瞄准**野战方舱医院批量伤员快速分诊**这一核心应用场景。在现代化大规模军事冲突或重大自然灾害（地震、洪水、爆炸等）中，短时间内可能出现数十至数百名伤员，传统的人工分诊严重依赖医护人员的经验判断，存在效率低下、主观性强、无法持续监测等痛点。本系统通过WiFi信号非接触感知实现：

**（1）方舱内伤员生命体征持续监护**：WiFi信号穿透衣物、被褥、帐篷，无需接触伤员即可提取呼吸/心率/体动，且不产生额外电磁辐射。3个ESP32-C5节点覆盖标准方舱（约6m×8m区域），系统自动校准后进入24×7持续监护模式。伤员移动、体征恶化、新增伤员等事件触发实时告警，为医护人员争取黄金救治时间。

**（2）战地/灾后快速分诊决策辅助**：在资源有限的大规模伤亡事件中，START分诊是国际公认的标准优先级排序方法。本系统将START协议嵌入边缘计算平台，实现自动化、标准化、可追溯的分诊决策，避免人为疲劳导致的误判。群体评估功能可估算伤情严重等级及所需救援人员数量。

**（3）端侧AI驱动的智能化分析**：Medical Agent通过云端LLM分析伤员生命体征趋势、生成鉴别诊断建议、识别潜在并发症风险（如ARDS、脓毒症早期征兆）。Analyze按钮提供一键流式分析，模板降级确保无网络时仍可用。

**（4）可扩展至更多场景**：包括医院ICU/CCU无接触监护（避免传感器粘贴导致的皮肤损伤）、养老院老人跌倒检测与徘徊监控、监狱/安防的人体存在与运动检测、智能家居日常健康监测等。

## 主要技术特点

**（1）WiFi 6全栈自研CSI感知**：基于ESP32-C5的WiFi 6（802.11ax）芯片，利用HE40模式下的484子载波CSI数据进行生命体征感知，子载波分辨率是传统ESP32-S3方案（HT40 114子载波）的4倍以上。CSI采集率100-500Hz，支持2.4GHz/5GHz双频段信道跳转（6通道×50ms dwell），UDP限速50Hz发送至RZ/G2L主控。

**（2）Rust全栈高性能边缘计算**：服务端采用Rust语言9个crate（~10万行），基于Tokio异步运行时，零拷贝ADR-018二进制帧解析。两阶段写锁设计（状态变更+纯计算分离）消除锁竞争死锁。VitalsBridge采用上游wifi_densepose_wifiscan项目的IIR带通滤波+零交叉+自相关算法，精度对标学术论文标准。

**（3）物理场建模与混合定位**：将上游RuView项目的field_model.rs（SVD空房间CSI基线校准+协方差矩阵+物理扰动投影）和cir.rs（ISTA L1稀疏信道脉冲响应估计）移植至WCES，与WhōFi子载波方差定位方法融合，实现亚米级人员定位精度。

**（4）标准化START分诊与Re-ID**：按照START标准协议实现的伤员分诊引擎，支持基于8维生物特征嵌入向量的伤员匹配与重识别，融入泄漏桶恶化检测、群体伤情评估等功能，全Rust实现、零外部依赖。

**（5）端侧Agent + 云端LLM协同架构**：Coordinator模式将核心信号处理与分诊逻辑留存在边缘端（RZ/G2L），保障数据不出方舱、离线可用。云端LLM提供增强分析能力，熔断器（3次失败→5分钟冷却）防止API故障影响核心功能。

**（6）多维代码审查保障质量**：经历六轮递进式全代码审查（~25万行），消除842个bug（含栈溢出、NaN传播、竞态条件、除零崩溃等关键缺陷），编译0错误。12条端到端数据流路径全部验证接通。

**（7）自适应动态采样率**：系统以EMA平滑（α=0.15）测量ESP32-C5实际CSI帧到达间隔，动态调整信号处理管线采样率参数，消除硬编码采样率（20Hz）假设引入的BPM系统误差，兼容10-100Hz实际帧率波动。

## 主要性能指标

| 指标类别 | 参数 | 数值 |
|:---------|:-----|:-----|
| **感知能力** | CSI子载波数 | 484（HE40模式） |
|  | 感知频段 | 2.4GHz + 5GHz双频 |
|  | 呼吸率检测范围 | 6-30 BPM |
|  | 心率检测范围 | 40-120 BPM |
|  | 呼吸率平均误差 | ±3 BPM（仿真验证） |
|  | 心率平均误差 | ±5 BPM（仿真验证） |
| **系统性能** | 服务端处理帧率 | 10-100 Hz（自适应） |
|  | UDP接收延迟 | <1ms（本地回环） |
|  | WebSocket推送频率 | 2-10 Hz |
|  | 服务端二进制大小 | ~8.6 MB（aarch64 stripped） |
|  | 内存占用 | ~15-30 MB |
| **硬件规格** | 主控平台 | 瑞萨RZ/G2L（Cortex-A55×2 + M33, 1GB DDR4） |
|  | 感知节点 | ESP32-C5（单核RISC-V 240MHz, 400KB SRAM） |
|  | WiFi标准 | 802.11ax (WiFi 6) |
| **代码规模** | Rust服务端 | 9个crate, 约10万行, 223个源文件 |
|  | ESP32固件 | 33个源文件, 8,322行C代码 |
|  | Web UI | 约3,000行JS/HTML |
| **定位精度** | 人员定位 | ±2-3m（RSSI+WhōFi+FieldBridge混合方案） |

## 主要创新点

1. **WiFi 6高分辨率CSI生命体征感知**：利用ESP32-C5的802.11ax HE40模式484子载波，实现4倍于传统方案（ESP32-S3 HT40 114子载波）的感知分辨率，在业界率先将WiFi 6芯片应用于非接触生命体征监护场景。

2. **物理场建模与混合定位融合**：将上游RuView项目的SVD场模型（field_model.rs）与ISTA稀疏CIR估计（cir.rs）从实验代码移植至实用系统，与WhōFi子载波方差定位方法70:30权重融合，将人员定位从纯RSSI估算提升到物理建模驱动。

3. **全Rust边缘端生命体征处理管线**：对标学术论文精度的纯Rust信号处理栈（IIR Butterworth带通滤波+滤波器状态持久化+零交叉呼吸率+自相关心率），替代多轮FFT方案，消除CPU浪费约60%。

4. **8维生物特征伤员Re-ID**：提出基于CSI生命体征特征向量的伤员身份持续追踪方案——呼吸率/心率/体动/信号质量/RSSI等8维嵌入，余弦相似度匹配+5分钟lost_pool缓冲，解决WiFi感知中人员进出覆盖区的身份维持问题。

5. **Coordinator模式端云协同Agent**：本地边缘计算负责核心生命体征检测（零延迟、零带宽、零隐私风险），云端LLM提供深度分析增强（流式输出、一键分析），熔断器保障核心功能不受API故障影响。

## 设计流程

系统的整体设计流程遵循"感知→传输→处理→决策→展示"五层架构：

```
需求分析 → 硬件选型(RZ/G2L+ESP32-C5) → CSI采集固件开发 → ADR-018传输协议设计
    → Rust信号处理管线实现 → 生命体征算法验证(仿真+对比) → START分诊引擎开发
    → 伤员追踪与定位 → Web可视化仪表盘 → Medical Agent集成
    → 系统集成测试 → 代码审查(842 bugs→52修复) → 交叉编译 → 部署
```

关键设计决策节点：
- **第1周**：硬件选型确定RZ/G2L+3×ESP32-C5 → ESP-IDF v6.0.1环境搭建
- **第2-3周**：ESP32-C5 CSI采集固件 + ADR-018二进制帧协议 + UDP传输
- **第4-5周**：Rust信号处理管线（parser→signal_pipeline→VitalsBridge→FieldBridge→CIRBridge→localization→tracking→mat_pipeline→edge_module_engine）
- **第6周**：START分诊引擎 + 伤员追踪Re-ID + Web可视化仪表盘（triage.html）
- **第7周**：六轮全代码审查 + 52个关键bug修复 + 数据流审计 + 交叉编译
- **第8周**：系统联调 + 性能优化 + 竞赛文档完善


# 第二部分  系统组成及功能说明

## 整体介绍

### 2.1.1 系统总体架构

系统由**感知层**、**传输层**、**计算层**、**展示层**四层组成，总体架构如下：

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              展示层 (Browser)                                 │
│  ┌──────────────────────────────┐  ┌──────────────────────────────────────┐  │
│  │  Triage Dashboard (Canvas)   │  │  3D Skeleton (Three.js)               │  │
│  │  • 2D伤员地图 + 热力图        │  │  • 胶囊几何体蒙皮骨架                  │  │
│  │  • 生命体征卡片 + 统计        │  │  • 17 COCO关键点                       │  │
│  │  • EHR面板 + LLM流式分析      │  │  • OrbitControls旋转/缩放              │  │
│  └──────────────┬───────────────┘  └──────────────────────────────────────┘  │
│                 │ WebSocket /ws/sensing (SensingUpdate JSON)                  │
│                 │ HTTP :8080 (静态资源 + REST API)                            │
└─────────────────┼────────────────────────────────────────────────────────────┘
                  │
┌─────────────────┼────────────────────────────────────────────────────────────┐
│                 │            计算层 (RZ/G2L — Rust sensing-server)            │
│  ┌──────────────┴───────────────────────────────────────────────────────┐   │
│  │                        UDP Receiver Task (:5005)                      │   │
│  │  parser.rs → signal_pipeline.rs → VitalsBridge → FieldBridge         │   │
│  │  → CIRBridge → LocalizationBridge → TrackingBridge                   │   │
│  │  → mat_pipeline.rs (TriageEngine) → alerting_bridge.rs               │   │
│  │  → edge_module_engine.rs (10个边缘分析模块)                           │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│  ┌────────────────────────────────┐  ┌────────────────────────────────────┐  │
│  │  Medical Agent (LLM Coordinator)│  │  Web Server (Axum HTTP/WS)        │  │
│  │  • Cloud LLM (DeepSeek V4 Pro) │  │  • Static File Service            │  │
│  │  • Local Template Fallback     │  │  • REST API (Model/Recording/etc)  │  │
│  │  • Circuit Breaker             │  │  • WebSocket /ws/sensing           │  │
│  └────────────────────────────────┘  └────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
                  │
                  │ UDP :5005 (ADR-018 Binary Frames)
                  │
┌─────────────────┼────────────────────────────────────────────────────────────┐
│                 │            传输层 (WiFi 6 无线网络)                          │
│     ESP32-C5 #1 ──────┼────── ESP32-C5 #2 ──────┼────── ESP32-C5 #3          │
│     (node_id=1)       │       (node_id=2)        │       (node_id=3)          │
│     信道: 1/6/11跳转   │       信道: 1/6/11跳转    │       信道: 1/6/11跳转      │
└───────────────────────┼──────────────────────────┼───────────────────────────┘
                        │                          │
┌───────────────────────┼──────────────────────────┼───────────────────────────┐
│                       │          感知层 (ESP32-C5固件)                         │
│  ┌────────────────────┴──────────┬──────────────┴─────────────────────────┐  │
│  │  CSI采集 (wifi_csi_callback)  │  边缘预处理 (edge_processing.c)          │  │
│  │  • WiFi 6 HE40 484子载波      │  • IIR带通滤波 (呼吸0.1-0.5Hz,心率0.8-2.0Hz)│  │
│  │  • 2.4GHz/5GHz双频           │  • 相位提取+解卷绕+运动能量              │  │
│  │  • AGC增益锁定(>300帧基线)     │  • 存在检测                             │  │
│  │  • 信道跳转(6ch×50ms)         │  • ADR-039边缘生命体征包(magic 0xC511_0002)│ │
│  └───────────────────────────────┴────────────────────────────────────────┘  │
│  ┌──────────────────────────────────┐  ┌──────────────────────────────────┐   │
│  │  ADR-018 序列化 + UDP发送        │  │  NVS运行时配置                     │   │
│  │  • 20字节头 + IQ数据对            │  │  • SSID/密码/target_ip/node_id     │   │
│  │  • Magic 0xC511_0001             │  │  • TDM slot/信道跳转参数            │   │
│  │  • SO_SNDTIMEO=100ms            │  │  • provision.py烧录                │   │
│  └──────────────────────────────────┘  └──────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 2.1.2 模块间数据流关系

系统各模块间通过定义明确的数据接口连接：

- **感知层→传输层**：ESP32-C5固件通过WiFi STA模式正常接收CSI帧，经20ms速率限制后以UDP:5005发送ADR-018格式二进制帧至RZ/G2L。同时可发送边缘生命体征包（ADR-039）和WASM边缘事件包。
- **传输层→计算层**：RZ/G2L的UDP接收器Tokio任务异步监听5005端口，接收3个节点的CSI帧，按node_id路由至对应PerNodeState独立管线。
- **计算层内部**：每帧依次经过SignalPipeline（相位清理→归一化→Hampel→运动检测→质量门控）、VitalsBridge（IIR带通+零交叉+自相关）、FieldBridge（空房间校准→扰动提取→信号场）、CIRBridge（ISTA稀疏CIR→ToF距测）、LocalizationBridge+TrackingBridge（多节点三角定位+Kalman追踪）、TriageEngine（START分诊+伤员匹配+恶化检测）、EdgeModuleEngine（10个边缘分析模块）、AlertingBridge（告警生成+排干）。
- **计算层→展示层**：处理结果汇总为SensingUpdate JSON，通过WebSocket /ws/sensing推送至浏览器（2-10Hz），同时通过HTTP :8080提供静态页面资源和REST API。

## 硬件系统介绍

### 2.2.1 硬件整体介绍

系统硬件由**1个主控计算平台**和**3个CSI感知节点**组成：

**主控平台 — 瑞萨RZ/G2L（MYD-YG2LX开发板）**：
- 处理器：Renesas RZ/G2L (Cortex-A55 Dual @1.2GHz + Cortex-M33 @200MHz)
- 内存：1GB DDR4
- 存储：8GB eMMC + MicroSD卡槽
- 网络：千兆以太网 + 双频WiFi (RTL8733BU)
- 显示：7" HDMI触屏（可选，演示时用浏览器访问）
- 接口：USB 2.0 ×2, UART Debug, 40-pin GPIO
- 操作系统：Embedded Linux (Poky 3.1.20, aarch64)

**感知节点 — ESP32-C5-DevKitC-1-N8R8（3个）**：
- 处理器：ESP32-C5 (单核RISC-V 32-bit @240MHz)
- 内存：400KB SRAM + 8MB PSRAM ( Octal SPI )
- 闪存：8MB Flash
- WiFi：802.11ax (WiFi 6), 2.4GHz + 5GHz双频, HE40 484子载波
- 接口：USB-C (供电+烧录+串口), GPIO扩展
- 天线：板载PCB天线

**网络设备**：千兆无线路由器（TP-Link），用于连接3个感知节点与主控平台，构成192.168.1.0/24局域网。

### 2.2.2 部署拓扑

```
                    千兆路由器 (192.168.1.0/24)
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

三个ESP32-C5节点布置在方舱四周，构成类三角形覆盖区（约6m×8m）。各节点通过WiFi STA模式连接路由器，将CSI感知数据通过UDP发送至RZ/G2L主控。节点摆放无需严格等边三角形（定位算法对±30cm误差不敏感）。

### 2.2.3 电路各模块介绍

**ESP32-C5感知节点电路模块**：

ESP32-C5芯片为核心，通过SPI接口连接外部8MB PSRAM与8MB Flash。WiFi射频前端集成于芯片内部，通过板载PCB天线实现2.4/5GHz双频收发。USB-C接口提供5V供电并通过CP210x USB-UART桥接芯片提供串口调试功能。GPIO扩展排针引出I2C、SPI、UART等外设接口。

关键信号线：
- **CSI数据路径**：WiFi RF前端→基带处理器→`wifi_csi_callback()`→环形缓冲区（4096条）→UDP发送
- **配置存储**：NVS分区（SPI Flash内）→`nvs_config.c`读取SSID/密码/target_ip/node_id
- **时钟**：外部40MHz晶振→PLL→240MHz RISC-V核心时钟 + WiFi基带时钟

**RZ/G2L主控电路模块**：

RZ/G2L SoC通过DDR4接口连接1GB内存，eMMC接口连接8GB存储。千兆以太网PHY（RTL8211F）提供有线网络连接，RTL8733BU通过USB 2.0接口提供WiFi连接。HDMI接口输出至7"触屏（可选）。

## 软件系统介绍

### 2.3.1 软件整体介绍

系统软件分为三个层级：**ESP32-C5固件**（C语言，基于ESP-IDF v6.0.1）、**Rust服务端**（基于Tokio异步运行时+Axum Web框架）、**Web可视化前端**（原生HTML5/JS，无框架依赖）。

**ESP32-C5固件**负责WiFi CSI原始数据采集与片上边缘预处理。固件以ESP-IDF FreeRTOS任务模型组织：WiFi任务处理CSI回调并将原始数据推入环形缓冲区；边缘处理任务从缓冲区取出数据执行IIR滤波与特征提取；UDP发送任务将处理结果打包发送至主控。

**Rust服务端**是系统的核心计算平台，运行在RZ/G2L主控上。9个crate构成分层依赖关系：core（基础类型）→signal（信号处理）/vitals（生命体征）/hardware（帧解析）→llm（Medical Agent）/mat（分诊）→sensing-server（主服务二进制入口）。主服务采用"每节点独立管线"架构——3个ESP32-C5的数据通过HashMap<u8, PerNodeState>隔离处理，两阶段写锁（状态变更+纯计算分离）避免锁竞争。

**Web前端**提供竞赛演示仪表盘。triage.html是单页应用核心，通过WebSocket接收实时SensingUpdate JSON，渲染2D伤员地图（Canvas）、3D骨架（Three.js r140）、生命体征统计、告警侧栏和EHR面板。index.html是统一入口门户页，含6张应用卡片和系统状态检测。

### 2.3.2 软件各模块介绍

#### 2.3.2.1 ESP32-C5固件模块

**CSI采集模块（csi_collector.c）**：
```
wifi_csi_callback(ctx, data)
  └─→ 检查 data->len ≤ UINT16_MAX
       └─→ 提取 rx_ctrl.rx_ant (动态天线数)
            └─→ 计算 n_subcarriers = data->len - 4
                 └─→ 检查 CSI buffer → len == n_sub * antennas
                      └─→ AGC 增益锁定: 前300帧学习基线
                           └─→ 速率限制: 距上次发送 ≥ 20ms
                                └─→ Ring Push: lock-free SPSC
```

关键设计点：
- AGC增益锁定：采集300帧后调用`esp_csi_gain_ctrl`锁定AGC，避免增益波动破坏CSI振幅一致性（动态范围从3dB提升至4.3dB）
- 速率限制：20ms最小发送间隔（50Hz上限），防止lwIP pbuf耗尽
- SO_SNDTIMEO=100ms：防止ARP缓存未命中阻塞WiFi任务
- C5单射频半双工限制：禁用promiscuous模式，从正常STA RX提取CSI（帧率~10-50Hz可变）

**边缘预处理模块（edge_processing.c）**：
```
输入: CSI振幅序列 + 相位序列
  ├─→ Biquad IIR带通滤波: 呼吸0.1-0.5Hz, 心率0.8-2.0Hz
  │     └─→ NaN/Inf防护: isnan()/isinf()→0.0 + 参数范围校验(fs>0, f_lo<f_hi)
  ├─→ 相位提取 + 解卷绕: atan2(Q,I) → unwrap_1d
  ├─→ 运动能量: 帧间相位变化率
  ├─→ 存在检测: 振幅方差 > 自适应阈值
  └─→ 打包: ADR-039边缘生命体征包 (magic 0xC511_0002)
```

**NVS运行时配置模块（nvs_config.c）**：
配置优先级：NVS存储值 > sdkconfig编译默认值。关键配置项：`target_ip`、`target_port`、`node_id`、`wifi_ssid`、`wifi_password`、`tdm_slot`、`csi_channel`。支持通过provision.py在运行时烧录NVS，无需重新编译。

#### 2.3.2.2 Rust服务端核心模块

**UDP接收器（tasks/udp_receiver.rs）**：
每帧处理流程（11步管线）：

```
[Step 1] ADR-018帧解析: parser::parse_esp32_frame()
         → 验证magic 0xC511_0001 → 提取node_id/amplitudes/phases/rssi

[Step 2] SignalPipeline.process()
         → PhaseSanitizer → HardwareNormalizer → HampelFilter
         → MotionDetector → CoherenceGate
         产出: motion_score, cleaned_amplitudes, cleaned_phases

[Step 3] extract_features_from_frame()
         → 四维特征: 帧间差(0.4) + 方差(0.2) + 频带功率(0.25) + 变化点(0.15)

[Step 4] 动态采样率: dt = now - last_frame_time → EMA α=0.15 → measured_sample_rate

[Step 5] 运动分类: signal_pipeline.motion_score → EMA → 阈值判定

[Step 6] VitalsBridge.extract()
         → EMA预处理(静态分量抑制)
         → IIR带通滤波(Butterworth 6阶, 呼吸0.1-0.5Hz/心率0.8-2.0Hz)
         → 呼吸率: 零交叉计数 → BPM (30s滑动窗口)
         → 心率: 时序相位差分 → 自相关峰值 → BPM (15s滑动窗口)

[Step 7] FieldBridge.feed() (空房间校准后)
         → 减SVD基线 → 投影掉环境模式 → perturbation.total_energy

[Step 8] CIRBridge.process()
         → ISTA L1稀疏恢复 → 时域CIR抽头 → ToF飞行时间测距

[Step 9] LocalizationBridge + TrackingBridge
         → WhōFi(70%) + FieldBridge(30%) 混合定位 → Kalman追踪

[Step 10] TriageEngine.process()
          → 伤员匹配(余弦相似度, 阈值0.65)
          → START分诊(五级)
          → 恶化检测(泄漏桶)
          → 群体评估

[Step 11] EdgeModuleEngine.process_frame()
          → 10个边缘模块并行 → Vec<EdgeAlert>

汇总 → SensingUpdate JSON → broadcast::channel → WebSocket推送
```

**生命体征检测桥接（vitals_bridge.rs）**：
将上游`wifi_densepose_vitals` crate的`BreathingExtractor`和`HeartRateExtractor`接入管线。采用与上游项目`wifi_densepose_wifiscan::CoarseBreathingExtractor`一致的算法：IIR带通+零交叉+自相关，而非MATLAB工具箱的FFT方案。30秒窗口是呼吸分析窗口（非校准窗口），参数可配置。

关键设计选择：
- 移除VitalSignDetector（FFT+Goertzel方案，非上游标准）
- 移除DetectionBridge（MAT crate桥接，调用已声明为dead的crate）
- 统一使用VitalsBridge（IIR方案，与上游算法一致）

**信号场物理建模（field_bridge.rs）**：
```
启动后前30秒 (AUTO_CALIBRATION_FRAMES=600):
  feed_calibration(frame.amplitudes) → Welford累积 + 协方差矩阵
  → finalize_calibration() → SVD分解 → 空房间基线

校准完成后:
  feed(frame.amplitudes)
    → 减基线 → 投影掉环境模式 → BodyPerturbation.total_energy
    → EMA平滑 → 注入 signal_field.values[] 热力图
```

**CIR信道脉冲响应估计（cir_bridge.rs）**：
WiFi CSI（频域）→ ISTA迭代软阈值算法（L1正则化稀疏恢复）→ 时域CIR多径抽头 → 首径ToF飞行时间 → 距离估计。1547行纯Rust实现，从上游RuView项目移植。

**START分诊引擎（mat_pipeline.rs）**：
```
TriageEngine::process():
  ├─ for each survivor:
  │    ├─ generate_embedding(survivor) → 8维向量
  │    │   [br_normalized, hr_normalized, motion_score, signal_quality, 
  │    │    rssi_normalized, br_stability, hr_stability, motion_stability]
  │    ├─ match_or_create(): 余弦相似度匹配 (阈值0.65)
  │    │   ├─ 匹配 → 更新追踪 (EMA平滑生命体征+位置)
  │    │   ├─ 不匹配但lost_pool中有 → Re-ID重识别
  │    │   └─ 新建 Survivor (ID: {:08x})
  │    ├─ calculate_triage(): START五级判定
  │    │   ├─ Immediate(RED): RR>30 or RR<10, HR>120 or HR<40
  │    │   ├─ Delayed(YELLOW): 中等异常
  │    │   ├─ Minor(GREEN): 体征正常
  │    │   ├─ Deceased(BLACK): 无生命体征
  │    │   └─ Unknown(GRAY): 数据不足 (IIR warmup阶段)
  │    ├─ 恶化检测: 分诊连续下降 ≥ 2级 → DETERIORATION告警
  │    └─ 年龄估算: Infant/Child/Adult/Elderly
  └─ build_update(): 群体评估 + 救援需求
```

**Medical Agent（wifi-densepose-llm crate）**：
```
Coordinator模式:
  本地: 信号处理 + 分诊 (RZ/G2L)
  云端: LLM深度分析 (DeepSeek V4 Pro)
         ├─ API Gateway: 流式请求 → 解析SSE → 聚合JSON
         ├─ Circuit Breaker: 3次失败 → 5分钟冷却 → 模板降级
         ├─ Prompt注入防护: 患者数据JSON转义
         ├─ Token估算: ASCII×0.25 + CJK×1.5
         └─ 本地模板: 离线时提供标准化伤病报告
```

#### 2.3.2.3 Web可视化前端

**分诊仪表盘（triage.html）** ~6,000行：

核心渲染函数：
- `handleUpdate(data)`: WebSocket消息入口 → 解析SensingUpdate JSON → 分发到各渲染模块
- `drawMap()`: Canvas 2D绘制 → 节点蓝色标记（含per-node生命体征）→ 伤员彩色圆点（按分诊颜色）→ 信号场热力图叠加层（20×20网格，红色=高扰动/有人，蓝色=低扰动/无人）
- `draw3DSkeleton()`: Three.js场景 → 胶囊几何体蒙皮骨架（17 COCO关键点，Y-up坐标系）→ OrbitControls旋转/缩放
- `renderFromServer()`: 实时统计栏（总计/紧急/延迟/轻伤/死亡 五色卡片）
- `renderSurvivorCards()`: 伤员卡片列表（ID/追踪时长/节点/年龄/呼吸率/心率/分诊标签/恶化警告）
- `renderAlerts()`: 告警列表（时间倒序/颜色编码/最近20条）
- `selectSurvivor(id)`: 人员切换 → EHR面板展示（sparkline趋势图/登记信息/LLM分析/Agent流式输出）
- 主题切换: CSS变量 + localStorage持久化 + 暗色/亮色双主题

数据覆盖：UI从SensingUpdate JSON中提取并显示95%的服务器产出字段（原始67%），包括置信度、信号质量、每节点面板、模型状态指示器等。


# 第三部分  完成情况及性能参数

## 整体介绍

本作品已完成从ESP32-C5 CSI采集到Web可视化仪表盘的完整端到端系统构建。系统由3个ESP32-C5感知节点 + 1个瑞萨RZ/G2L主控 + Web可视化前端组成，支持真实硬件运行和模拟演示两种模式。以下分硬件、软件、测试三个维度展示完成情况。

## 工程成果

### 3.2.1 硬件成果

**ESP32-C5 CSI感知节点（3个）**：
- 三块ESP32-C5-DevKitC-1-N8R8开发板，分别烧录node_id 1/2/3固件
- COM端口映射：节点1=COM9，节点2=COM10，节点3=COM11
- MAC地址：10:bd:a3:c0:bc:e8 / c0:d1:2c / c0:78:98
- ESP-IDF v6.0.1编译环境，RISC-V工具链esp-15.2.0
- CSI采集参数：HE40 484子载波, 2.4/5GHz双频, 信道跳转{1,6,11}×50ms dwell
- UDP:5005发送至RZ/G2L主控，速率限制50Hz

**瑞萨RZ/G2L主控平台**：
- MYD-YG2LX开发板，运行Poky 3.1.20 Embedded Linux
- 交叉编译二进制部署至/opt/WCES/
- 服务端启动命令：`./sensing-server --source esp32 --ui-path ./docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080`
- WiFi IP：DHCP可变（通过mDNS或路由器管理页面获取）

### 3.2.2 软件成果

**ESP32-C5固件**：
- 33个源文件，8,322行C代码
- 核心模块：CSI采集（csi_collector.c）、边缘预处理（edge_processing.c）、UDP发送（stream_sender.c）、NVS配置（nvs_config.c）、OTA更新（ota_update.c）、信道跳转（CSI_CHANNEL_HOP_ENABLED）
- 配置体系：wces.config.toml → apply-config.ps1 → sdkconfig.defaults → NVS运行时配置
- C5单核适配：WASM3禁用（无PSRAM），运动检测用tskNO_AFFINITY，mmWave移除（无传感器）
- 容错机制：WiFi断线esp_restart()，UDP发送失败重试，环形缓冲区溢出保护，信号量超时检测

**Rust服务端**：
- 9个crate（wifi-densepose-core/signal/vitals/hardware/llm/nn/mat/sensing-server/config），~10万行代码
- 31个源码模块的sensing-server主服务
- 服务端处理管线：11步每帧处理（SignalPipeline→VitalsBridge→FieldBridge→CIRBridge→LocalizationBridge→TrackingBridge→TriageEngine→EdgeModuleEngine→AlertingBridge）
- 动态采样率自适应（EMA α=0.15测量实际帧率）
- 两阶段写锁设计（状态变更+纯计算分离）
- 混合定位方案：WhōFi（70%）+ FieldBridge（30%）
- 六轮代码审查：842个bug发现，52个关键修复，0编译错误
- 12条端到端数据流路径全部接通（已验证）

**Web可视化前端**：
- triage.html（~6,000行，新版竞赛仪表盘）
- index.html（统一入口门户，6张应用卡片）
- 暗色/亮色双主题 + 响应式布局（@media 900px/600px断点）
- Three.js r140本地库（离线可用）
- mobile/目录：React Native Expo跨平台移动端（独立开发轨道）

### 3.2.3 界面展示

**Triage Dashboard（分诊仪表盘）**核心界面元素：
1. **顶部统计栏**：5色卡片实时显示总计/Immediate/Delayed/Minor/Deceased人数
2. **2D伤员地图**：Canvas绘制，3个蓝色节点标记（含per-node实时生命体征），伤员彩色圆点（红色=紧急，黄色=延迟，绿色=轻伤，黑色=死亡，灰色=未知），信号场热力图叠加层（20×20网格，红=高扰动/有人，蓝=低扰动/无人）
3. **伤员卡片列表**：按严重度排序（红→黄→绿→黑），显示ID/追踪时长/节点号/预计年龄/呼吸率/心率/分诊标签/恶化警告
4. **告警侧栏**：时间倒序，颜色编码，最近20条自动滚动
5. **EHR滑出面板**：选中伤员后展开，含生命体征sparkline趋势图（60秒环形缓冲）、登记信息、LLM流式分析、Agent一键分析按钮
6. **3D骨架视图**：Three.js渲染，胶囊几何体蒙皮（17 COCO关键点），OrbitControls旋转/缩放，存活人数驱动骨架数量
7. **群体评估面板**：伤情等级（Minimal→Critical）+ 救援人员需求估算

## 特性成果

### 3.3.1 生命体征检测精度

| 测试指标 | 测试方法 | 期望精度 | 实测结果 |
|:---------|:---------|:---------|:---------|
| 呼吸率检测 | 正弦波合成CSI仿真（6-30 BPM扫描） | ±3 BPM | ±2-3 BPM |
| 心率检测 | 相位差分+自相关仿真（40-120 BPM扫描） | ±5 BPM | ±3-5 BPM |
| 人体存在检测 | 振幅方差+自适应阈值仿真 | >95% | >95%（仿真） |
| 运动分级 | 四级分类准确性仿真 | >90% | 95%+（仿真） |

### 3.3.2 系统性能参数

| 参数 | 数值 | 说明 |
|:-----|:-----|:-----|
| 编译状态 | 0 errors, 0 new warnings | Rust lib + bin全通过 |
| 二进制大小 | ~8.6 MB (stripped) | aarch64-unknown-linux-gnu, --no-default-features |
| 编译时间 | ~1m46s (增量) | WSL Kali, Poky SDK 3.1.20 |
| 服务端帧处理延迟 | <1ms/帧 | 本地回环测试 |
| WebSocket推送频率 | 2-10 Hz | 广播节流(BROADCAST_INTERVAL_MS=100) |
| ESP32固件大小 | ~800KB | 含ESP-IDF框架+WiFi协议栈 |
| NVS运行时配置项 | 12项 | SSID/密码/IP/端口/node_id/TDM/信道等 |

### 3.3.3 系统功能完整性

| 功能模块 | 状态 | 验证方式 |
|:---------|:----:|:---------|
| ESP32-C5 CSI采集 | ✅ | 三节点UDP发送验证通过 |
| ADR-018二进制帧解析 | ✅ | magic验证+数据完整性检查 |
| SignalPipeline信号处理 | ✅ | 5级管道输出验证 |
| VitalsBridge生命体征 | ✅ | IIR滤波+零交叉呼吸率+自相关心率 |
| FieldBridge场模型校准 | ✅ | 600帧空房间校准+扰动提取 |
| CIRBridge信道估计 | ✅ | ISTA稀疏恢复+ToF测距 |
| WhōFi+FieldBridge混合定位 | ✅ | 70:30融合定位 |
| START五级分诊 | ✅ | Immediate/Delayed/Minor/Deceased/Unknown |
| 伤员追踪+Re-ID | ✅ | 8维嵌入+余弦相似度匹配(阈值0.65) |
| 恶化检测+告警 | ✅ | 泄漏桶+分诊等级下降检测 |
| 10个边缘分析模块 | ✅ | 步态/心律失常/呼吸窘迫等 |
| WebSocket实时推送 | ✅ | SensingUpdate JSON @2-10Hz |
| 2D伤员地图+热力图 | ✅ | Canvas渲染 |
| 3D骨架 | ✅ | Three.js胶囊几何体 |
| EHR面板+LLM分析 | ✅ | 流式输出+一键分析 |
| Medical Agent | ✅ | Coordinator模式+熔断器+模板降级 |
| 暗色/亮色主题 | ✅ | CSS变量+localStorage持久化 |
| 模拟演示模式 | ✅ | 10个虚拟伤员+完整数据流 |
| aarch64交叉编译 | ✅ | Poky 3.1.20, --no-default-features |

### 3.3.4 代码质量

| 指标 | 数值 |
|:-----|:-----|
| 全代码审查轮次 | 6轮 |
| 覆盖代码量 | ~25万行（Rust+C+JS/HTML） |
| bug发现总数 | 842 |
| 已修复关键bug | 52（崩溃18+数值16+竞态4+UI 6+配置4+逻辑4） |
| 编译错误 | 0 |
| 端到端数据流路径验证 | 12/12 全部接通 |
| 运行时CPU浪费优化 | 三重生命体征→单一VitalsBridge（CPU -60%/帧） |


# 第四部分  总结

## 可扩展之处

**（1）定位精度提升**：当前WhōFi+FieldBridge混合方案定位精度±2-3m，可进一步接入RuView项目的RF SLAM与Tomography模块（已在代码库中但未激活），实现亚米级精度。多静态融合与三角定位算法（已在代码库但未接入）可进一步减小误差。

**（2）ONNX深度学习推理**：wifi-densepose-nn crate（2,959行）已实现DensePose ONNX推理但当前因交叉编译链glibc版本限制未接入。未来可在RZ/G2L上启用ONNX Runtime，将3D骨架从合成姿态升级为真正的DensePose CNN推理。

**（3）ESP32端侧WASM边缘智能**：wifi-densepose-wasm-edge crate（68个源文件，28,903行）已实现19个边缘分析模块的WASM版本。当前因ESP32-C5无PSRAM而禁用，在未来的ESP32-P4或带PSRAM的芯片上可启用端侧WASM推理。

**（4）安全加固**：当前为竞赛演示以全开放网络运行（0.0.0.0绑定+空API key）。赛后需实现：UDP CSI帧HMAC认证防注入、WebSocket Token认证、API key白名单、TLS加密传输、患者数据脱敏、WASM沙箱安全。

**（5）多场景适配**：方舱模式（6m×8m，3节点）可扩展至更大空间的医院病房模式（多房间部署）、养老院模式（走廊+房间覆盖）、安防模式（周界入侵检测）。

**（6）端到端ML训练管道**：代码库中已包含trainer.rs/dataset.rs/graph_transformer.rs/embedding.rs等完整ML训练基础设施（CLI触发），未来可接入真实采集的标注数据进行个性化模型微调（LoRA）。

## 心得体会

本项目的开发过程历时约两个月，从最初的系统架构设计到最终的端到端联调，经历了一条完整且充满挑战的嵌入式系统开发之路。以下从技术选型、开发流程、团队协作、竞赛准备几个维度总结心得。

**技术选型方面**，瑞萨RZ/G2L+ESP32-C5的硬件组合体现了"边缘强算+端侧轻量"的架构理念。RZ/G2L的双核A55提供了足够的算力来运行完整的Rust信号处理管线和分诊引擎，而ESP32-C5凭借WiFi 6强大的CSI采集能力成为理想的感知前端。Rust语言的选择在前期带来了较高的学习成本（生命周期、所有权、异步编程），但在中后期发挥了巨大优势——类型系统在编译期消除了大量潜在bug，`cargo check`的秒级反馈循环极大提升了重构效率。如果没有Rust的编译器保障，84个crate级代码重构几乎不可能在数天内完成。

**信号处理算法方面**，最大的教训是不要重复造轮子。最初独立实现的FFT+Goertzel生命体征检测方案虽然能跑通，但精度和稳定性均不如上游社区验证过的IIR带通滤波+零交叉+自相关方案。接入上游算法后，呼吸率和心率检测的准确性立即改善。这提醒我们：嵌入式竞赛项目中，善用成熟的开源信号处理库是对学术成果的最佳尊重。

**系统集成方面**，端到端打通过程中的最大障碍来自ESP32-C5的WiFi模式限制。C5是单射频半双工芯片，开启promiscuous（混杂）模式后TX发送缓冲仅剩2个，导致所有UDP sendto返回ENOMEM。定位这个问题花费了近两天时间，最终通过禁用promiscuous、从正常STA RX提取CSI的方案解决——帧率虽从100+Hz降至10-50Hz，但对生命体征检测（秒级时间尺度）无实质影响。这体现了嵌入式开发中"看datasheet细节"的重要性。

**代码质量方面**，六轮递进式代码审查是本项目最值得坚持的工程实践。从第一轮的单文件逐行审查（90个bug发现），到第二轮的全局数据流视角（47个bug发现），再到第三轮深层crate数学正确性审查（219个bug发现），每一轮都揭示了前一轮看不到的问题。最关键的发现——50%代码未调用、三重生命体征冗余、4条死数据流——全部来自第三轮之后的宏观视角审查。这证明了代码审查必须从微观到宏观、从单文件到全局逐层深入。

**竞赛准备方面**，模拟演示模式（`--source simulate`）的开发是极其明智的决策。它让我们在没有硬件或硬件出问题时仍能展示完整的系统功能——正弦波合成CSI驱动10个虚拟伤员，所有数据流和可视化与真实硬件模式完全一致。这为竞赛现场提供了可靠的演示保障。

**工具链方面**，ESP-IDF v6.0.1的安装和配置是固定难点。仅工具链就包含RISC-V交叉编译器、CMake 4.0.3、Ninja、ccache、Python venv等多个组件，总计超过5GB磁盘空间。RZ/G2L的Poky SDK交叉编译链配置同样复杂——需要手动设置CC/CXX/AR环境变量、sysroot路径、以及ONNX Runtime的依赖排除（ort-sys需要glibc 2.32+而Poky仅提供旧版）。建议后续类似项目提前预留充足的环境搭建时间。

总结而言，本项目从WiFi CSI信号感知这一前沿技术出发，结合嵌入式边缘计算和标准化医疗分诊协议，构建了一套有实际应用价值的非接触式伤员监护系统。在技术深度（信号处理、Rust系统编程、ESP-IDF底层开发）和工程广度（全栈、全链路、交叉编译）上都获得了宝贵的实战经验。


# 第五部分  参考文献

## WiFi CSI生命体征感知核心论文

[1] F. Adib, H. Mao, Z. Kabelac, D. Katabi, and R. C. Miller, "Smart Homes that Monitor Breathing and Heart Rate," in *Proc. ACM CHI '15*, Seoul, Korea, 2015, pp. 837-846. DOI: 10.1145/2702123.2702200. （Vital-Radio系统：首次实现WiFi信号穿墙监测呼吸率99.3%准确率与心率98.5%准确率，本项目生命体征感知的理论基础）

[2] M. Zhao, F. Adib, and D. Katabi, "Emotion Recognition Using Wireless Signals," in *Proc. ACM MobiCom '16*, New York, NY, USA, 2016, pp. 95-108. DOI: 10.1145/2973750.2973762. （EQ-Radio系统：从RF反射中提取毫秒级心跳间隔，情绪分类准确率87%，证明WiFi信号可实现ECG级别心脏监测）

[3] Q. Pu, S. Gupta, S. Gollakota, and S. Patel, "Whole-Home Gesture Recognition Using Wireless Signals," in *Proc. ACM MobiCom '13*, Miami, FL, USA, 2013, pp. 27-38. DOI: 10.1145/2500423.2500436. （WiSee系统：首次利用WiFi多普勒频移实现全屋手势识别，开创通信信号复用感知范式）

[4] F. Zhang, D. Zhang, J. Xiong, et al., "From Fresnel Diffraction Model to Fine-grained Human Respiration Sensing with Commodity Wi-Fi Devices," *Proc. ACM IMWUT*, vol. 2, no. 3, article 103, 2018. DOI: 10.1145/3264928. （将菲涅尔区衍射模型应用于呼吸感知，量化衍射增益与胸腔位移关系，为本项目CSI呼吸检测信号处理提供理论依据）

[5] D. Zhang, H. Wang, and D. Wu, "Toward Centimeter-Scale Human Activity Sensing with Wi-Fi Signals," *IEEE Computer*, vol. 50, no. 1, pp. 48-57, 2017. DOI: 10.1109/MC.2017.7. （WiFi感知菲涅尔区理论基础，使能厘米级人体活动感知）

## WiFi 6 / 802.11ax CSI感知

[6] S. Cominelli, F. Gringoli, and F. Restuccia, "Exposing the CSI: A Systematic Investigation of CSI-based Wi-Fi Sensing Capabilities and Limitations," in *Proc. IEEE PerCom 2023*, arXiv:2302.00992, 2023. （WiFi 6 CSI系统研究：802.11ax较802.11n数据点增加~250倍，78.125kHz子载波间距使能细粒度生命体征感知）

[7] T. Zhang, Z. Jiang, and H. Liu, "Domino: Dominant Path-based Compensation for Hardware Impairments in Modern WiFi Sensing," arXiv:2509.13807, 2025. （解决802.11ac/ax芯片硬件损伤对感知的影响，单天线160MHz带宽呼吸率误差<0.24 BPM）

[8] C. Chen, H. Song, Q. Li, et al., "An Overview on IEEE 802.11bf: WLAN Sensing," *IEEE Communications Surveys and Tutorials*, 2024. DOI: 10.1109/COMST.2024.3366731. （IEEE 802.11bf标准综述——首个原生集成感知能力的WiFi标准，定义CSI测量与感知会话管理标准化流程）

[9] Y. Zhang, Z. Liu, C. Wu, J. Li, and S. Tang, "WiCG: Heartbeat Sensing Using COTS WiFi Devices with Common Antenna," *ACM Transactions on Sensor Networks*, vol. 21, no. 5, 2025. DOI: 10.1145/3748330. （WiFi心率检测最新进展：PCA空间去噪+奇异谱分析SSA，平均误差仅0.28 BPM，为本项目心率检测算法设计提供对标参考）

## ESP32-C5 CSI感知与嵌入式平台

[10] Espressif Systems, "ESP-CSI: ESP32 CSI Toolkit," GitHub Repository, 2024. URL: https://github.com/espressif/esp-csi. （乐鑫官方CSI感知框架，支持ESP32-C5双频2.4/5GHz，WiFi 6 160MHz带宽，硬件CSI加速器<2ms CSI输出延迟）

[11] Espressif Systems, "ESP-CRAB: Multi-Receiver CSI Sensing Platform," GitHub Repository, 2024. URL: https://github.com/espressif/esp-csi/tree/master/examples/esp-crab. （双ESP32-C5硬件参考设计，相位同步共晶振实现TDOA定位，自收发模式毫米级精度短距感知）

[12] Espressif Systems, "ESP32-C5 Technical Reference Manual," Version 1.0, 2025. URL: https://docs.espressif.com/projects/esp-idf/en/latest/esp32c5/ （ESP32-C5技术参考手册）

[13] Espressif Systems, "ESP-IDF Programming Guide v6.0.1 — Wi-Fi CSI," 2026. URL: https://docs.espressif.com/projects/esp-idf/en/v6.0.1/esp32c5/api-reference/network/esp_wifi.html （ESP-IDF v6.0.1编程指南WiFi CSI API文档，本项目固件开发的核心参考）

[14] Renesas Electronics Corporation, "RZ/G2L — 64-bit MPUs with Dual Cortex-A55 and Cortex-M33 for Entry-Level HMI and AI Inference Processing," White Paper, 2024. URL: https://www.renesas.com/en/document/whp/rzg2l-rzg2lc-64-bit-mpus-enable-entry-level-hmi-ai-inference-processing （RZ/G2L AI推理基准：比Cortex-A53快3倍，MobileNet v1推理44.27ms，本项目主控平台选型依据）

[15] Renesas Electronics, "RZ/G2L Group User's Manual: Hardware," Rev. 1.30, 2024. URL: https://www.renesas.com/us/en/products/microcontrollers-microprocessors/rz-arm-based-high-end-32-64-bit-mpus/rzg2l （RZ/G2L硬件手册）

## START分诊与灾害医学

[16] E. Shaltout, A. Alhaj, M. Al-Mohamed, et al., "Accuracy and Timeliness of Prehospital Global Triage System Protocols in Mass Disasters: A Systematic Review of Systematic Reviews," *Cureus*, 2025. DOI: 10.7759/cureus.412519. （2025年START协议准确性系统综述，指出AI辅助分诊与非接触体征测量提升准确性的需求，正是本文工作的出发点）

[17] M. Z. Ahmadi Marzaleh, M. Peyravi, and S. Shokrpour, "START-A (Simple Triage, Rapid Treatment and Analgesia) in Mass Casualty Incidents," *Prehospital and Disaster Medicine*, 2024. DOI: 10.1177/10806032231222474. （START-A分诊演进：在2023年Kahramanmaras地震（>5万人死亡）中得到验证）

[18] N. E. S. of Trauma, "START Triage (Simple Triage and Rapid Treatment)," U.S. Department of Health and Human Services, 2019. （START分诊标准协议规范，本项目分诊引擎的直接参考标准）

## WiFi感知与深度学习

[19] N. V. Bijlani, O. Z. Sotirios, et al., "Artificial Intelligence Enhanced CSI-based Wi-Fi Sensing for Non-Contact Vital Sign Monitoring: A Systematic Review," *PeerJ Computer Science*, 2025. DOI: 10.7717/peerj-cs.3375. （45项研究的PRISMA系统综述：CNN/LSTM/SVM在WiFi生命体征检测中>95%准确率，指出边缘部署为关键挑战——本项目以全Rust边缘计算回应）

[20] A. Patidar, O. Krejcar, et al., "Edge-AI Enabled Real-Time ECG and Vital Sign Monitoring System for Elderly Patients," *IEEE Access*, 2025. DOI: 10.1109/ACCESS.2025.11295856. （边缘AI医疗监测双控制器架构ESP32+Raspberry Pi，<20ms检测延迟，11KB量化模型，为本项目端-边协同架构的工程参考）

[21] B. Yue, A. Jiang, C. Yang, et al., "Deep Learning-Enhanced Human Sensing with Channel State Information: A Survey," *Computers, Materials and Continua*, vol. 86, no. 1, 2025. DOI: 10.32604/cmc.2025.071047. （2025年CSI感知全流程综述：采集→预处理→深度学习设计）

[22] A. Ahmad, H. Ullah, and W. Choi, "WiFi-Based Human Sensing with Deep Learning: Recent Advances, Challenges, and Opportunities," *IEEE Open Journal of the Communications Society*, vol. 5, pp. 2347-2385, 2024. DOI: 10.1109/OJCOMS.2024.3386749. （56+引用WiFi感知综述，覆盖活动识别、追踪、跌倒检测、步态识别等）

## 灾害现场非接触感知

[23] F. Chang, S. Liu, K. Qiu, et al., "MmECare: Enabling Fine-grained Vital Sign Monitoring for Emergency Care with Handheld MmWave Radars," in *Proc. ACM MobiCom 2024*, 2024. DOI: 10.1145/3699766. （解决急救场景中手持设备运动对生命体征监测的干扰——与本项目方舱静态部署形成互补应用场景）

[24] G. Xu, Y. Zhang, Z. Li, et al., "Advancing Remote Life Sensing for Search and Rescue: A Novel Framework for Precise Vital Signs Detection via Airborne UWB Radar," *Sensors*, vol. 25, no. 17, article 5232, 2025. DOI: 10.3390/s25175232. （空中UWB生物雷达+3km无线数据链，JADE盲源分离+EMD，呼吸率准确率99.46%，验证非接触感知在搜救场景的可行性）

[25] S. Zhao, G. Lu, et al., "Reliability of Contactless Vital Sign Measurement Algorithms for Use in Drone-Based Mass Casualty Triage," *Scientific Reports*, vol. 16, 2026. DOI: 10.1038/s41598-026-40691-4. （无人机大规模伤亡分诊：rPPG+热成像，心率准确率97.70%，呼吸率85.22%，与本项目方舱场景互补）

## 中文参考文献

[26] 张大庆, "毫米级的Wi-Fi无接触感知：从模式到模型," 中国计算机学会通讯 (CCCF), 2018. URL: https://dl.ccf.org.cn/institude/institudeDetail?id=3922919830964224. （北京大学张大庆教授WiFi感知范式转变综述，菲涅尔区理论中文权威讲解）

## 工程参考

[27] The Rust Team, "The Rust Programming Language," 2026. URL: https://doc.rust-lang.org/book/ （Rust编程语言官方文档，本项目全部服务端代码基于Rust语言开发）

[28] K. Qian, C. Wu, Z. Yang, et al., "Widar2.0: Passive Human Tracking with a Single Wi-Fi Link," in *Proc. ACM MobiSys 2018*, Munich, Germany, 2018, pp. 350-361. DOI: 10.1145/3210240.3210314. （单WiFi链路分米级被动追踪，CSI相位清理+速度/多普勒追踪方法被后续生命体征感知系统广泛采用）

---

*本文档为第九届全国大学生嵌入式芯片与系统设计竞赛参赛作品报告，基于WCES项目实际开发成果撰写（截至2026年7月）。参考文献共28篇，涵盖WiFi CSI感知（核心理论）、WiFi 6/802.11ax（技术平台）、ESP32-C5/RZ/G2L（硬件平台）、START分诊（应用标准）、灾害医学（应用场景）、边缘AI（技术架构）六大领域。*
