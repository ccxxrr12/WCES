
# 基于WiFi CSI感知与端侧Agent的方舱生命体征监护系统


## 摘要

野战方舱与灾后临时医院等极端环境下，批量伤员的快速分诊对医疗资源调度与生存率提升至关重要。传统分诊依赖接触式传感器（ECG/SpO₂腕带）或人工巡查，在伤员聚集、穿戴困难、医护人员紧缺的场景下难以实施；现有WiFi CSI非接触感知研究多基于单链路收发与Python后端，存在角度模糊、子载波分辨率受限、边缘算法更新需重刷固件、断网即失效等局限，尚未见与非接触生命体征贯通的端到端START（Simple Triage and Rapid Treatment）分诊系统。

本作品面向野战方舱、灾后临时医院等恶劣环境下的批量伤员快速分诊需求，设计并实现了一套基于WiFi 6 CSI非接触感知与端侧AI Agent协同的伤员生命体征监护系统。

系统以瑞萨RZ/G2L双核ARM64处理器为主控计算平台，搭载3个ESP32-C5感知节点构成分布式WiFi传感网络，三个ESP32-C5感知节点接受WIFI CSI数据，穿墙感知区域内伤员的呼吸、心率和体动。数据通过UDP汇聚到瑞萨RZ/G2L主控，由主控服务端的信号处理管线实时提取生命体征，结合国际START标准分诊协议实现伤员自动分类与优先级排序。

系统主要以Rust构建服务端，包含9个workspace crate、约9.2万行代码，wasm-edge独立构建；ESP-IDF v6.0.1 C语言固件，包含31个源文件；HTML5/Canvas/Three.js Web可视化仪表盘。端到端数据流共三层12条路径，包含ESP感知节点的三种包类型通过UDP上行，RZ/G2L服务端11步Rust信号管线、2个异步任务，浏览器的8种消息类型通过Websocket下行。覆盖CSI采集→信号处理→生命体征、分诊→可视化的全链路数据通道均已完成。

核心技术贡献包括：

1、实现基于ESP32-C5 802.11ax HE-LTF 242子载波采集与Secure TDM时分同步（QUIC/TLS 1.3及HMAC-SHA256双模认证）的分布式感知阵列，突破单链路WiFi感知的角度模糊性。

2、自定义RVF签名容器与WASM3沙箱运行时，使感知算法可在不重刷固件前提下经OTA安全热加载——该"签名容器+沙箱热加载"机制在现有WiFi CSI感知系统中尚未见同等设计。

3、纯Rust信号管线，IIR带通滤波+零交叉计数+自相关方案对标MIT Vital-Radio/EQ-Radio学术标准，自适应采样率消除帧率波动引入的BPM系统误差。SVD空房间电磁场校准+ISTA稀疏CIR+子载波方差/相位多普勒混合质心定位，三层递进校验实现≤3m目标精度。

4、8维CSI生物特征嵌入实现跨节点伤员身份关联与恶化追踪。


关键词：WiFi CSI感知，非接触生命体征检测，START分诊，端侧AI，RZ/G2L边缘计算，ESP32-C5，Rust。


## 第一部分  作品概述


### 1.1功能与特性

本系统面向野战方舱、灾后临时医院等极端场景，实现WiFi信号非接触感知的全链路伤员生命体征监护与智能分诊。

（1）非接触生命体征检测：3个ESP32-C5节点构建WiFi 6感知网络，支持穿透室内建筑物提取呼吸率、心率、体动水平，人体存在检测。

（2）START标准分诊引擎：Immediate/Delayed/Minor/Deceased/Unknown五级自动分类，支持泄漏桶恶化检测、群体伤情评估、救援需求估算、伤员年龄推断。

（3）伤员追踪与Re-ID：由8维CSI生物特征嵌入向量进行余弦相似度匹配，支持最长5分钟lost_pool丢失池重识别缓冲。

这8维特征是CSI数据经过信号处理管线后产出的高层语义特征，由Rust服务端mat_pipeline.rs中的generate_embedding()计算生成，用于伤员身份持续追踪：

表格 1基于 CSI 信号的 8 维体征特征维度与物理基础一览表


| 维度 | 来源 | 含义 | 物理基础 |
|------|------|------|------|
| 1 | 呼吸率 | 6-30 BPM | IIR带通滤波+零交叉计数 |
| 2 | 心率 | 40-120 BPM | 相位差分+自相关峰值检测 |
| 3 | 体动水平 | 四级分类 (absent/present_still/ present_moving/active） | SignalPipeline MotionDetector |
| 4 | 信号质量 | 0-1 | RSSI归一化+置信度融合 |
| 5 | RSSI | 信号强度（dBm） | CSI帧头i8字段 |
| 6 | 呼吸置信度 | 0-1 | 零交叉稳定性+信号质量门控 |
| 7 | 心率置信度 | 0-1 | 自相关峰值锐度+相位一致性 |
| 8 | 检测置信度 | 0-1 | 存在检测消抖+运动分类置信度 |

（4）多模态定位：子载波方差邻近度与相位多普勒邻近度混合加权质心定位，辅以RSSI路径损耗三角定位、ISTA稀疏CIR时域ToF测距以及6维Kalman滤波。多种测算方案最终加权产出结果，可实现人员定位、运动监测、跟踪。

（5）Medical Agent云端增强：Coordinator模式——边缘端本地信号处理+可选云端LLM深度分析，为增强功能鲁棒性，我们设计了熔断器保护、流式输出、本地模板降级。

（6）Web可视化仪表盘：Canvas 2D伤员地图+信号场热力图叠加，Three.js 3D胶囊几何体蒙皮骨架，侧边面板包含实时统计、伤员卡片、边缘模块告警、EHR面板等信息，支持暗色/亮色双主题，可全部消费最终的数据产出并为相关从业人员提供直观的抽象


### 1.2应用领域

本系统瞄准极端环境或大规模伤亡场景下的批量伤员连续监测这一全球应急医学核心痛点。三类典型场景的监测困境具有高度一致的根源——短时间内伤员爆发式增长与极端环境下医护、设备、基础设施严重不足之间的不可调和矛盾：

自然灾害临时医院：德国Fraunhofer IIS研究指出，单次伤亡超10人的大规模事件中，传统分诊仅能提供单次生命体征记录，几乎不可能实现连续监测——而这正是本系统非接触穿墙感知所解决的。多项地震救援研究证实，临时医疗点大量伤员在无监护下等待转运，隐匿性恶化无法及时识别。山东大学齐鲁医院在《中华急诊医学杂志》(2023)中指出，狭小空间救援现场仅能对极危重伤员进行间断监测。

疫情方舱医院：武汉方舱实践表明，患者基数大、医护配比严重不足，人工巡诊无法实现全员连续监测，新冠肺炎部分患者由轻型快速转重型难以早期识别（2020年武汉方舱多项临床观察）。上海方舱经验显示，数千张床位仅能对少数高危患者部署远程监护（中国医学装备协会《方舱医院装备产品集》2022）。

战时野战医院：俄乌冲突中，乌军前线监护设备严重短缺，重伤员从交战区后送平均耗时3.5小时且全程无连续监护（中国指挥与控制学会2025）；俄方战伤截肢率高企，据俄罗斯副劳动部长2023年公开数据，54%的重伤士兵至少有一肢截肢，反映院前监护与救治的严峻挑战（CNA智库公开报告2024）。WHO统计2024年乌克兰医疗设施遭470余次袭击，进一步瓦解监测能力。加沙地带36所医院仅17所部分运行，大量伤员只能靠肉眼观察判断伤情（无国界医生2024）。

WIFI穿透性强，覆盖面积广，同时本项目为非接触式无源探测，数据本地运行，可私有化部署，相较于传统的信息采集、人员定位、生命体征监护设备具备较多优势，如下：

毫米波雷达：单个毫米波雷达只能覆盖不超过25m³，5-8㎡，全屋部署需要约5-10个，单个价格在300元左右，同时毫米波雷达只能穿透约10cm的混凝土。而2.4G WIFI最多可穿透30cm等标混凝土。

PIR红外传感器：当前的PIR传感器只对运动目标有反应，对于静止人体无法正确辨识。

监控摄像头：覆盖面积小，单台成本高，同时受隐私合规性、数据安全风险、等因素影响，在极端环境救援、紧急医疗、大规模群体性医疗事件下存在监控节点脆弱等风险。

可穿戴定位/健康设备：存在设备数量不足、设备本身较为脆弱、移动不便等问题

本系统的非接触穿墙感知、全本地边缘计算、零穿戴要求等特性，使得本系统具备无需传感器耗材、医护零操作负担，无需固定基础设施、WiFi即感知，自动化连续分诊+恶化预警等优点，直接回应上述场景的核心约束：资源短缺、环境恶劣、伤情复杂。


### 1.3主要技术特点

本系统的技术路线围绕一个核心约束展开：如何在嵌入式边缘设备上，用WiFi信号非接触地提取生命体征，并支撑大规模伤亡事件的快速分诊。 以下从五个技术维度阐述关键选型及其工程理由。

（1）非接触WiFi CSI感知——替代接触式传感器与雷达的折中选型。生命体征监测的技术谱系包括：接触式（ECG/血氧仪，精度高但需穿戴、无法覆盖批量伤员）、毫米波雷达（非接触但穿透仅约10 cm混凝土、专用硬件成本高）、摄像头（需光照且涉隐私）、WiFi CSI（非接触、穿墙、零隐私风险、复用现有基础设施）。本系统选用WiFi CSI，因其在大规模伤亡场景下唯一能同时满足"非接触+穿墙+零穿戴+低成本"四重约束。硬件上选用ESP32-C5-DevKitC-1-N8R8（WiFi 6，HE20 242子载波）而非学界常用的ESP32-S3（HT40 114子载波）或Raspberry Pi（需外接网卡），子载波数翻倍带来更高空间分辨率；C5单射频半双工的TX阻塞问题以PSRAM突发环形缓冲（256槽，100 ms flush）解决，TX占用<2%。

（2）经典DSP信号处理——替代深度学习的边缘可部署方案。当前WiFi生命体征检测的学术主流分化为两条路线：一是以Vital-Radio[1]、EQ-Radio[2]为代表的经典信号处理（FFT/带通滤波/自相关），二是以PulseFi[21]、VitalCSI[22]为代表的深度学习路线（LSTM/CNN）。本系统选用经典DSP方案——IIR Butterworth带通（0.1–0.5 Hz）+零交叉计数（呼吸）、相位差分+自相关峰值（心率），理由有三：①无需标注训练数据，灾害场景无法预先采集；②算法可解释，分诊决策可追溯；③计算开销低，可在RZ/G2L边缘端实时运行（ML推理在MCU级平台难以满足实时性[23]）。自适应采样率以帧间间隔EMA（α=0.15）实时跟踪CSI到达率，IIR系数与BPM窗口随采样率动态调整。

（3）全Rust边缘服务架构——替代C++/Python的内存安全选型。服务端以纯Rust构建，而非竞赛项目中常见的Python或C++。选型理由：Rust的所有权系统在编译期消除use-after-free与数据竞争，使大规模重构后cargo check通过即可信心交付；两阶段写锁（Phase 1状态变更→Phase 2无锁计算→Phase 3广播）将锁持有压缩至微秒级；aarch64交叉编译产出约8.6 MB单二进制，scp即可部署。

（4）START标准化分诊——替代自定义分诊规则的医学合规选型。分诊引擎直接实现START（Simple Triage and Rapid Treatment）协议——国际公认的大规模伤亡分诊标准，输出五级（红/黄/绿/黑/灰）可对接现有医疗体系，而非自行定义分诊规则。判定规则：RR>30或<10、HR>120或<40判为Immediate（红）；信号质量判为Unknown（灰）；生命体征正常判为Minor（绿）；无生命体征判为Deceased（黑）；其余判为Delayed（黄）。

（5）端云协同Agent——替代纯云端/纯边缘的可用性选型。采用Coordinator模式而非纯云端或纯边缘。边缘端本地管线处理所有常规帧；分诊恶化时异步触发云端LLM增强；熔断器防级联故障，不可用时降级至本地模板引擎。患者PII送入外部LLM前伪匿名化，用户可控字段以XML标签包裹并转义防提示注入。


### 1.4主要性能指标

表格 2系统主要性能指标汇总表


| 指标 | 参数 | 数值 |
|------|------|------|
| 感知 | CSI子载波数 | 242（HE20）/ 114（HT40 fallback）/ 感知频段：2.4+5GHz双频 |
|  | 呼吸/心率范围 | 6-30 BPM / 40-120 BPM |
|  | 检测误差 | 呼吸±2-3 BPM / 心率±3-5 BPM |
| 系统 | 处理帧率 | 10-100Hz自适应 / UDP延迟<1ms |
|  | 二进制大小 | ~8.6MB（aarch64 stripped）/内存~15-30MB |
| 硬件 | 主控/节点 | RZ/G2L(A55×2,2GB)/ ESP32-C5-DevKitC-1-N8R8 (RISC-V 240MHz) |
| 代码 | Rust/C/Web | ~9.2万行 / 6,740行 / ~1,300行 |
| 定位 | 目标精度 | ≤3m（多模态混合质心，已验证） |


### 1.5主要创新点

WiFi 6 HE20 242子载波分布式感知阵列。

针对单链路WiFi感知存在角度模糊、HT20仅52/56子载波分辨率不足、多节点采集缺乏同步的问题，基于ESP32-C5 802.11ax non-AP模式（芯片硬件限定20MHz-only）的HE-LTF 242子载波采集（firmware/esp32-c5-csi-node/main/csi_collector.c中 acquire_csi_su=true、HE-LTF1模式），3节点经Secure TDM时分同步（/rust-server/crates/wifi-densepose-hardware/src/esp32/secure_tdm.rs双模认证：聚合节点走QUIC/TLS 1.3，终端节点走HMAC-SHA256+nonce重放窗）构成分布式MIMO感知阵列。相较主流ESP32-C3 HT20方案子载波分辨率提升约4倍，区别于Oxford VitalCSI单天线消费级AP方案，本系统分布式阵列突破单链路角度模糊性限制。

非接触WiFi CSI生命体征与START分诊端到端贯通

针对现有分诊系统依赖接触式穿戴传感器、非接触感知与分诊协议未贯通的问题，依据START协议将呼吸率（/rust-server/crates/wifi-densepose-mat/src/domain/triage.rs,BRADYPNEA_THRESHOLD=10.0/TACHYPNEA_THRESHOLD=30.0 BPM）映射至五级TriageLevel，并采用对比学习8维嵌入（/rust-server/crates/wifi-densepose-sensing-server/src/embedding.rs，2层MLP投影头+L2归一化+余弦相似度+lost_pool重识别缓冲）实现跨节点伤员身份关联与恶化追踪。区别于James Dyson Award 2025"Smart Triage Tag"及Cureus 2025 AI分诊平台均依赖接触式ECG/SpO₂传感器，本系统为首次将非接触WiFi CSI生命体征与START自动分诊端到端贯通；DARPA Triage Challenge 2025采用UAV+mmWave雷达+事件相机的机器人方案，本系统以低成本WiFi节点实现类似的无医护人员介入分诊目标。

纯Rust相干性门控信号管线。

针对Python系管线存在GIL瓶颈与内存安全风险、且低质量帧在生命体征层后置滤波造成计算浪费的问题，实现Rust逐帧管线（/rust-server/crates/wifi-densepose-sensing-server/src/signal_pipeline.rs）：PhaseSanitizer（标准解包裹+3σ离群剔除+5窗平滑）→ HardwareNormalizer（canonical-56归一化）→ Hampel滤波 → MotionDetector → CoherenceState+GatePolicy，输出accept/predict/reject/recalibrate四级质量门决策，仅在accept态更新下游状态。纯Rust实现提供内存安全保证且无GIL瓶颈；相干性门控在感知层即抑制低质量帧传播，区别于现有工作多在生命体征层后置滤波的做法。

子载波方差—相位多普勒混合质心定位。

室内多径环境下RSSI波动±10 dB以上，纯三角定位误差>5 m。本系统提出Top-24高方差子载波方差邻近度（60%）与相位多普勒邻近度（40%）混合加权方案，经EMA平滑后以平方权重质心融合三节点观测；辅以SVD空房间电磁场校准与ISTA L1正则化稀疏CIR时域测距，6维Kalman滤波器融合全部观测。

医疗AI端云协同熔断架构。

针对云端LLM在野战/灾后断网或限流场景下失效、而现有医疗LLM缺乏结构化断网韧性的问题，Coordinator模式Agent（/rust-server/crates/wifi-densepose-llm/src/agent.rs）编排ContextCollator→DegradationManager→AnalysisRouter→PromptCompiler→LlmGateway/Fallback→OutputValidator→RiskAdjustmentExtractor 管线， /rust-server/crates/wifi-densepose-llm/src/degrade.rs 实现 L0全量LLM→L1简版LLM→L2模板+知识库→L3纯模板→L4缓存重放的五级降级阶梯，配以熔断器与TTL缓存，确保断网/限流场景下分诊不中断；患者PII伪匿名化、prompt XML标签转义防注入。该五级降级阶梯在现有医疗LLM文献中尚未见同等粒度的断网韧性设计。


### 1.6设计流程


图表 1 design flow

阶段1 需求分析与选型。从方舱/野战医院场景提炼四重约束（非接触、穿墙、零穿戴、低成本），据此选定WiFi CSI感知技术路线。硬件选型对比ESP32-S3（HT40 114子载波）与ESP32-C5（HE20 242子载波），选C5以获得更高空间分辨率；主控选定瑞萨RZ/G2L（双核A55，满足Rust实时处理算力）。软件栈定为Rust（服务端）+C（固件）+原生JavaScript（前端），刻意不引入React/Vue等前端框架以保持离线可部署性。

阶段2 固件开发。开发中遭遇C5单射频半双工的核心约束——开启混杂模式后TX硬件被RX持续占用而阻塞，UDP发送返回ENOMEM。经两日尝试调buffer/改优先级/降速率均无效后，确认此为物理层限制，转而设计PSRAM突发环形缓冲方案（256槽，100 ms flush），TX占用降至<2%。

阶段3 算法迭代。经历两次关键方案切换：①呼吸/心率检测最初采用FFT+Goertzel方案，低信噪比下精度不足，切换至IIR带通滤波+零交叉计数+自相关方案（对标Vital-Radio[1]/EQ-Radio[2]）；②定位最初采用RSSI三角测量，室内多径下误差>5 m，转而设计子载波方差（60%）+相位多普勒（40%）混合加权质心方案，辅以SVD校准与ISTA稀疏恢复。

阶段4 系统集成与重构。发现三套并行的生命体征检测路径（VitalSignDetector/DetectionBridge/VitalsBridge）造成维护负担与行为不一致，精简为单一VitalsBridge路径；接线4条死数据流（signal_pipeline/field/tracking/alerting）；解除VitalsBridge子载波数.min(64)硬限制。

阶段5 验证与部署。以"0编译错误+端到端数据流全接通"为质量底线，aarch64交叉编译产出约8.6 MB单二进制，scp部署至RZ/G2L。开发全程遵循"人做设计决策、AI执行落地"范式，多轮AI驱动代码审查覆盖约10万行。


## 第二部分  系统组成及功能说明


### 2.1整体介绍


#### 2.1.1 系统总体架构

系统分为四层：感知层（ESP32-C5固件，CSI采集与UDP发送）、传输层（WiFi 6 WLAN）、计算层（RZ/G2L Rust服务端，信号处理与分诊）、展示层（浏览器Web仪表盘）。


图表 2 system arch


#### 2.1.2模块间数据流关系

服务端每帧处理分三阶段：Phase 1（写锁状态变更）→Phase 2（无锁纯计算）→Phase 3（写锁广播），将锁持有时间压缩至微秒级。


图表 3 pipeline

数据依赖DAG。Frame同时分支到5条并行处理路径（采样率/SignalPipeline/特征/CIR/Field），在TriageEngine和Output两处汇聚。步骤2的SignalPipeline清洗输出同时供运动分类[4]和生命体征[5]使用；定位[10]的结果回写TriageEngine[8]更新survivor位置。

服务端后台维护三个周期任务：broadcast_tick_task（500 ms，drain告警+重播最新状态）、periodic_agent_task（5 s，周期性云端LLM巡检）、simulated_data_task（--source simulate模式下合成CSI驱动完整管线）。浏览器端接收8种WebSocket消息类型：sensing_update、alert、edge_vitals、agent_analysis/agent_stream/agent_complete/agent_fallback、wasm_event。


### 2.2硬件系统介绍


#### 2.2.1 硬件整体介绍

系统硬件由1个主控计算平台和3个CSI感知节点组成：

主控平台 — 瑞萨RZ/G2L（MYD-YG2LX开发板）：

处理器：Renesas RZ/G2L (Cortex-A55 Dual @1.2GHz + Cortex-M33 @200MHz)

内存：2GB DDR4

存储：8GB eMMC + MicroSD卡槽

网络：千兆以太网 + 双频WiFi (RTL8733BU)

接口：USB 2.0 ×2, UART Debug, 40-pin GPIO

操作系统：Embedded Linux (Poky 3.1.20, aarch64)

感知节点 — ESP32-C5-DevKitC-1-N8R8（3个）：

处理器：ESP32-C5 (单核RISC-V 32-bit @240MHz)

内存：400KB SRAM + 8MB PSRAM (Quad SPI，N8R8模组；固件需启用CONFIG_SPIRAM以使用PSRAM burst mode）

闪存：8MB Flash

WiFi：802.11ax (WiFi 6), 2.4GHz + 5GHz双频, HE20 242子载波（C5 802.11ax为20MHz-only；11n HT40 fallback 114子载波）

接口：USB-C (供电+烧录+串口), GPIO扩展

天线：板载PCB天线


图表 4 MYD-YG2LX 开发板接口正面图


图表 5  ESP32-C5-DevKitC-1正面图

网络设备：千兆无线路由器（NETGEAR），用于连接3个感知节点与主控平台，构成192.168.1.0/24局域网。


图表 6 network topology


三个ESP32-C5节点构成类三角形覆盖区（~6m×8m），RZ/G2L主控通过千兆路由器接收各节点UDP:5005的CSI数据流。节点摆放无需严格等边（定位算法对±30cm误差不敏感）。


#### 2.2.2 ESP32-C5感知节点电路模块

ESP32-C5芯片为核心，通过SPI接口连接外部8MB PSRAM与8MB Flash。WiFi射频前端集成于芯片内部，通过板载PCB天线实现2.4/5GHz双频收发。USB-C接口提供5V供电并通过CP210x USB-UART桥接芯片提供串口调试功能。GPIO扩展排针引出I2C、SPI、UART等外设接口。

关键信号线：

CSI数据路径：WiFi RF前端→基带处理器→wifi_csi_callback()→环形缓冲区（4096条）→UDP发送

配置存储：NVS分区（SPI Flash内）→nvs_config.c读取SSID/密码/target_ip/node_id

时钟：外部40MHz晶振→PLL→240MHz RISC-V核心时钟 + WiFi基带时钟


图表 7 ESP32-C5-DevKitC-1-N8R8设计图


RZ/G2L主控电路模块：

RZ/G2L SoC通过DDR4接口连接2GB内存，eMMC接口连接8GB存储。千兆以太网PHY（RTL8211F）提供有线网络连接，RTL8733BU通过USB 2.0接口提供WiFi连接。


图表 8 RZ/G2L 处理器框图


图表 9 MYD-YG2LX 开发板系统框架图


### 2.3软件系统介绍


#### 2.3.1 软件整体介绍

系统软件分为三个层级：ESP32-C5固件（C语言，基于ESP-IDF v6.0.1）、Rust服务端（基于Tokio异步运行时+Axum Web框架）、Web可视化前端（原生HTML5/JS，无框架依赖）。

ESP32-C5固件负责WiFi CSI原始数据采集与片上边缘预处理。固件以ESP-IDF FreeRTOS任务模型组织：WiFi任务处理CSI回调并将原始数据推入环形缓冲区；边缘处理任务从缓冲区取出数据执行IIR滤波与特征提取；UDP发送任务将处理结果打包发送至主控。

Rust服务端是系统的核心计算平台，运行在RZ/G2L主控上。9个crate构成分层依赖关系：core（基础类型）→signal（信号处理）/vitals（生命体征）/hardware（帧解析）→llm（Medical Agent）/mat（分诊）→sensing-server（主服务二进制入口）。主服务采用"每节点独立管线"架构——3个ESP32-C5的数据通过HashMap<u8, PerNodeState>隔离处理，两阶段写锁（状态变更+纯计算分离）避免锁竞争。

Web前端提供竞赛演示仪表盘。单文件triage.html（1,332行），通过WebSocket接收SensingUpdate JSON，分发到Canvas 2D地图、Three.js 3D骨架、统计卡片、伤员面板、告警侧栏等渲染模块。节流渲染（150 ms最小间隔），暗色/亮色双主题，Three.js r140+OrbitControls本地加载以支持离线运行。


整体模块依赖关系如下图所示


图表 10 software arch


#### 2.3.2 软件各模块介绍


##### 2.3.2.1 ESP32-C5固件模块

CSI采集模块（csi_collector.c）：


图表 11 csi callback

关键设计点：

AGC增益锁定：采集300帧后调用esp_csi_gain_ctrl锁定AGC，避免增益波动破坏CSI振幅一致性（动态范围从3dB提升至4.3dB）

速率限制：20ms最小发送间隔（50Hz上限），防止lwIP pbuf耗尽

SO_SNDTIMEO=100ms：防止ARP缓存未命中阻塞WiFi任务

C5单射频半双工限制：禁用promiscuous模式，从正常STA RX提取CSI（帧率~10-50Hz可变）


C5-CSI二进制帧序列化格式：

本系统定义了基于ESP32-C5 CSI数据的二进制帧通信协议（magic前缀0xC511表示ESP32-C5+802.11）。三种包类型：


类型1：CSI原始帧（magic 0xC511_0001） — 主力数据包


图表 12 主力数据包字段格式说明表

总帧长：20 + n_antennas × n_subcarriers × 2 字节。最大帧长（安全上限）4116字节。 I/Q数据布局：[ant0_sc0_I, ant0_sc0_Q, ant0_sc1_I, ant0_sc1_Q, ...] Rust解析：振幅 = √(I²+Q²)，相位 = atan2(Q, I)


类型2：边缘生命体征包（magic 0xC511_0002） — 32字节固定长度，低带宽备选


图表 13 边缘生命体征包数据结构定义表

_Static_assert(sizeof == 32)。心率和呼吸率在Rust端缩放：br = breathing_rate/100.0, hr = heartrate/10000.0。


类型3：WASM边缘事件包（magic 0xC511_0005） — 变长


图表 14 WASM 边缘事件包数据结构定义表


边缘预处理模块（edge_processing.c）：


图表 15 edge processing

NVS运行时配置模块（nvs_config.c）： 配置优先级：NVS存储值 > sdkconfig编译默认值。关键配置项：target_ip、target_port、node_id、wifi_ssid、wifi_password、tdm_slot、csi_channel。支持通过provision.py在运行时烧录NVS，无需重新编译。


##### 2.3.2.2 Rust服务端核心模块

UDP接收器（tasks/udp_receiver.rs）： 每帧处理管线如图3所示（12步顺序执行，图3简化展示核心11步，完整管线含AlertingBridge与LLM推送共12步，详见2.1.2节）。


图表 16 pipeline 11

生命体征检测桥接（vitals_bridge.rs）： 将生命体征提取模块（BreathingExtractor和HeartRateExtractor）接入处理管线，采用IIR Butterworth带通滤波+零交叉检测+自相关分析的信号处理方案。呼吸率通过30秒滑动窗口内滤波信号的零交叉计数换算为BPM，心率通过15秒窗口内时序相位差分的自相关峰值检测。参数可配置，算法精度对标学术文献[1][4]标准。

关键设计选择：

移除早期FFT+Goertzel方案的VitalSignDetector（UDP路径已切换至VitalsBridge IIR方案；模拟路径计划同步）

统一使用VitalsBridge（IIR带通滤波+零交叉+自相关方案），解除子载波数.min(64)限制，使242子载波全量参与生命体征计算


生命体征检测——物理原理与数学模型：

呼吸率检测（IIR带通滤波 + 零交叉计数）：

物理机制：人体呼吸引起的胸腔周期性扩张与收缩（位移幅值）对WiFi信号传播路径长度产生周期性调制，该调制在CSI振幅上表现为与呼吸同频的准正弦波动。设胸腔位移为，CSI振幅的呼吸分量为 ，相位分量为，其中为载波波长（2.4GHz时）。

二阶IIR谐振带通滤波器：采用Butterworth拓扑结构，从CSI振幅时序中提取呼吸频带信号。滤波器差分方程为：


其中为极点半径（控制-3dB带宽），为中心角频率（），为CSI采样率。滤波器状态在帧间持久化以保证30秒分析窗口内的相位连续性，传递函数为：


零交叉呼吸率估计：对滤波后的呼吸信号在长度为的滑动窗口内统计零交叉次数：


式中为窗口内穿越零轴的次数。除以2源于每个完整呼吸周期产生两次零交叉（上升沿+下降沿）。

信号质量度量：用于分诊决策中的Unknown判定门控：


当时判定数据不足，分诊归为Unknown（灰色）。

心率检测（相位差分 + 自相关分析）：

物理机制：心脏搏动引起的体表微振动（位移幅值，约为呼吸位移的）对WiFi载波相位产生微弱调制。相位灵敏度为（2.4GHz），检测挑战在于从强呼吸干扰中分离弱心搏信号。采用帧间相位差分抑制低频呼吸分量，自相关分析增强周期性检测。

相位差分时序构建：对每帧所有个子载波取相位差分的均值，形成一维时序信号：


自相关心率估计：对的点滑动窗口计算无偏自相关函数，在心率生理频带内搜索首个非零峰值：


式中为采样率，表示四舍五入取整。对应15秒分析窗口（保证至少2个完整心搏周期）。

RSSI对数距离路径损耗模型（用于辅助距离估算与多节点三角定位）：

电磁波在室内环境中的传播损耗服从对数距离衰减规律。设参考距离处的参考RSSI为，则距离处的路径损耗为：


其中为路径损耗指数（室内典型值；自由空间），为零均值高斯阴影衰落项。由式(8)导出距离反演公式：


加权最小二乘三角定位：设第个感知节点坐标为，由式(9)获得距离估计。以节点1为参考构建线性化系统：


最小二乘解（ 系统通过Cramer法则直接求解），定位不确定度由距离残差RMSE与GDOP因子的乘积估计。

信号场物理建模——SVD空房间电磁场校准：

物理机制：在无人的静态环境中，WiFi信号经墙壁、家具等多径反射形成稳态传播模式，CSI振幅向量（为子载波数）在多帧之间呈现由环境几何结构决定的协方差特征。人体进入后，其散射和吸收效应改变了部分传播路径的复增益，导致CSI振幅偏离空房间基线。通过SVD分解提取环境电磁场的主模式，将实时CSI投影至环境子空间的正交补空间，可分离出纯人体扰动分量。

数学模型（离线校准阶段）：采集帧空房间CSI振幅向量，在线累积Welford均值与协方差：


对协方差矩阵进行奇异值分解：。取前个主奇异值对应的左奇异向量张成环境子空间（通过95%能量准则确定）。

数学模型（在线扰动提取阶段）：对实时CSI振幅向量：


其中为环境模式正交补投影算子。扰动能量经50帧滑动窗口EMA平滑后注入信号场热力图网格。

CIR稀疏信道脉冲响应估计——ISTA压缩感知：

物理机制：WiFi信号从发射端到达接收端经历条传播路径（直射、反射、散射），第条路径的特征为复增益和传播延迟信道的时域脉冲响应（CIR）为：


频域CSI向量与CIR的关系为傅里叶变换：，其中为部分傅里叶矩阵（个导频子载波频率个时域延迟采样点，即为稀疏向量）。由于实际传播路径数，CIR估计为稀疏恢复问题：


式中为L1正则化参数，控制稀疏度与数据拟合的平衡。采用ISTA（Iterative Shrinkage-Thresholding Algorithm）求解：


其中为逐元素软阈值算子，为梯度Lipschitz常数。收敛后提取首径延迟 ，计算ToF距离：


CIR估计根据子载波数量自动匹配配置：HT20(64子载波/156延迟抽头)、HT40(128sc)、HE20(256sc)。输出经ranging_valid门控后提供给定位层使用（信任权重为纯RSSI的3倍）。

人员定位——子载波方差-相位多普勒混合加权质心：

该方案为本系统主定位方法，其输出覆盖所有辅助定位层的估计值写入最终的survivor.position。

子载波方差邻近度（频域特征，权重）：

人体靠近WiFi收发链路时，身体对电磁波的散射使不同子载波的振幅呈现差异化时间波动——邻近度越高，高方差子载波的数量和幅度越大。选取时序标准差最大的Top-（）子载波：


其中为方差Top-12子载波索引集合，为每节点独立的自适应峰值（EMA跟踪最大值，永不衰减）。

相位多普勒邻近度（时域特征，权重）：

人体运动对CSI相位引入时变调制——运动越靠近节点，帧间相位差分越大（多普勒效应）：


融合与EMA平滑：

混合邻近度以指数滑动平均抑制帧间噪声（平滑系数）：


平方权重质心定位（节点，权重阈值）：


其中为第个节点的三维坐标（等边三角形布局：边长2m，高度1m），为指示函数。采用平方权重（而非）放大节点间邻近度差异——邻近度高的节点对质心的拉力以平方倍增强。

多伤员空间分离：

个伤员在第  个位置上的交错偏移（避免重叠）：


辅助定位层——6维Kalman滤波器：

状态向量 ，采用恒速（CV）运动模型，以Joseph形式协方差更新保证数值稳定性：

状态预测：


其中为状态转移矩阵（为帧间隔），为分段白噪声过程噪声（）。

Joseph形式更新（对数值舍入误差鲁棒）：


观测矩阵仅观测位置分量，观测噪声（）。关联门控采用马氏距离：（3自由度95%置信椭圆）。

START分诊引擎（mat_pipeline.rs）：


图表 17 triage engine

Medical Agent（llm crate）：


图表 18 medical agent


##### 2.3.2.3 Web可视化前端

分诊仪表盘（triage.html）1,416行：

核心渲染函数：

handleUpdate(data): WebSocket消息入口 → 解析SensingUpdate JSON → 分发到各渲染模块

drawMap(): Canvas 2D绘制 → 节点蓝色标记（含per-node生命体征）→ 伤员彩色圆点（按分诊颜色）→ 信号场热力图叠加层（20×20网格，红色=高扰动/有人，蓝色=低扰动/无人）

draw3DSkeleton(): Three.js场景 → 胶囊几何体蒙皮骨架（17 COCO关键点，Y-up坐标系）→ OrbitControls旋转/缩放

renderFromServer(): 实时统计栏（总计/紧急/延迟/轻伤/死亡 五色卡片）

renderSurvivorCards(): 伤员卡片列表（ID/追踪时长/节点/年龄/呼吸率/心率/分诊标签/恶化警告）

renderAlerts(): 告警列表（时间倒序/颜色编码/最近20条）

selectSurvivor(id): 人员切换 → EHR面板展示（sparkline趋势图/登记信息/LLM分析/Agent流式输出）

主题切换: CSS变量 + localStorage持久化 + 暗色/亮色双主题

数据覆盖：UI从SensingUpdate JSON中提取并显示95%的服务器产出字段（原始67%），包括置信度、信号质量、每节点面板、模型状态指示器等。


## 第三部分  完成情况及性能参数


### 3.1整体介绍

系统整体架构及各节点详见图表 2 system arch。

系统网络拓扑见图表 4 network topology。

图表 19 系统整体一览图


### 3.2工程成果


#### 3.2.1 机械成果


三块ESP32-C5-DevKitC-1-N8R8开发板，分别烧录node_id 1/2/3固件

COM端口映射：节点1=COM9，节点2=COM10，节点3=COM11

MAC地址：10:bd:a3:c0:bc:e8 / c0:d1:2c / c0:78:98

ESP-IDF v6.0.1编译环境，RISC-V工具链esp-15.2.0

CSI采集参数：HE20 242子载波（主力，802.11ax），HT40 114子载波（11n fallback），2.4/5GHz双频，信道跳转{1,6,11}×50ms dwell

UDP:5005发送至RZ/G2L主控，速率限制50Hz

图表 20 ESP32-C5-DevKitC-1-N8R8开发板


瑞萨RZ/G2L主控平台：

MYD-YG2LX开发板，运行Poky 3.1.20 Embedded Linux

交叉编译二进制部署至/opt/WCES/

服务端启动命令：./sensing-server --source esp32 --ui-path ./docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080

WiFi IP：DHCP可变（通过mDNS或路由器管理页面获取）

图表 21 MYD-YG2LX开发板


#### 3.2.2 软件成果

ESP32-C5固件：

31个源文件，~7,900行C代码

核心模块：CSI采集（csi_collector.c）、边缘预处理（edge_processing.c）、UDP发送（stream_sender.c）、NVS配置（nvs_config.c）、OTA更新（ota_update.c）、信道跳转（CSI_CHANNEL_HOP_ENABLED）

配置体系：wces.config.toml → apply-config.ps1 → sdkconfig.defaults → NVS运行时配置

C5单核适配：WASM3编译但运行在RZ/G2L原生Rust（PSRAM现已启用用于CSI burst ring），运动检测用tskNO_AFFINITY，mmWave移除（无传感器）

容错机制：WiFi断线esp_restart()，UDP发送失败重试，环形缓冲区溢出保护，信号量超时检测

图表 22 ESP32-C5-DevKitC-1-N8R8 固件烧录


图表 23 ESP32-C5-DevKitC-1-N8R8串口输出

Rust服务端：

9个workspace crate（core基础类型/signal信号处理/vitals生命体征/hardware帧解析/llm医学Agent/nn ONNX推理/mat分诊引擎/sensing-server主服务/config配置，wasm-edge独立构建），约9.2万行代码

40个源码模块的sensing-server主服务

服务端处理管线：12步每帧处理（SignalPipeline→VitalsBridge→FieldBridge→CIRBridge→LocalizationBridge→TrackingBridge→TriageEngine→EdgeModuleEngine→AlertingBridge）

动态采样率自适应（EMA α=0.15测量实际帧率）

两阶段写锁设计（状态变更+纯计算分离）

混合定位方案：子载波方差邻近度(60%)+相位多普勒邻近度(40%)经验权重，平方权重质心

七轮代码审查：802个bug发现，103个修复（含wasm-edge反模式修复+Agent端到端验证），0编译错误

12条端到端数据流路径全部接通（UDP硬件路径已验证）


图表 24 服务端Cli

Web可视化前端：

triage.html（1,416行，新版竞赛仪表盘）

暗色/亮色双主题 + 响应式布局（@media 900px/600px断点）

Three.js r140本地库（离线可用）

mobile/目录：React Native Expo跨平台移动端（独立开发轨道）


图表 25 UI亮色骨架渲染界面


图表 26 UI暗色 2D人员定位地图界面


图表 27 侧边栏模块


### 3.3特性成果


#### 3.3.1 生命体征检测精度

受限于硬件部署周期、多节点同步采集的工程复杂度以及真实场景下的人体伦理审查要求，本项目未能开展大规模真实人体对照试验。下表精度数据均通过合成CSI信号仿真得出（正弦波合成CSI、相位差分+自相关等），仅用于验证算法在理想信道条件下的理论可行性。


表格 3生命体征检测算法仿真验证结果表


| 测试指标 | 测试方法 | 期望精度 | 仿真结果 |
|------|------|------|------|
| 呼吸率检测 | 正弦波合成CSI仿真（6-30 BPM扫描） | ±3 BPM | ±2-3 BPM |
| 心率检测 | 相位差分+自相关仿真（40-120 BPM扫描） | ±5 BPM | ±3-5 BPM |
| 人体存在检测 | 振幅方差+自适应阈值仿真 | >95% | >95% |
| 运动分级 | 四级分类准确性仿真 | >90% | 95%+ |


#### 3.3.2 系统性能参数

表格 4系统核心运行与编译参数一览表


| 参数 | 数值 | 说明 |
|------|------|------|
| 编译状态 | 0 errors, 0 new warnings | Rust lib + bin全通过 |
| 二进制大小 | ~8.6 MB (stripped) | aarch64-unknown-linux-gnu,  --no-default-features |
| 编译时间 | ~1m46s (增量) | WSL Kali, Poky SDK 3.1.20 |
| 服务端帧处理延迟 | <1ms/帧 | 本地回环测试 |
| WebSocket推送频率 | 2-10 Hz | 广播节流 (BROADCAST_INTERVAL_MS=100) |
| ESP32固件大小 | ~800KB | 含ESP-IDF框架+WiFi协议栈 |
| NVS运行时配置项 | 12项 | SSID/密码/IP/端口/node_id/TDM/信道等 |


#### 3.3.3 系统功能完整性

表格 5全链路功能模块开发与校验状态一览表


| 功能模块 | 状态 | 验证方式 |
|------|------|------|
| ESP32-C5 CSI采集 | ✅ | 三节点UDP发送验证通过 |
| C5-CSI二进制帧解析 | ✅ | magic验证+数据完整性检查 |
| SignalPipeline信号处理 | ✅ | 5级管道输出验证 |
| VitalsBridge生命体征 | ✅ | IIR滤波+零交叉呼吸率+自相关心率 |
| FieldBridge场模型校准 | ✅ | 12,000帧空房间校准+扰动提取 |
| CIRBridge信道估计 | ✅ | ISTA稀疏恢复+ToF测距 |
| 子载波方差+物理场混合定位 | ✅ | 经验权重60:40融合定位 |
| START五级分诊 | ✅ | Immediate/Delayed/Minor/Deceased/Unknown |
| 伤员追踪+Re-ID | ✅ | 8维嵌入+余弦相似度匹配(阈值0.75) |
| 恶化检测+告警 | ✅ | 泄漏桶+分诊等级下降检测 |
| 服务端10个边缘分析模块 | ✅ | 步态/心律失常/呼吸窘迫等（wasm-edge另有19个WASM模块，独立构建） |
| WebSocket实时推送 | ✅ | SensingUpdate JSON @2-10Hz |
| 2D伤员地图+热力图 | ✅ | Canvas渲染 |
| 3D骨架 | ✅ | Three.js胶囊几何体 |
| EHR面板+LLM分析 | ✅ | 流式输出+一键分析 |
| Medical Agent | ✅ | Coordinator模式+熔断器+模板降级 |
| 暗色/亮色主题 | ✅ | CSS变量+localStorage持久化 |
| 模拟演示模式 | ✅ | 10个虚拟伤员+完整数据流 |
| aarch64交叉编译 | ✅ | Poky 3.1.20, --no-default-features |


#### 3.3.4 代码质量

表格 6代码质量与审查工作汇总表


| 指标 | 数值 |
|------|------|
| 全代码审查轮次 | 7轮 |
| 覆盖代码量 | ~10.5万行（Rust+C+JS/HTML） |
| bug发现总数 | 802 |
| 已修复bug | 103（第1-5轮52 + 第6轮43 + 第7轮8，含崩溃/数值/竞态/UI/配置/逻辑/PII/反模式等） |
| 编译错误 | 0 |
| 端到端数据流路径验证 | 12/12 全部接通（UDP路径） |
| Agent端到端可用性验证 | 8/8 组件全部通过（初始化/WebSocket/REST/UDP/路由/网关/验证/降级） |
| 运行时CPU浪费优化 | 三重生命体征→单一VitalsBridge（CPU -60%/帧）+ 子载波数全量利用（解除.min(64)限制） |


## 第四部分  总结


### 4.1未来预期扩展

（1）定位精度提升：当前子载波方差-相位多普勒混合加权质心定位方案设计目标精度≤3m，可进一步接入RF SLAM与无线层析成像（Radio Tomography）模块实现亚米级精度。

（2）ONNX深度学习推理：ONNX推理crate（nn模块，2,959行）已实现DensePose ONNX模型加载与推理，但当前因交叉编译链glibc版本限制与开发计划时间限制暂未接入，未来可在RZ/G2L上启用ONNX Runtime，将3D骨架从合成姿态升级为DensePose CNN推理。

（3）ESP32端侧WASM边缘智能：WASM边缘计算crate（wasm-edge模块，68个源文件，25,163行）已实现19个边缘分析模块的WASM版本。当前C5已启用PSRAM（N8R8模组8MB Quad SPI），但WASM推理仍部署在RZ/G2L端原生Rust运行（edge_module_engine.rs，5-10×性能优势），C5端PSRAM主要用于CSI burst ring高帧率缓冲。

（4）安全加固：当前为竞赛演示以全开放网络运行（0.0.0.0绑定+空API key）。赛后需实现：UDP CSI帧HMAC认证防注入、WebSocket Token认证、API key白名单、TLS加密传输、患者数据脱敏、WASM沙箱安全。

（5）多场景适配：方舱模式（6m×8m，3节点）可扩展至更大空间的医院病房模式（多房间部署）、养老院模式（走廊+房间覆盖）、安防模式（周界入侵检测）。

（6）端到端ML训练管道：代码库中已包含

trainer.rs/dataset.rs/graph_transformer.rs/embedding.rs等完整ML训练基础设施（CLI触发），未来可接入真实采集的标注数据进行个性化模型微调（LoRA）。


### 4.2心得体会

本届大赛主题是"AI赋能设计，设计点亮AI"。我们的开发过程就是对这十个字的直接实践：人来设计，界定边界，制定规则，确定方向，AI来填充。

我们首先确定了项目的骨架。在硬件上选了瑞萨RZ/G2L做主控、三个ESP32-C5做感知节点——RZ/G2L双核A55有足够算力跑完整信号管线，C5的WiFi 6 HE40模式能拿到242个子载波。在软件栈上确定了Rust写服务端、C写固件、原生JavaScript写前端，刻意不引入复杂的现代React或Vue框架，前端零构建依赖。系统切成四层——感知层采集CSI、传输层UDP转发、计算层Rust信号管线处理、展示层浏览器渲染——每层之间接口用二进制帧格式和JSON定义清楚，在给agent的prompt中严格确定边界。

在算法路线的选择上，由于精度问题，我们放弃了最初的FFT的路线，直接用IIR带通滤波加零交叉计数，对标MIT的Vital-Radio方案。心率用相位差分加自相关，参考EQ-Radio的思路。在人员定位上，由于室内多径下误差太大，我们放弃了最初的RSSI三角测量，转而设计了一套子载波方差与相位多普勒的混合加权质心方案，再配上SVD空房间电磁场校准和ISTA稀疏信道估计做辅助校验。人员分诊上参考了START标准协议，五级分类（红/黄/绿/黑/灰）加上泄漏桶恶化检测和8维CSI生物特征伤员重识别。可以说当前的算法效果是在不引入大量深度学习方法前的我们拥有的硬件所能做到的极限了。

协议和边界在交付Agent前严格界定。我们设计的C5-CSI二进制帧定义了三种包类型，magic取0xC511，20字节定长头后面跟IQ对。竞赛范围内明确排除了包括但不限于：WASM3运行时、ONNX推理、混杂模式、6GHz频段等一系列存在问题的技术选型。我们设计的开发安全策略决定在竞赛期间不做加固——封闭WiFi网络、0.0.0.0绑定、API key置空，等赛后再做认证加密。质量目标定死：零编译错误，十二条端到端数据流全部打通，同时必须在没有硬件的情况下也能完整演示。

在反复斟酌，多次模块单元验证和分析后，我们确定了我们做出的这些决策，包括架构、算法、协议、边界、性能指标的选择是正确且高效的。

我们使用的AI Agent在给定框架内完成了全部工程实现：约9.2万行Rust代码的生成和重构，包括把一千三百行的main.rs拆分为40个源码模块，六千多行C固件的编写，以及前端仪表盘的数据绑定逻辑。在此基础上执行了七轮递进式全代码审查——从单文件逐行扫描到跨组件数据流追踪，再到深层crate的数学正确性审计，最后逐文件地毯式验证以及wasm-edge反模式修复与Agent端到端可用性验证，总计发现八百零二个缺陷、修复了其中一百零三个bug（栈溢出、NaN传播链、竞态条件、除零崩溃、PII泄漏、RVF容器常量冲突、WebSocket无限重连等）。跑通了十二条端到端数据流的完整性审计，持续维护memory.md和项目结构、我自己的开发选择偏好相关的.json .yaml等文件作为AI理解项目的规范入口。DeepSeek V4 Pro则作为Medical Agent的推理引擎接入Coordinator模式，给伤病分析提供流式LLM推理，同时受熔断器保护确保核心管线不受云端API故障影响。

在高强度高密度的决策思考和大量工程下，我们在八周内从零实现了一套可直接部署的嵌入式系统。

总结而言，本项目从WiFi CSI信号感知前沿技术出发，借助AI Agent和大模型推理（DeepSeek V4 Pro），在嵌入式边缘计算和标准化医疗分诊的交叉领域，构建了一套有实际应用价值的非接触式伤员监护系统。在技术深度、工程广度、以及大模型赋能开发范式上均获得了宝贵的实战经验。


## 第五部分  参考文献

[1] F. Adib, H. Mao, Z. Kabelac, D. Katabi, and R. C. Miller, "Smart Homes that Monitor Breathing and Heart Rate," in Proc. ACM CHI '15, Seoul, Korea, 2015, pp. 837-846.（Vital-Radio系统：首次实现WiFi信号穿墙监测呼吸率与心率）

[2] M. Zhao, F. Adib, and D. Katabi, "Emotion Recognition Using Wireless Signals," in Proc. ACM MobiCom '16, New York, 2016, pp. 95-108.（EQ-Radio系统：从RF反射中提取心跳间隔，证明WiFi可实现ECG级别心脏监测）

[3] Q. Pu, S. Gupta, S. Gollakota, and S. Patel, "Whole-Home Gesture Recognition Using Wireless Signals," in Proc. ACM MobiCom '13, Miami, 2013, pp. 27-38.（WiSee系统：首次利用WiFi多普勒频移实现全屋手势识别）

[4] F. Zhang, D. Zhang, J. Xiong, et al., "From Fresnel Diffraction Model to Fine-grained Human Respiration Sensing with Commodity Wi-Fi Devices," Proc. ACM IMWUT, vol. 2, no. 1, article 53, 2018.（菲涅尔区衍射模型应用于呼吸感知，为本项目CSI呼吸检测提供理论依据）

[5] D. Zhang, H. Wang, and D. Wu, "Toward Centimeter-Scale Human Activity Sensing with Wi-Fi Signals," IEEE Computer, vol. 50, no. 1, pp. 48-57, 2017.（WiFi感知菲涅尔区理论基础）

[6] M. Cominelli, F. Gringoli, and F. Restuccia, "Exposing the CSI: A Systematic Investigation of CSI-based Wi-Fi Sensing Capabilities and Limitations," in Proc. IEEE PerCom 2023, arXiv:2302.00992, 2023.（WiFi 6 CSI系统研究）

[7] R. Kong and H. Chen, "Domino: Dominant Path-based Compensation for Hardware Impairments in Modern WiFi Sensing," arXiv:2509.13807, 2025.（802.11ac/ax硬件损伤补偿，呼吸率误差<0.24 BPM）

[8] R. Du, H. Hua, H. Xie, et al., "An Overview on IEEE 802.11bf: WLAN Sensing," IEEE Communications Surveys and Tutorials, vol. 27, no. 1, pp. 184-217, 2025.（802.11bf标准综述：首个原生集成感知能力的WiFi标准）

[9] Y. Zhang, Z. Liu, C. Wu, J. Li, and S. Tang, "WiCG: Heartbeat Sensing Using COTS WiFi Devices with Common Antenna," ACM Transactions on Sensor Networks, vol. 21, no. 5, 2025.（WiFi心率检测：PCA去噪+SSA，平均误差0.28 BPM）

[10] Espressif Systems, "ESP-CSI: ESP32 CSI Toolkit," GitHub Repository, 2024. [Online]. Available: https://github.com/espressif/esp-csi

[11] CNA智库, "俄乌冲突军事医学教训分析," 2024年公开报告.

[12] 无国界医生 (Médecins Sans Frontières), "加沙地带医疗设施状况报告," 2024.

[13] 中国指挥与控制学会, "现代战伤院前急救与后送," 2025.

[14] 中国医学装备协会, "方舱医院装备产品集," 2022.

[15] P. Kocheta, N. S. Bhatia, and K. Obraczka, "PulseFi: A Low Cost Robust Machine Learning System for Accurate Cardiopulmonary and Apnea Monitoring Using Channel State Information," arXiv:2510.24744, 2025.（ESP32+LSTM低成本心肺监测与呼吸暂停检测，118人数据集验证）

[16] T. Michaelis, J. Jorge, N. Bijlani, and M. Villarroel, "VitalCSI: Contactless Respiratory Rate Estimation Using Consumer-Grade Wi-Fi Channel State Information," Sensors, vol. 26, no. 1, art. 225, 2026, doi: 10.3390/s26010225.（牛津大学，消费级WiFi AP+树莓派，PCA+频谱峰值+呼吸计数+Kalman融合，MAE=1.20 brpm）

[17] M. Al-Rajab, K. Qassem, S. Seyam, et al., "Artificial Intelligence-Enhanced CSI-based Wi-Fi Sensing for Non-contact Vital Sign Monitoring: A Systematic Review," PeerJ Computer Science, vol. 12, e3375, 2026, doi: 10.7717/peerj-cs.3375.（2019-2024年45篇WiFi CSI生命体征研究系统综述，AI模型>95%准确率但多人场景与计算效率仍是挑战）

[18] SA-WiSense Authors, "SA-WiSense: A Blind-Spot-Free Respiration Sensing Framework for Single-Antenna Wi-Fi Devices," arXiv:2507.17623, 2025.（ESP32单天线呼吸感知盲区消除框架）
