# WCES 项目从 CSI 采集到可视化的完整流程分析

> 本报告 100% 基于实际代码逐行分析，不参考任何外部文档。所有代码引用均带文件路径与行号。
> 生成日期：2026-07-18

---

## 📝 协议命名说明

本系统 ESP32-C5 固件与 Rust 服务端之间的二进制数据传输协议统称为 **WCES CSI 二进制帧协议 (WCES Binary Frame Protocol, WCES-BFP)**。该协议族通过 4 字节 Magic 标识符区分不同帧类型，命名规范如下：

| 协议名称 | Magic 标识 | 帧类型 | 用途 |
|---------|-----------|--------|------|
| WCES-BFP/CSI | `0xC511_0001` | CSI 主帧 | 原始 IQ 数据传输 |
| WCES-BFP/Vitals | `0xC511_0002` | 生命体征帧 | 边缘预处理后的呼吸/心率 |
| WCES-BFP/Compressed | `0xC511_0003` | 压缩帧 | Delta 压缩 IQ 数据 |
| WCES-BFP/WASM | `0xC511_0005` | WASM 事件帧 | 边缘模块告警事件 |

**Magic 字段编码规则**：`0xC511_00XX`，其中 `C5` 标识 ESP32-C5 芯片、`11` 标识 802.11 协议族、`00XX` 为帧类型编号。

---

## 📋 全局架构概览

整个系统的数据流向可类比为"急诊医院的运转流程"：

```
采集层(传感器)   传输层(快递员)   处理层(医生诊断)   可视化层(显示屏)
─────────────    ─────────────    ───────────────    ──────────────
ESP32-C5 ×3     UDP 二进制帧    Rust 异步服务端    浏览器 Canvas/WebGL
  WiFi 6 CSI  ──→ WCES-BFP协议 ──→  FFT/START分诊  ──→  伤员地图/3D骨架
  484子载波       Magic 0xC511   Tokio broadcast   WebSocket推送
```

---

## 🟢 第一层：CSI 数据采集（ESP32-C5 固件）

### 1.1 启动入口：main.c `app_main()`

ESP32-C5 上电后由 FreeRTOS 调用 `app_main()`（`firmware/esp32-c5-csi-node/main/main.c:244`），按以下顺序逐行执行：

**步骤 1：NVS 非易失存储初始化**（`main.c:247-252`）
```c
esp_err_t ret = nvs_flash_init();
if (ret == ESP_ERR_NVS_NO_FREE_PAGES || ret == ESP_ERR_NVS_NEW_VERSION_FOUND) {
    ESP_ERROR_CHECK(nvs_flash_erase());
    ret = nvs_flash_init();
}
ESP_ERROR_CHECK(ret);
```
> **通俗类比**：NVS 是 ESP32 的"小本本"，断电仍保留 WiFi 密码、目标 IP 等配置。本小本写满或版本升级时先擦除再重建。

**步骤 2：PSRAM 检测**（`main.c:263-276`）— 通过 `heap_caps_get_free_size(MALLOC_CAP_SPIRAM)` 查询可用 PSRAM，三档分级决定后续是否走 burst 模式（>64KB 启用，0KB 降级到直接 UDP）。

**步骤 3：WiFi STA 模式初始化**（`main.c:155-242`）— 调用 `wifi_init_sta()`，关键配置：

```c
// main.c:209-226 — C5 双频段配置
WIFI_BAND_MODE_AUTO   // 2.4G/5G 自动切换
2.4G: B/G/N/AX 全支持
5G:   N/AX 支持
带宽 BW20             // HE20 242 子载波必需
```

WiFi 重连采用**指数退避**（`main.c:119-120`）：
```c
delay_ms = 1000U << (s_retry_num - 1);  // 1s, 2s, 4s, 8s, 16s 封顶
```
最多重试 10 次，符合项目硬约束"WebSocket connections must implement exponential backoff with a maximum of 10 retries"。

**步骤 4：UDP 发送端初始化**（`main.c:285-294`）— 调用 `stream_sender_init_with(g_nvs_config.target_ip, g_nvs_config.target_port)`，失败则 3 秒后重启。

**步骤 5：CSI 采集初始化**（`main.c:296-307`）— 调用 `csi_collector_init()`。

**步骤 6：信道跳跃配置**（`main.c:309-318`）— 默认在 5GHz ch36/40/44 间跳跃，每信道停留 50ms。

**步骤 7：NDP 注入定时器**（`main.c:423-445`）— 每 10ms 发送一次空数据帧（100Hz），主动触发 AP 回应，将 CSI 频率从被动 ~15Hz 提升到 50-100Hz。

---

### 1.2 CSI 采集核心：csi_collector.c

#### 1.2.1 CSI 初始化（`csi_collector.c:408-562`）

**关键步骤**：检测当前 WiFi 频段 → 创建互斥锁 → 确定 CSI 信道（三级优先级：NVS 显式覆盖 > 已连接 AP 自动检测 > Kconfig 默认值）→ 开启 promiscuous 模式（混杂模式让 CSI 回调在所有数据帧上触发）→ 配置 CSI 参数 → 注册回调 → 启用采集 → 初始化 PSRAM burst 环形缓冲区 → 启动 flush 定时器。

**CSI 配置参数**（`csi_collector.c:477-491`）：
```c
wifi_csi_acquire_config_t csi_config = {
    .acquire_csi_legacy = false,  // L-LTF 52sc — SNR 太低
    .acquire_csi_ht20   = true,   // HT20 56sc — 11n 备选
    .acquire_csi_ht40   = true,   // HT40 114sc — 11n 备选
    .acquire_csi_su     = true,   // HE SU 242sc — 主用 ★
    .acquire_csi_mu     = false,  // MU OFDMA — 罕见无益
    .acquire_csi_dcm    = false,  // DCM 弱信号
    .acquire_csi_beamformed = false,  // BF 相位失真
    .acquire_csi_vht    = true,   // VHT20 — 第三备选
    .val_scale_cfg      = 5,      // 弱信号精度
    .dump_ack_en        = false,  // ACK CSI SNR 差
};
```

> **物理原理**：802.11ax (WiFi 6) HE-LTF 提供 242 子载波，相当于"242 个独立的距离/角度探测器"，比传统 56 子载波精度提升 4.3 倍。每个子载波对应不同频率，对环境中不同尺寸的物体响应不同。

#### 1.2.2 CSI 回调函数（`csi_collector.c:268-365`）

ESP-IDF 在每帧 CSI 数据可用时调用 `wifi_csi_callback()`，执行四步：

**步骤 1：MAC 地址过滤**（`csi_collector.c:273-277`）— 仅采集指定源 MAC 的 CSI，降低噪声。

**步骤 2：AGC 增益锁定**（`csi_collector.c:282-307`）— 三阶段状态机 `RX_GAIN_COLLECT → RX_GAIN_READY → RX_GAIN_FORCE`，将 CSI 幅度仅与信道变化绑定，而非 AGC 自动调整。

> **通俗类比**：AGC 像相机自动 ISO，会随场景亮度变化使照片无法直接对比；增益锁定相当于固定 ISO 拍摄，让 CSI 幅度仅反映真实信道变化。

**步骤 3：序列化帧并选择发送路径**（`csi_collector.c:309-353`）— 双路径设计：

```c
if (s_psram_ok) {
    // PSRAM burst 路径：SPSC 环形缓冲区，CSI 回调只做 memcpy（无阻塞 IO）
    uint32_t next = (s_burst_head + 1) % CSI_BURST_SLOTS;
    if (next != s_burst_tail) {
        memcpy(&s_burst_ring[off], frame_buf, frame_len);
        __sync_synchronize();  // 内存屏障
        s_burst_head = next;
    }
} else {
    // 直接 UDP 路径：限速 50Hz
    if ((now - s_last_send_us) >= CSI_MIN_SEND_INTERVAL_US) {
        int ret = stream_sender_send(frame_buf, frame_len);
        ...
    }
}
```

> **设计哲学**：PSRAM burst 异步路径将 CSI 高频回调与 UDP 慢速 IO 解耦；直接 UDP 路径限速 50Hz 防止 lwIP pbuf 耗尽。

**步骤 4：边缘处理入队**（`csi_collector.c:355-364`）— 原始 IQ 同时入 edge 处理的 SPSC 环，供 DSP 任务消费。

#### 1.2.3 WCES-BFP/CSI 二进制帧序列化（`csi_collector.c:142-263`）

帧布局定义：

| 偏移 | 长度 | 字段 | 类型 | 说明 |
|------|------|------|------|------|
| 0-3  | 4 字节 | Magic | u32 LE | `0xC5110001` |
| 4    | 1 字节 | Node ID | u8 | 节点编号 |
| 5    | 1 字节 | n_antennas | u8 | 天线数（C5=1） |
| 6-7  | 2 字节 | n_subcarriers | u16 LE | 子载波数 |
| 8-11 | 4 字节 | freq_mhz | u32 LE | 中心频率 |
| 12-15| 4 字节 | sequence | u32 LE | 帧序号 |
| 16   | 1 字节 | rssi | i8 | 信号强度（dBm） |
| 17   | 1 字节 | noise_floor | i8 | 噪声底（dBm） |
| 18-19| 2 字节 | reserved | u8×2 | 保留 |
| 20+  | 变长 | I/Q data | bytes | 原始 IQ 字节流 |

**Magic 解读**：`0xC5110001` = `C5`（ESP32-C5）+ `11`（802.11 协议）+ `0001`（版本 1）。

**频率推导表**（`csi_collector.c:189-219`）：
- 2.4GHz ch1-13：`freq = 2412 + (ch-1) × 5`
- 2.4GHz ch14（日本）：固定 2484MHz
- 5GHz ch36-177：`freq = 5000 + ch × 5`

> **物理原理**：WiFi 信号使用 OFDM 调制，CSI 记录每个子载波的复数信道响应 `H(k) = I + jQ`，其中 I 为同相分量、Q 为正交分量。复数的模 `|H| = √(I²+Q²)` 即幅度，辐角 `φ = atan2(Q, I)` 即相位。

---

### 1.3 UDP 传输：stream_sender.c

#### 1.3.1 Socket 初始化（`stream_sender.c:18-58`）

```c
s_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
struct timeval tv = { .tv_sec = 0, .tv_usec = 50 * 1000 };  // 50ms 超时
setsockopt(s_sock, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
int sndbuf = 64 * 1024;  // 64KB 发送缓冲
setsockopt(s_sock, SOL_SOCKET, SO_SNDBUF, &sndbuf, sizeof(sndbuf));
inet_pton(AF_INET, ip, &s_dest_addr.sin_addr);  // 点分十进制 → 网络字节序
```

**关键设计**：
- **E-1 修复**：每次重新初始化前关闭旧 socket，避免 fd 泄漏（lwIP C5 上限仅 16）
- **SO_SNDTIMEO = 50ms**：单包超时 50ms，CSI 流容忍丢包
- **SO_SNDBUF = 64KB**：burst 模式下一次性排空多个包

#### 1.3.2 sendto 调用（`stream_sender.c:65-70`）

```c
int stream_sender_send(const uint8_t *data, size_t len) {
    if (s_sock < 0) return -1;
    return sendto(s_sock, data, len, 0,
                  (struct sockaddr *)&s_dest_addr, sizeof(s_dest_addr));
}
```

阻塞式 sendto，目标地址固定。**无应用层重传**——设计哲学是 CSI 流容忍丢包，接收端通过 `s_sequence` 检测丢帧但无 NACK 机制。

---

### 1.4 边缘预处理：edge_processing.c

14 步 DSP pipeline 流程：

**步骤 1：SPSC 无锁环形缓冲区**（`edge_processing.c:68-110`）— 单生产者（CSI 回调）单消费者（DSP 任务）无锁队列，`__sync_synchronize()` 内存屏障确保多核可见性。

**步骤 2：动态采样率测量**（EMA α=0.15）— 实时跟踪真实 ESP32 传输速率。

**步骤 3：Biquad 滤波器设计**（`edge_processing.c:124-149`）— 标准 Audio EQ Cookbook Butterworth 带通：

```c
float w0 = 2.0f * M_PI * (f_lo + f_hi) / 2.0f / fs;  // 中心角频率
float bw = 2.0f * M_PI * (f_hi - f_lo) / fs;          // 归一化带宽
float alpha = sinf(w0) * sinhf(logf(2.0f) / 2.0f * bw / sinf(w0));
// 系数：b0=alpha, b1=0, b2=-alpha, a1=-2cos(w0), a2=1-alpha
```

两个滤波器：
- `bq_breathing`：0.1-0.5 Hz（呼吸 6-30 BPM）
- `bq_heartrate`：0.8-2.0 Hz（心率 48-120 BPM）

**步骤 4：相位提取与解卷绕**（`edge_processing.c:234-264`）
```c
float extract_phase(const uint8_t *iq, uint16_t idx) {
    int8_t i_val = (int8_t)iq[idx * 2];
    int8_t q_val = (int8_t)iq[idx * 2 + 1];
    return atan2f((float)q_val, (float)i_val);  // [-π, π]
}
// 解卷绕：相邻相位差 > π 时减 2π，< -π 时加 2π
```

> **物理原理**：呼吸时胸腔前后径变化约 1-5 mm。WiFi 5GHz 波长 λ = 6 cm，相位变化 Δφ = 2π·Δd/λ，当 Δd = 5mm 时 Δφ ≈ 30°，明显可测。

**步骤 5：Welford 在线方差**（`edge_processing.c:277-289`）— 长期运行不溢出的方差算法：`Var = E[X²] − (E[X])²`

**步骤 6：Top-K 子载波选择**（`edge_processing.c:423-459`）— 每 100 帧选出方差最大的 K 个子载波（默认 8 个），这些子载波对运动最敏感。

**步骤 7：零交叉 BPM 估计**（`edge_processing.c:295-330`）— 检测信号从负到正的零交叉点，平均周期 × 采样率 × 60 = BPM。

**步骤 8：Delta 压缩**（`edge_processing.c:505-543`）— XOR + RLE 两阶段压缩，Magic `0xC5110003`，每 500ms 发送一次。

---

## 🔵 第二层：UDP 接收与帧解析（服务端入口）

### 2.1 UDP 接收器：tasks/udp_receiver.rs

#### 2.1.1 函数入口与绑定（`tasks/udp_receiver.rs:29-31`）

```rust
pub(crate) async fn udp_receiver_task(state: SharedState, udp_port: u16) {
    let addr = format!("0.0.0.0:{udp_port}");
    let socket = match UdpSocket::bind(&addr).await {
        Ok(s) => { info!("..."); s }
        Err(e) => { error!("..."); return; }
    };
```

**SharedState 类型别名**（`main.rs:342`）：
```rust
pub(crate) type SharedState = Arc<RwLock<AppStateInner>>;
```
`Arc` 原子引用计数跨线程共享，`RwLock` 读写锁允许多读单写。

#### 2.1.2 三阶段写锁优化（核心设计）

`tasks/udp_receiver.rs` 采用三阶段模式最大化并发：

```
Phase 1: 写锁状态修改 (L122-405)  → 元组返回 18 个字段，锁释放
Phase 2: 无锁纯计算 (L556-673)    → DensePose、信号场、定位等
Phase 3: 写锁广播 (L675-689)      → tx.send(json) + latest_update 更新
```

> **通俗类比**：这就像"图书馆借书流程"——Phase 1 短暂进馆把书取出来（写锁），Phase 2 在馆外阅读整理（无锁），Phase 3 短暂进馆还书登记（写锁）。

#### 2.1.3 优先级 1：边缘生命体征包（WCES-BFP/Vitals）

`tasks/udp_receiver.rs:70-97` 处理 `Magic 0xC511_0002` 包，包含呼吸率、心率、存在标志、跌倒标志等，按 10Hz 节流广播。

#### 2.1.4 优先级 2：WASM 输出包（WCES-BFP/WASM）

`tasks/udp_receiver.rs:99-116` 处理 `Magic 0xC511_0005` 包，WASM 边缘模块事件，直接广播无节流。

#### 2.1.5 CSI 帧处理（核心）

`tasks/udp_receiver.rs:118-690` 处理 `Magic 0xC511_0001` 包，包含每节点独立流水线：

```rust
// 1. PerNodeState 获取或创建
let ns = s.node_states.entry(frame.node_id)
    .or_insert_with(|| crate::types::PerNodeState::new(20.0));

// 2. 动态采样率测量（EMA α=0.15）
let dt = now.duration_since(prev).as_secs_f64();
ns.measured_sample_rate = ns.measured_sample_rate * 0.85 + instantaneous * 0.15;

// 3. frame_history 环形缓冲区更新（容量 300 = 3s @ 100Hz）
ns.frame_history.push_back(frame.amplitudes.clone());
if ns.frame_history.len() > FRAME_HISTORY_CAPACITY { ns.frame_history.pop_front(); }

// 4. SignalPipeline: PhaseSanitizer → Normalize → Hampel → MotionDetector → CoherenceGate
signal_out = ns.signal_pipeline.process(&frame.amplitudes, &frame.phases, freq_hz, bw_hz);

// 5. 特征提取 (7 个特征)
let (f, _c, b, v, rm) = extract_features_from_frame(&frame, &ns.frame_history, sample_rate_hz);

// 6. VitalsBridge: IIR带通 → 呼吸率/心率
let (vb_br, vb_hr, vb_br_conf, vb_hr_conf) = vb.extract(use_amps, use_phases, tick);

// 7. CirBridge: ISTA稀疏CIR → ToF距离
cb.process(&frame.amplitudes, &frame.phases);
cir_distance_m = cb.ranging_distance_m().or_else(|| cb.dominant_distance_m());

// 8. LocalizationBridge: 多节点三角定位
s.localization_bridge.feed_observation(frame.node_id, features.mean_rssi, cir_distance_m);
let triangulated_pos: Option<[f64; 3]> = s.localization_bridge.estimate_position();

// 9. TrackingBridge: 卡尔曼+指纹re-ID
s.tracking_bridge.update(&[track_obs]);

// 10. TriageEngine: START分诊
triage_update = s.triage_engine.process(...);

// 11. EdgeModuleEngine: 19 个边缘模块
wasm_alerts = s.edge_module_engine.process_frame(...);
```

#### 2.1.6 WhōFi + 相位 Doppler 混合定位（`tasks/udp_receiver.rs:575-624`）

```rust
// WhōFi: Top-K 子载波方差 → proximity
let topk: f64 = sv[..k].iter().sum::<f64>() / k as f64;
let var_prox = (raw_var / new_max.max(1e-9)).clamp(0.0, 1.0);

// 相位 Doppler: 帧间相位差 → 多普勒代理
let phase_prox: f64 = (sum / n as f64 / PI).clamp(0.0, 1.0);

// 融合：60% WhōFi + 40% phase-doppler
let prox = (var_prox * 0.6 + phase_prox * 0.4).clamp(0.0, 1.0);
```

#### 2.1.7 多节点平方加权质心定位（`tasks/udp_receiver.rs:607-624`）

```rust
for (&cid, &w) in node_prox.iter() {
    let w2 = w * w;  // 平方加权强化强 proximity 节点
    if w2 > 0.005 {  // 阈值过滤噪声
        wx += nx * w2; wy += ny * w2; wz += nz * w2;
        tw += w2;
    }
}
let centroid: [f64; 3] = [wx / tw, wy / tw, wz / tw];
```

#### 2.1.8 错误处理与节流（`tasks/udp_receiver.rs:675-696`）

```rust
const BROADCAST_INTERVAL_MS: u64 = 100;  // 10Hz max
// 即便不广播也写入 latest_update，确保 broadcast_tick 拉取最新状态
if now.duration_since(last_broadcast) >= Duration::from_millis(BROADCAST_INTERVAL_MS) {
    let mut s = state.write().await;
    let _ = s.tx.send(json);
    s.latest_update = Some(update);
} else {
    let mut s = state.write().await;
    s.latest_update = Some(update);
}
```

---

### 2.2 WCES-BFP 帧解析器：parser.rs

#### 2.2.1 parse_esp32_frame() 核心解析（`parser.rs:79-136`）

```rust
pub(crate) fn parse_esp32_frame(buf: &[u8]) -> Option<Esp32Frame> {
    if buf.len() < 20 { return None; }  // 第一道长度检查
    
    // Magic 验证 (小端)
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != 0xC511_0001 { return None; }
    
    // 20 字节头部字段提取
    let node_id = buf[4];
    let n_antennas = buf[5];
    let n_subcarriers = u16::from_le_bytes([buf[6], buf[7]]);
    let freq_mhz = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let sequence = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let rssi = buf[16] as i8;       // 有符号转换
    let noise_floor = buf[17] as i8;
    
    // DoS 防御（BUG 10 修复）
    const MAX_PAIRS: usize = 2048;
    if n_pairs > MAX_PAIRS || n_antennas == 0 || n_subcarriers == 0 {
        warn!("Rejecting frame: n_pairs={n_pairs}...");
        return None;
    }
    
    // IQ 数据对解析（核心数学）
    for k in 0..n_pairs {
        let i_val = buf[iq_start + k * 2] as i8 as f64;     // u8 → i8 → f64
        let q_val = buf[iq_start + k * 2 + 1] as i8 as f64;
        amplitudes.push((i_val * i_val + q_val * q_val).sqrt());  // |Z| = √(I²+Q²)
        phases.push(q_val.atan2(i_val));                          // φ = atan2(Q, I)
    }
    
    Some(Esp32Frame { magic, node_id, n_antennas, n_subcarriers, 
                      freq_mhz, sequence, rssi, noise_floor, amplitudes, phases })
}
```

#### 2.2.2 IQ 数据的数学原理

WiFi CSI 本质是复数：`Z = I + jQ = |Z|·e^(jφ)`

| 转换 | 公式 | 代码位置 | 物理意义 |
|------|------|---------|---------|
| 幅度 | `|Z| = √(I² + Q²)` | `parser.rs:120` | 信号强度 |
| 相位 | `φ = atan2(Q, I)` | `parser.rs:121` | 传播延迟 |

> **为何用 atan2 而非 atan？** `atan(Q/I)` 在 I=0 时除零 panic，且 I<0 时丢失 π 相位。`atan2(Q, I)` 通过两参数符号判断正确象限，返回完整 [-π, π] 范围。

---

### 2.3 三种 Magic 帧类型汇总

| Magic | 帧类型 | 解析函数 | 用途 |
|-------|-------|---------|------|
| `0xC511_0001` | CSI 帧 | `parser.rs:79` `parse_esp32_frame()` | 原始 IQ 数据 |
| `0xC511_0002` | Vitals 包 | `parser.rs:9` `parse_esp32_vitals()` | 边缘预处理的呼吸/心率 |
| `0xC511_0005` | WASM 输出 | `parser.rs:44` `parse_wasm_output()` | WASM 模块事件 |
| `0xC511_0003` | 压缩帧 | （未实现） | Delta 压缩 IQ |

---

## 🟡 第三层：信号处理与生命体征检测

### 3.1 信号处理核心：signal_processing.rs

#### 3.1.1 extract_features_from_frame()（`signal_processing.rs:246-365`）

提取 7 个特征向量：

```rust
let mean_amp: f64 = frame.amplitudes.iter().sum::<f64>() / n;        // 均值
let intra_variance = ...                                              // 空间方差
let temporal_variance = compute_subcarrier_variances(...)            // 时序方差
let variance = intra_variance.max(temporal_variance);                // 取大
let spectral_power = frame.amplitudes.iter().map(|a| a*a).sum() / n; // 谱功率
let motion_band_power = ...                                           // 高频子载波功率
let breathing_band_power = ...                                        // 低频子载波功率
let dominant_freq_hz = peak_idx as f64 * 0.05;                        // 主频估计
let change_points = ...                                                // 变化点计数
```

#### 3.1.2 运动检测算法（`signal_processing.rs:308-333`）

```rust
// 帧间时序差分运动分（主项，权重 0.40）
let diff_energy: f64 = (0..n_cmp)
    .map(|k| (frame.amplitudes[k] - prev_frame[k]).powi(2))
    .sum::<f64>() / n_cmp as f64;
let temporal_motion_score = (diff_energy / ref_energy).sqrt().clamp(0.0, 1.0);

// 加权融合
let motion_score = (temporal_motion_score * 0.4   // 帧间时序变化
                  + variance_motion * 0.2          // 时序方差
                  + mbp_motion * 0.25               // 高频子载波功率
                  + cp_motion * 0.15).clamp(0.0, 1.0);  // 变化点穿越
```

#### 3.1.3 Goertzel 算法（`signal_processing.rs:125-183`）

```rust
let omega = 2.0 * PI * freq / sample_rate_hz;
let coeff = 2.0 * omega.cos();
let mut s_prev2 = 0.0; let mut s_prev1 = 0.0;
for &x in &detrended {
    let s = x + coeff * s_prev1 - s_prev2;  // 二阶 IIR 递推
    s_prev2 = s_prev1;
    s_prev1 = s;
}
let power = s_prev2 * s_prev2 + s_prev1 * s_prev1 - coeff * s_prev1 * s_prev2;
```

> **数学原理**：Goertzel 利用二阶 IIR 滤波器高效计算单频点 DFT。复杂度单频点 O(N)，FFT 全频谱 O(N log N)。当只需少数频点时 Goertzel 更优。
>
> **通俗类比**：FFT 是"全频谱扫描"，Goertzel 是"调谐到特定电台"——只想听一个台时调谐更快。

#### 3.1.4 信号场生成（`signal_processing.rs:28-110`）

实际代码使用 **40×40 网格**（不是 README 描述的 20×20）：

```rust
for (k, &var) in subcarrier_variances.iter().enumerate() {
    let weight = (var / norm_factor) * motion_score;
    let angle = (k as f64 / n_sub as f64) * 2.0 * PI;     // 子载波索引 → 角度
    let radius = center * 0.8 * weight.sqrt();
    let hx = center + radius * angle.cos();
    let hz = center + radius * angle.sin();
    
    for z in 0..grid {
        for x in 0..grid {
            let dist2 = (x - hx).powi(2) + (z - hz).powi(2);
            let spread = (0.5 + weight * 2.0).max(0.5);
            values[z * grid + x] += weight * (-dist2 / (2.0 * spread * spread)).exp();
        }
    }
}
```

> **物理原理**：把子载波索引 k 当作"角度 bin"，高方差子载波在该方向产生高斯"光斑"，重建运动的空间分布图。

---

### 3.2 生命体征检测：vital_signs.rs

#### 3.2.1 配置常量（`vital_signs.rs:21-33`）

```rust
const BREATHING_MIN_HZ: f64 = 0.1;       // 6 BPM
const BREATHING_MAX_HZ: f64 = 0.5;       // 30 BPM
const HEARTBEAT_MIN_HZ: f64 = 0.667;     // 40 BPM
const HEARTBEAT_MAX_HZ: f64 = 2.0;       // 120 BPM
const MIN_BREATHING_SAMPLES: usize = 40; // ~2s at 20 Hz
const MIN_HEARTBEAT_SAMPLES: usize = 30; // ~1.5s at 20 Hz
const CONFIDENCE_THRESHOLD: f64 = 2.0;
```

| 频段 | 频率范围 | BPM 范围 | 物理意义 |
|---|---|---|---|
| 呼吸 | 0.1-0.5 Hz | 6-30 BPM | 成人静息 12-20 BPM，运动时可达 30 |
| 心跳 | 0.667-2.0 Hz | 40-120 BPM | 成人静息 60-100 BPM，运动时可达 120+ |

#### 3.2.2 帧处理（`vital_signs.rs:127-191`）

```rust
// 呼吸特征：均值幅度（呼吸引起胸腔 1-5mm 起伏调制 CSI 幅度）
let mean_amp: f64 = amplitude.iter().sum::<f64>() / n;
self.breathing_buffer.push_back(mean_amp);

// 心跳特征：相位方差（心跳引起 <0.5mm 微小振动，相位更敏感）
let phase_var = if phase.len() > 1 {
    let mean_phase: f64 = phase.iter().sum::<f64>() / phase.len() as f64;
    phase.iter().map(|p| (p - mean_phase).powi(2)).sum::<f64>() / phase.len() as f64
} else {
    // Fallback: 上半子载波幅度方差
    ...
};
```

> **物理原理**：
> - 呼吸：胸腔起伏 1-5 mm，5GHz 波长 6cm，Δφ = 2π·5/60 ≈ 30°，可被 CSI 幅度直接捕获
> - 心跳：体表位移 <0.5 mm，Δφ ≈ 1.4°，单子载波难以分辨，但跨子载波相位方差对微小扰动更敏感

#### 3.2.3 FFT 峰值检测（`vital_signs.rs:219-326`）

7 步流程：
1. 零填充到 2 的幂
2. Hann 窗 `w[n] = 0.5·(1 − cos(2πn/(N−1)))` 减少频谱泄漏
3. FFT 频谱计算（Cooley-Tukey radix-2 DIT）
4. 频段 bin 范围（频率分辨率 Δf = fs/N）
5. 找峰值 + 平均，`peak_ratio = peak_mag / band_mean`
6. **抛物线插值**（亚 bin 精度）：`p = 0.5·(α − γ)/(α − 2β + γ)`
7. BPM 转换：`bpm = peak_freq × 60`

#### 3.2.4 FIR 带通滤波器（`vital_signs.rs:390-464`）

窗 sinc FIR 设计：
```rust
// 理想 LPF 冲激响应
let lp_high = (2.0 * PI * high_norm * n).sin() / (PI * n);  // sinc(2·high·n)·2·high
let lp_low  = (2.0 * PI * low_norm * n).sin() / (PI * n);
// BPF = LPF(high) − LPF(low)
coeffs[i] = (lp_high - lp_low) * w;  // w = Hamming 窗
```

> **通俗类比**：带通滤波像"筛子筛沙"——只让指定大小的颗粒（频率）通过，过大过小都被挡住。

#### 3.2.5 radix-2 DIT FFT（`vital_signs.rs:472-543`）

完整 Cooley-Tukey 实现，纯 Rust 无外部依赖：

```rust
// 比特反转置换
bit_reverse_permute(&mut real, &mut imag);

// 蝶形运算（O(N log N)）
let mut size = 2;
while size <= n {
    let half = size / 2;
    let angle_step = -2.0 * PI / size as f64;
    for start in (0..n).step_by(size) {
        for k in 0..half {
            let angle = angle_step * k as f64;
            let wr = angle.cos(); let wi = angle.sin();
            let i = start + k; let j = start + k + half;
            let tr = wr * real[j] - wi * imag[j];
            let ti = wr * imag[j] + wi * real[j];
            real[j] = real[i] - tr; imag[j] = imag[i] - ti;
            real[i] += tr;          imag[i] += ti;
        }
    }
    size *= 2;
}
```

> **数学原理**：将 N 点 DFT `X[k] = Σ x[n]·e^(−2πi·kn/N)` 递归分解为两个 N/2 点 DFT。复杂度从 O(N²) 降到 O(N log N)。
>
> **通俗类比**：FFT 像"三棱镜分光"——白光（时域混合信号）通过棱镜后展开为彩虹（频域频谱），每个 bin 对应一个频率"颜色"。

---

### 3.3 状态操作：state_ops.rs

#### 3.3.1 自适应阈值分级（`state_ops.rs:20-66`）

```rust
// 自适应基线（warmup 期 α=0.1，稳定期 α=0.003）
if state.baseline_frames < BASELINE_WARMUP {
    state.baseline_motion = state.baseline_motion * 0.9 + raw_motion * 0.1;
} else if raw_motion < state.smoothed_motion + 0.05 {
    state.baseline_motion = state.baseline_motion * (1.0 - BASELINE_EMA_ALPHA)
                          + raw_motion * BASELINE_EMA_ALPHA;
}

// 基线扣减 + EMA 平滑
let adjusted = (raw_motion - state.baseline_motion * 0.7).max(0.0);
state.smoothed_motion = state.smoothed_motion * (1.0 - MOTION_EMA_ALPHA)
                      + adjusted * MOTION_EMA_ALPHA;

// 分类 + 5 帧消抖（DEBOUNCE_FRAMES = 4）
let candidate = raw_classify(sm);
if candidate == state.debounce_candidate {
    state.debounce_counter += 1;
    if state.debounce_counter >= DEBOUNCE_FRAMES {
        state.current_motion_level = candidate;
    }
}
```

#### 3.3.2 生命体征平滑（`state_ops.rs:99-154`）

```rust
// 异常值剔除（最大跳变阈值）
let hr_ok = state.smoothed_hr < 1.0 || (raw_hr - state.smoothed_hr).abs() < HR_MAX_JUMP;
if hr_ok && raw_hr > 0.0 {
    state.hr_buffer.push_back(raw_hr);
    if state.hr_buffer.len() > VITAL_MEDIAN_WINDOW { state.hr_buffer.pop_front(); }
}

// trimmed_mean（25% 截尾均值）
let trimmed_hr = trimmed_mean(&state.hr_buffer);

// 死区 + EMA 平滑
if (trimmed_hr - state.smoothed_hr).abs() > HR_DEAD_BAND {
    state.smoothed_hr = state.smoothed_hr * (1.0 - VITAL_EMA_ALPHA)
                      + trimmed_hr * VITAL_EMA_ALPHA;
}
```

> **通俗类比**：死区机制像"惯性体温计"——读数缓慢追随真实温度，不会被瞬时冷热干扰。

---

## 🟠 第四层：START 分诊与伤员追踪

### 4.1 START 分诊核心：mat_pipeline.rs

#### 4.1.1 calculate_triage() 函数（`mat_pipeline.rs:226-270`）

```rust
pub fn calculate_triage(input: &VitalSignsInput) -> TriageLevel {
    // 1. 信号质量门槛：< 0.05 → Unknown (灰)
    if input.signal_quality < 0.05 {
        return TriageLevel::Unknown;
    }
    
    // 2. 无生命体征 ≠ 死亡 (BUG 50 修复) → Unknown
    if br.is_none() && hr.is_none() {
        return TriageLevel::Unknown;
    }
    
    // 3. Immediate (红) — 呼吸率 >30 或 <10
    if let Some(b) = br {
        if b > 30.0 || b < 10.0 {
            return TriageLevel::Immediate;
        }
    }
    
    // 4. Immediate (红) — 心率 >120 或 <40
    if let Some(h) = hr {
        if h > 120.0 || h < 40.0 {
            return TriageLevel::Immediate;
        }
    }
    
    // 5. Minor (绿) — 自主移动 + 体征正常
    if motion_score > 0.3 && b 12-24 && h 50-100 {
        return TriageLevel::Minor;
    }
    
    // 6. Minor (绿) — 体征完全正常
    if b 12-20 && h 60-100 {
        return TriageLevel::Minor;
    }
    
    // 7. Delayed (黄) — 兜底
    TriageLevel::Delayed
}
```

**TriageLevel 枚举**（`mat_pipeline.rs:162-223`）符合项目硬约束"TriageLevel enum must follow the order: Unknown=0, Minor=1, Delayed=2, Immediate=3, Deceased=4"：

```rust
#[repr(u8)]
pub enum TriageLevel {
    Unknown = 0,    // 灰
    Minor = 1,      // 绿
    Delayed = 2,    // 黄
    Immediate = 3,  // 红
    Deceased = 4,   // 黑
}
```

#### 4.1.2 START 协议医学依据

| 体征 | 正常范围 | 危重阈值 | 病理机制 |
|------|---------|---------|---------|
| 呼吸率 RR | 12-20 /min | >30 或 <10 | >30=呼吸急促（缺氧/休克代偿）；<10=呼吸抑制（颅脑伤/濒死） |
| 心率 HR | 60-100 BPM | >120 或 <40 | >120=心动过速（失血代偿）；<40=心动过缓（濒死前兆） |

> **通俗类比**：START 分诊像"急诊科预检分诊"——快速判断谁需要立即救治（红）、谁可以等一会（黄）、谁轻伤能走（绿）、谁已无生命体征（黑）。

#### 4.1.3 RSSI 对数路径损失模型（`mat_pipeline.rs:279-281`）

```rust
fn rssi_to_distance(rssi: f64, ref_rssi: f64, n: f64) -> f64 {
    10.0_f64.powf((ref_rssi - rssi.max(-90.0)) / (10.0 * n))
}
```

**数学推导**：

1. 自由空间 Friis 方程：`P_r(d) ∝ 1/d²`
2. 取对数得 dB 形式：`PL(d)_{dB} = PL(d_0)_{dB} + 10n·log₁₀(d/d_0)`
3. RSSI 与路径损失关系：`RSSI(d) = RSSI(d_0) − 10n·log₁₀(d)`
4. 反解距离 d：`d = 10^((ref_rssi − rssi)/(10n))`

**参数物理意义**：
- `ref_rssi = -30.0` dBm：1 米处校准的 RSSI
- `n = 3.0`：室内路径损失指数（n=2 自由空间，n=3 一格墙，n=4 多墙遮挡）
- `rssi.max(-90.0)`：信号下限钳制

**数值示例**：rssi=-60, ref=-30, n=3 → d = 10^(30/30) = 10 米

#### 4.1.4 伤员匹配（`mat_pipeline.rs:540-603`）

四步策略：
1. **精确匹配**：同 person_id + 同 node_id + 5 秒内
2. **跨节点生物特征匹配**（阈值 0.65）：8 维嵌入向量 + 余弦相似度
3. **重识别**：从 lost_pool 匹配（阈值 0.75，更严格）
4. **创建新伤员**：ID 格式 `SURV-{:03x}`

> **通俗类比**：像医院急诊室的"病历匹配系统"——先用身份证号（person_id）精确匹配；若失败用面部特征（生物特征嵌入）模糊匹配；若仍失败查"失踪人口档案"（lost_pool）；最后才新建病历。

#### 4.1.5 恶化检测（`mat_pipeline.rs:461-482`）

```rust
if s.triage.priority() > s.prev_triage.priority() 
   && s.prev_triage != TriageLevel::Unknown {
    s.deterioration_count += 1;
    if s.deterioration_count >= self.config.deterioration_window {  // 5 帧
        self.alerts.push_back(AlertSnapshot {
            alert_type: "DETERIORATION".to_string(),
            message: format!("{} → {}", s.prev_triage.name(), s.triage.name()),
            priority: s.triage.priority(),
        });
        while self.alerts.len() > 500 { self.alerts.pop_front(); }  // 上限 500 条
    }
}
```

#### 4.1.6 年龄估算（`mat_pipeline.rs:701-708`）

```rust
fn estimate_age(br: Option<f64>, hr: Option<f64>) -> String {
    match (br, hr) {
        (Some(b), Some(h)) if b > 25.0 && h > 100.0 => "Infant (<2y)",
        (Some(b), Some(h)) if b > 18.0 && h > 80.0  => "Child (2-12y)",
        (Some(b), Some(h)) if b < 16.0 && h < 65.0  => "Elderly (60y+)",
        _ => "Adult",
    }
}
```

**医学依据**：婴儿肺泡表面积/体重比小，需更高呼吸频率满足代谢；婴儿心脏每搏输出量小，需更高心率维持心输出量。

---

### 4.2 边缘模块引擎：edge_module_engine.rs

19 个原生 Rust 编译的医疗 WASM 模块：

| # | 模块名 | 中文名 | 用途 |
|---|--------|-------|------|
| 1 | vital_trend | 生命体征趋势 | 呼吸暂停/心动过速等 |
| 2 | lrn_anomaly_attractor | 混沌吸引子异常 | 异常检测 |
| 3 | coherence | CSI 相干性 | 信号质量门控 |
| 4 | med_respiratory_distress | 呼吸窘迫 | 医疗检测 |
| 5 | ind_confined_space | 密闭空间监护 | 工业场景 |
| 6 | sec_panic_motion | 恐慌动作 | 安防场景 |
| 7 | med_sleep_apnea | 睡眠呼吸暂停 | 医疗检测 |
| 8 | med_cardiac_arrhythmia | 心律失常 | 医疗检测 |
| 9 | med_seizure_detect | 癫痫检测 | 医疗检测 |
| 10 | intrusion | 入侵检测 | 安防场景 |
| 11 | occupancy | 空间人数统计 | 占用检测 |
| 12 | sig_mincut | 多人 CSI 身份匹配 | 信号处理 |
| 13 | sec_weapon_detect | 暴力/武器检测 | 安防场景 |
| 14 | sig_sparse_recovery | 稀疏子载波恢复 | 信号处理 |
| 15 | med_gait_analysis | 步态分析 | 医疗检测 |
| 16 | sec_loitering | 徘徊检测 | 安防场景 |
| 17 | ind_structural_vibration | 建筑振动 | 工业场景 |
| 18 | lrn_meta_adapt | 元学习参数自适应 | 自适应阈值 |
| 19 | tmp_temporal_logic_guard | LTL 时态逻辑守卫 | 安全规则 |

**Module 19 LTL（Linear Temporal Logic）守卫**：用 `G(φ)`（Globally 始终满足）和 `F(φ)`（Finally 最终满足）算子表达安全规则，如 `G(no_persons + person_id_active → violation)`。

---

## 🟣 第五层：Web 可视化

### 5.1 Web 入口：docs/triage-ui/triage.html

#### 5.1.1 HTML 整体结构（CSS Grid 双栏布局）

```
┌─────────────────────────────────┬──────────────────┐
│                                 │  分诊统计          │
│         位置地图 / 3D骨架         │  伤情评估          │
│         (Canvas 2D)             │  当前生命体征       │
│         (Three.js WebGL)        │  伤员列表          │
│                                 │  告警通知          │
│                                 │  边缘模块 (19)     │
│                                 │  Node Vitals      │
│                                 │  Agent 辅助分析    │
└─────────────────────────────────┴──────────────────┘
```

#### 5.1.2 NODES 全局状态（`triage.html:573-577`）

3 个 ESP32-C5 节点的屏幕坐标（canvas 像素）——等边三角形布局，与 Rust 端 `node_positions()` 一致。

#### 5.1.3 drawMap() 函数（`triage.html:624-702`）

```javascript
function drawMap(){
    // 背景渐变
    const bgGrad=ctx.createLinearGradient(0,0,0,mapH);
    bgGrad.addColorStop(0,'#1C1C1E'); bgGrad.addColorStop(1,'#0F0F10');
    
    // 热力图（可选）
    if(showHeatmap && latestData?.signal_field?.values) {
        // 按 grid_size 渲染色块：>0.7 红, >0.35 黄, 否则绿
    }
    
    // 节点绘制：虚线范围圆 + 菱形标记 + 标签 + 实时体征
    NODES.forEach(node=>{ ... });
    
    // 伤员绘制：按 triage_color 着色（red/yellow/green/black/gray）
    latestTriage.survivors.forEach(s=>{
        const [cx, cy] = s.position ? physToCanvas(s.position[0], s.position[1]) 
                                    : [mapW/2, mapH/2];
        // 恶化/红色伤员有外发光圈
    });
}
```

**物理坐标→Canvas 坐标转换**（`triage.html:618-622`）：
```javascript
function physToCanvas(px, py) {
    const normX = (px - MAP_BOUNDS.minX) / MAP_BOUNDS.sizeX;
    const normY = (py - MAP_BOUNDS.minY) / MAP_BOUNDS.sizeY;
    return [margin + normX*(mapW-2*margin), margin + normY*(mapH-2*margin)];
}
```

#### 5.1.4 WebSocket 连接逻辑（`triage.html:1242-1290`）

```javascript
function connectWebSocket(){
    const wsUrl = `${wsProto}//${location.hostname}:${location.port||8080}/ws/sensing${wsTokenQuery}`;
    ws = new WebSocket(wsUrl);
    
    ws.onopen = () => {
        wsReconnectAttempts = 0;
        document.getElementById('statusDot').className = 'status-dot online';
    };
    
    ws.onmessage = (e) => {
        const d = JSON.parse(e.data);
        if(d.type==='sensing_update') handleUpdate(d);
        else if(d.type==='alert') handleAlertMessage(d);
        else if(d.type==='edge_vitals') handleEdgeVitals(d);
        else if(d.type==='agent_stream') handleAgentStream(d);
        // ... 6 种消息类型
    };
    
    ws.onclose = () => {
        // 指数退避重连（符合项目硬约束 max 10 retries）
        if(wsReconnectAttempts < WS_MAX_RECONNECT) {
            wsReconnectAttempts++;
            const delay = Math.min(1000*Math.pow(2, wsReconnectAttempts), 30000);
            setTimeout(connectWebSocket, delay);
        }
    };
}
```

#### 5.1.5 renderFromServer() 性能优化（`triage.html:1017-1146`）

```javascript
function renderFromServer(){
    // 1. 廉价更新（每次）：统计数字、体征数字
    document.getElementById('statTotal').textContent = a.total;
    
    // 2. 限流渲染（150ms 最小间隔 = ~7 FPS）
    if(now - lastRenderStamp >= MIN_RENDER_MS) {
        // 2.1 伤员列表（按严重度排序）
        // 2.2 _lastSurvivorHtml 缓存：内容未变则跳过 innerHTML 赋值
        if(_lastSurvivorHtml !== newSurvivorHtml) {
            _lastSurvivorHtml = newSurvivorHtml;
            survivorsList.innerHTML = newSurvivorHtml;
        }
    }
}
```

#### 5.1.6 伤员卡片（`triage.html:1045-1056`）

```javascript
newSurvivorHtml = sortedSurvivors.map((s, idx) => `
    <div class="survivor-card ${escapeHtml(s.triage_color)}" 
         data-survivor-id="${escapeHtml(s.id)}">
        <div class="survivor-header">
            <h3>${escapeHtml(s.id)}</h3>
            <p>追踪 ${s.tracked_seconds?.toFixed(0)??'--'}s · 节点${escapeHtml(s.node_id)} · ${escapeHtml(s.estimated_age)}</p>
            ${s.is_deteriorating ? '<span class="deteriorating-badge">⚠ 恶化</span>' : ''}
            ${s.reidentified ? '<span class="reid-badge">🔄 重识别</span>' : ''}
        </div>
        <div class="vitals-row">
            <div class="vital-item"><div class="vital-icon breathing">🫁</div>
                <div class="vital-value">${s.breathing_rate?.toFixed(1)??'--'}</div>
                <div class="vital-unit">呼吸/分</div></div>
            <div class="vital-item"><div class="vital-icon heart">❤️</div>
                <div class="vital-value">${s.heart_rate?.toFixed(0)??'--'}</div>
                <div class="vital-unit">心率BPM</div></div>
        </div>
    </div>`).join('');
```

所有用户可控字段都经 `escapeHtml()` 转义，符合项目硬约束"All HTML user input must be escaped using escapeHtml() before inserting into innerHTML"。

#### 5.1.7 3D 骨架 Three.js（`triage.html:735-950`）

```javascript
function init3D(){
    scene = new THREE.Scene();
    camera = new THREE.PerspectiveCamera(50, w/Math.max(h,1), .1, 1000);
    renderer = new THREE.WebGLRenderer({canvas:canvasEl, antialias:true, alpha:true});
    controls = new THREE.OrbitControls(camera, renderer.domElement);
    
    // 三点光照
    scene.add(new THREE.AmbientLight(0x334466, .6));
    const key = new THREE.DirectionalLight(0x4488cc, 1.2); key.castShadow = true;
    // ...
    
    createFigurePool();  // 4 个姿态池（最多同屏 4 人）
    animate();
}
```

**17 关键点骨架**：COCO 17 格式（鼻、眼、耳、肩、肘、腕、髋、膝、踝）+ 16 根骨骼连接。

---

### 5.2 WebSocket 推送：handlers/ws.rs

#### 5.2.1 认证机制（`handlers/ws.rs:22-43`）

```rust
static WS_AUTH_TOKEN: LazyLock<Option<String>> =
    LazyLock::new(|| std::env::var("WCES_WS_TOKEN").ok().filter(|s| !s.empty()));

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) { diff |= x ^ y; }
    diff == 0  // 常量时间比较，防时序侧信道攻击
}
```

#### 5.2.2 handle_ws_client 核心循环（`handlers/ws.rs:65-345`）

```rust
pub(crate) async fn handle_ws_client(mut socket: WebSocket, state: SharedState) {
    let mut rx = {
        let s = state.read().await;
        s.tx.subscribe()  // 订阅 broadcast channel
    };
    
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(json) => {
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(Lagged(n)) => warn!("WS lagged by {} messages", n),
                    Err(Closed) => break,
                }
            }
            msg = socket.recv() => {
                // 处理 ping/patient_register/agent_analyze_request
            }
        }
    }
}
```

---

### 5.3 广播任务：tasks/broadcast_tick.rs

```rust
pub(crate) async fn broadcast_tick_task(state: SharedState, tick_ms: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(tick_ms));
    
    loop {
        interval.tick().await;
        
        // 1. 排空 AlertingBridge 中的待发告警
        {
            let mut s = state.write().await;
            let alerts = s.alerting_bridge.drain_alerts();
            for alert in &alerts {
                let json = serde_json::to_string(&serde_json::json!({
                    "type": "alert", ...
                })).unwrap();
                let _ = s.tx.send(json);
            }
        }
        
        // 2. 重广播最新 sensing_update（仅当有订阅者）
        let s = state.read().await;
        if let Some(ref update) = s.latest_update {
            if s.tx.receiver_count() > 0 {  // 无订阅者时跳过
                let json = serde_json::to_string(update).unwrap();
                let _ = s.tx.send(json);
            }
        }
    }
}
```

> **通俗类比**：这像广播电台的"整点报时"——即使没有新节目，也要定时播报让听众知道电台还活着；同时把记者发来的快讯（告警）插播出去。

---

## 🔄 端到端数据流总览

```
ESP32-C5 ×3 (采集层)
    │
    │ 1. wifi_csi_callback() [csi_collector.c:268]
    │    ├─ MAC 过滤
    │    ├─ AGC 增益锁定
    │    ├─ csi_serialize_frame() → WCES-BFP/CSI 二进制帧 (Magic 0xC511_0001)
    │    └─ PSRAM burst 环形缓冲 / 直接 UDP (限速 50Hz)
    │
    │ 2. stream_sender_send() [stream_sender.c:65]
    │    └─ sendto() → UDP:5005
    │
    ▼
Rust 服务端 (处理层)
    │
    │ 3. udp_receiver_task() [tasks/udp_receiver.rs:29]
    │    └─ socket.recv_from(&mut buf).await
    │
    │ 4. parse_esp32_frame() [parser.rs:79]
    │    ├─ Magic 验证 0xC511_0001
    │    ├─ 20 字节头部提取
    │    ├─ IQ → 幅度 (√(I²+Q²)) + 相位 (atan2(Q,I))
    │    └─ DoS 防御 (MAX_PAIRS=2048)
    │
    │ 5. Phase 1: 写锁状态修改 [tasks/udp_receiver.rs:122-405]
    │    ├─ PerNodeState 更新（frame_history 环形缓冲）
    │    ├─ SignalPipeline: PhaseSanitizer → Normalize → Hampel → MotionDetector → CoherenceGate
    │    ├─ extract_features_from_frame() → 7 个特征
    │    ├─ VitalsBridge: IIR 带通 → 呼吸率/心率
    │    ├─ CirBridge: ISTA 稀疏 CIR → ToF 距离
    │    ├─ LocalizationBridge: 多节点三角定位
    │    ├─ TrackingBridge: 卡尔曼+指纹 re-ID
    │    ├─ TriageEngine: START 分诊
    │    └─ EdgeModuleEngine: 19 个边缘模块
    │
    │ 6. Phase 2: 无锁纯计算 [tasks/udp_receiver.rs:556-673]
    │    ├─ generate_synthetic_pose() (DensePose)
    │    ├─ generate_signal_field() (40×40 信号场)
    │    ├─ WhōFi + 相位 Doppler 混合定位
    │    └─ 构建 SensingUpdate JSON
    │
    │ 7. Phase 3: 写锁广播 [tasks/udp_receiver.rs:675-689]
    │    └─ s.tx.send(json) → broadcast::channel
    │
    ▼
broadcast::channel (多接收者广播)
    │
    ├─→ handle_ws_client (sensing 端点)
    │     rx.recv() → socket.send(Message::Text(json))
    │
    └─→ broadcast_tick_task (保活任务)
          每 100ms 重广播最新 update + 排空告警
    │
    ▼
浏览器 (可视化层)
    │
    │ 8. ws.onmessage [triage.html:1266]
    │    └─ JSON.parse(e.data) → handleUpdate(d)
    │
    │ 9. handleUpdate() [triage.html:1214]
    │    ├─ 更新 NODES 元数据
    │    ├─ 自动计算 MAP_BOUNDS
    │    └─ latestTriage = data.triage_update → renderFromServer()
    │
    │ 10. renderFromServer() [triage.html:1017]
    │     ├─ 廉价更新（每次）：统计数字、体征数字
    │     └─ 限流更新（150ms）：伤员卡片、告警列表、sparkline、地图
    │
    │ 11. drawMap() [triage.html:624]
    │     └─ Canvas 2D: 节点 + 伤员圆点 + 热力图叠加
    │
    │ 12. animate() [triage.html:871]
    │     └─ Three.js WebGL: 17 关键点骨架渲染
```

---

## 📊 数据结构跨模块传递

| 数据结构 | 定义位置 | 生产者 | 消费者 |
|---------|---------|--------|--------|
| `Esp32Frame` | `types.rs:37` | `parser.rs:124` | `udp_receiver.rs:118` |
| `Esp32VitalsPacket` | `types.rs:180` | `parser.rs:28` | `udp_receiver.rs:71` |
| `PerNodeState` | `types.rs:216` | `types.rs:255` | `udp_receiver.rs:147` |
| `SensingUpdate` | `types.rs:54` | `udp_receiver.rs:642` | `ws.rs:377` |
| `VitalSigns` | `vital_signs.rs` | `udp_receiver.rs:227` | `SensingUpdate.vital_signs` |
| `TriageUpdate` | `mat_pipeline.rs` | `udp_receiver.rs:286` | `SensingUpdate.triage_update` |

---

## 🎯 关键设计亮点总结

### 1. 双轨呼吸检测
- **Goertzel 9 点扫频**（`signal_processing.rs:125`）：轻量、低延迟，驱动 UI
- **FFT + 抛物线插值**（`vital_signs.rs:219`）：高分辨率、高精度，驱动数据归档

### 2. 多层异常值剔除（防御性设计）
- **物理层**：Hampel 滤波
- **特征层**：相位方差 fallback
- **时序层**：HR_MAX_JUMP/BR_MAX_JUMP 跳变门限
- **统计层**：trimmed_mean 25% 截尾
- **门控层**：CoherenceState + GatePolicy

### 3. 迟滞与死区机制（防抖动）
- **人数估计**：上升/下降阈值不对称
- **状态分级**：5 帧消抖
- **生命体征显示**：死区抑制蠕动

### 4. 项目硬约束实现
- ✅ **TriageLevel 顺序**：Unknown=0, Minor=1, Delayed=2, Immediate=3, Deceased=4（`mat_pipeline.rs:162`）
- ✅ **WebSocket 指数退避 max 10 retries**（`triage.html:1242`）
- ✅ **HTML 输入 escapeHtml**（`triage.html:1045`）
- ✅ **VecDeque 替代 Vec FIFO**（`mat_pipeline.rs:285-312` TrackedSurvivor）

### 5. 性能优化
- **三阶段锁模式**：Phase 1 写锁（状态）→ Phase 2 无锁（计算）→ Phase 3 写锁（广播）
- **延迟异步**：LLM `push_vitals`、Agent `analyze` 均延迟到锁外执行
- **节流广播**：10Hz 上限避免 WebSocket 通道溢出
- **DOM diff 缓存**：`_lastSurvivorHtml` 内容未变则跳过 innerHTML 赋值

### 6. 安全防御
- **DoS 防御**：MAX_PAIRS=2048 限制恶意包
- **常量时间认证**：constant_time_eq 防时序攻击
- **超时保护**：Agent 分析 30s 超时，锁获取 1s 超时
- **Ed25519 WASM 签名验证**：32 字节公钥严格校验

---

## 📚 数学公式速查表

| 公式 | 代码位置 | 物理意义 |
|------|---------|---------|
| `|Z| = √(I² + Q²)` | `parser.rs:120` | 复数模（信号幅度） |
| `φ = atan2(Q, I)` | `parser.rs:121` | 复数辐角（信号相位） |
| `Δφ = 2π·Δd/λ` | 物理推导 | 位移→相位变化 |
| `Var[X] = E[X²] − (E[X])²` | `signal_processing.rs:199` | Welford 单遍方差 |
| `y[n] = α·x[n] + (1−α)·y[n−1]` | `state_ops.rs:37` | EMA 指数移动平均 |
| `s[n] = x[n] + 2cos(ω)·s[n−1] − s[n−2]` | `signal_processing.rs:154` | Goertzel 二阶 IIR |
| `d = 10^((ref_rssi − rssi)/(10n))` | `mat_pipeline.rs:280` | RSSI 对数路径损失反解 |
| `w[n] = 0.5·(1 − cos(2πn/(N−1)))` | `vital_signs.rs:240` | Hann 窗 |
| `X[k] = E[k] + W_N^k·O[k]` | `vital_signs.rs:508` | Cooley-Tukey FFT 蝶形 |
| `p = 0.5·(α − γ)/(α − 2β + γ)` | `vital_signs.rs:293` | 抛物线插值（亚 bin 精度） |

---

## 📖 关键代码文件索引

### 固件层（C 语言）
- `firmware/esp32-c5-csi-node/main/main.c` — ESP32 启动入口
- `firmware/esp32-c5-csi-node/main/csi_collector.c` — CSI 采集核心
- `firmware/esp32-c5-csi-node/main/stream_sender.c` — UDP 传输
- `firmware/esp32-c5-csi-node/main/edge_processing.c` — 边缘 DSP
- `firmware/esp32-c5-csi-node/main/nvs_config.c` — 配置管理

### 服务端（Rust）
- `rust-server/crates/wifi-densepose-sensing-server/src/tasks/udp_receiver.rs` — UDP 接收
- `rust-server/crates/wifi-densepose-sensing-server/src/parser.rs` — WCES-BFP 帧解析
- `rust-server/crates/wifi-densepose-sensing-server/src/signal_processing.rs` — 信号处理
- `rust-server/crates/wifi-densepose-sensing-server/src/vital_signs.rs` — 生命体征检测
- `rust-server/crates/wifi-densepose-sensing-server/src/state_ops.rs` — 有状态操作
- `rust-server/crates/wifi-densepose-sensing-server/src/mat_pipeline.rs` — START 分诊
- `rust-server/crates/wifi-densepose-sensing-server/src/edge_module_engine.rs` — 19 边缘模块
- `rust-server/crates/wifi-densepose-sensing-server/src/handlers/ws.rs` — WebSocket 推送
- `rust-server/crates/wifi-densepose-sensing-server/src/tasks/broadcast_tick.rs` — 保活广播

### 可视化（Web）
- `docs/triage-ui/triage.html` — 分诊仪表盘

---

## 📝 总结

本分析报告覆盖了 WCES 项目从 CSI 数据采集、UDP 传输、Rust 服务端处理到 Web 可视化的完整端到端流程，所有内容均基于实际代码逐行分析，包含数学公式推导、物理原理讲解和通俗易懂的类比，既适合零基础人员理解，也提供资深工程师所需的代码细节。

**核心数据流**：ESP32-C5 采集 → WCES-BFP/CSI 二进制帧 → UDP 传输 → Rust 解析 → 信号处理（FFT/Goertzel）→ 生命体征检测 → START 分诊 → 伤员追踪 → WebSocket 推送 → 浏览器渲染（Canvas 2D + Three.js WebGL）。

**技术栈**：
- 固件层：ESP-IDF v6.0.1 + C + FreeRTOS + PSRAM
- 服务端：Rust + Tokio + Axum + ndarray + serde
- 可视化：HTML5 Canvas + Three.js r140 + WebSocket + CSS Grid

**项目规模**：Rust ~100K 行，C 固件 8,322 行，10 个 Rust crate，19 个边缘模块。
