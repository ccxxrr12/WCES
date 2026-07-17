# WCES 项目审查报告

> **审查范围**：CSI 数据接收 → 数据处理 → 数据可视化完整通路 + ESP32-C5 固件全面审查
> **审查依据**：ESP32-C5 Datasheet v1.0、ESP-IDF v6.0 官方文档、项目硬约束（project_memory.md）、CLAUDE.md 架构规范
> **目标硬件**：ESP32-C5-DevKitC-1-N8R8（8 MB Flash + 8 MB PSRAM）
> **审查日期**：2026-07-16

---

## 目录

- [一、数据通路审查（15 项，已修复）](#一数据通路审查15-项已修复)
- [二、C5 固件二次审查（10 项）](#二c5-固件二次审查10-项)
- [三、N8R8 实际运行问题根因分析（6 项）](#三n8r8-实际运行问题根因分析6-项)
- [四、已确认正确实现的部分](#四已确认正确实现的部分)
- [五、修复状态总览](#五修复状态总览)

---

## 一、数据通路审查（15 项，已修复）

### CRITICAL-1 — 固件 biquad 滤波器采样率硬编码 20Hz 永不更新

- **严重度**：🔴 严重
- **位置**：`firmware/esp32-c5-csi-node/main/edge_processing.c:1048-1055`
- **现象**：Biquad 带通滤波器在 `edge_processing_init()` 中以硬编码 `fs = 20.0f` 设计一次，之后永不更新。当实际采样率（`s_measured_sample_rate`，动态测量）偏离 20Hz 时，呼吸（0.1-0.5 Hz）和心率（0.8-2.0 Hz）滤波器截止频率错误。
- **影响**：呼吸/心率医疗测量数据不准确。与 Rust 侧已修复的 BUG 49 同类，但固件侧未同步修复。
- **修复方案**：新增 `edge_redesign_biquads_if_needed()` 函数，采样率漂移 >10% 时重设计所有 biquad（含 per-person），保留 delay-line 状态避免信号不连续；init 不再设计，首帧统一通过该函数设计。
- **状态**：✅ 已修复

### HIGH-1 — UDP 接收缓冲区 2048 字节可能截断压缩帧

- **严重度**：🟠 高
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/tasks/udp_receiver.rs:61`
- **现象**：`let mut buf = [0u8; 2048]`，但压缩帧（magic `0xC5110003`）最大可达 2078 字节（10 字节头 + comp_len ≤ 2068）。超过 2048 的帧会被截断或丢弃。
- **影响**：压缩帧静默丢失，服务端无法接收边缘处理后的数据。
- **修复方案**：缓冲区 2048 → 4096 字节。
- **状态**：✅ 已修复

### HIGH-2 — WebSocket patient_id 解析失败导致 agent 链路断裂

- **严重度**：🟠 高
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/handlers/ws.rs:97`
- **现象**：`let patient_id_num: u32 = patient_id.parse().unwrap_or(1)` —— 幸存者 ID 是 "SURV-001" 格式（`mat_pipeline.rs:567`），解析为 u32 必然失败，默认回退到 1。所有 agent 分析请求丢失实际幸存者 ID。
- **影响**：LLM agent 无法关联具体幸存者，分析结果可能错配。
- **修复方案**：先查 `survivors.node_id`（u32），再回退 "SURV-NNN" 后缀解析。
- **状态**：✅ 已修复

### HIGH-3 — stream_sender 日志超时值与实际不符

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/main/stream_sender.c:56`
- **现象**：日志输出 `"send timeout 100 ms"`，但实际超时是 10ms（第 33 行 `.tv_usec = 10 * 1000`）。
- **影响**：调试时误导，可能基于错误日志做出错误判断。
- **修复方案**：日志 `"100 ms"` → `"10 ms"`。
- **状态**：✅ 已修复

### HIGH-4 — 默认跳表混合 2.4G/5G 频段

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/main/main.c:314`
- **现象**：`default_hop[] = {1, 6, 11, 36, 40, 44}` —— 默认跳表同时包含 2.4GHz（1/6/11）和 5GHz（36/40/44）信道。C5 关联在某一频段后，跳到另一频段会断连。
- **影响**：STA 模式下信道跳频与 AP 关联冲突，导致 WiFi 反复断连。
- **修复方案**：默认跳表改为 `{1, 6, 11}`（仅 2.4GHz），NVS 配置可覆盖跨频段。
- **状态**：✅ 已修复

### MEDIUM-1 — ch14 频率换算错误

- **严重度**：🟡 中
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/tasks/udp_receiver.rs:424`
- **现象**：`((frame.freq_mhz - 2407) / 5) as u8` 对 ch14（2484 MHz）计算得 15，实际应为 14。
- **影响**：ch14（日本专用）的帧信道号错误，影响信道相关分析。
- **修复方案**：ch14 频率换算特例处理。
- **状态**：✅ 已修复

### MEDIUM-2 — Vec::remove(0) 违反 VecDeque FIFO 约定

- **严重度**：🟡 中
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/mat_pipeline.rs:367, 371, 374`
- **现象**：`s.breathing_history.push(br); if s.breathing_history.len() > 30 { s.breathing_history.remove(0); }` —— `Vec::remove(0)` 是 O(n) 操作。项目内存约定明确要求「Use VecDeque instead of Vec for FIFO operations」。
- **影响**：性能损失（每次 remove(0) 移动所有元素）；违反项目工程约定。
- **修复方案**：Vec + remove(0) → VecDeque + pop_front()。
- **状态**：✅ 已修复

### MEDIUM-3 — 心率窗口 15s 与 CLAUDE.md 30s 不一致

- **严重度**：🟡 中
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/vitals_bridge.rs:38`
- **现象**：`HeartRateExtractor::new(n_sc, sample_rate.max(1.0), 15.0)` 使用 15 秒窗口，但 CLAUDE.md 文档说 "30s window"。
- **影响**：心率估算窗口过短，数据不足时估算不稳定；文档与代码不一致。
- **修复方案**：窗口 15s → 30s 对齐 CLAUDE.md。
- **状态**：✅ 已修复

### MEDIUM-4 — (0,0,0) 被当作"未初始化"位置标志

- **严重度**：🟡 中
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/mat_pipeline.rs:413`
- **现象**：`if s.position == (0.0, 0.0, 0.0)` —— 将原点 (0,0,0) 视为"未初始化"，但 (0,0,0) 是合法坐标（如节点正前方）。
- **影响**：位于原点的幸存者被误判为未定位，位置不更新。
- **修复方案**：用 `position_initialized` 布尔标志替代 (0,0,0) 判断。
- **状态**：✅ 已修复

### MEDIUM-5 — 位置置信度为 0 仍返回 Some

- **严重度**：🟡 中
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/mat_pipeline.rs:596`
- **现象**：`position: Some([s.position.0, ...])` 总是返回 Some，即使 `position_confidence == 0`。
- **影响**：UI 在无定位数据时仍在原点显示幸存者标记，误导操作员。
- **修复方案**：置信度 ≤0.01 或未初始化时返回 None。
- **状态**：✅ 已修复

### MEDIUM-6 — BSS 预算文档与实际不符

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/edge_processing.c:46-47`（原始）/ `edge_processing.h:39`
- **现象**：注释说 "s_ring ~33.1 KB (16 slots)"，但 `EDGE_RING_SLOTS=64`，实际 ~133 KB。
- **影响**：内存预算评估错误，可能低估 SRAM 压力。
- **修复方案**：注释更正为 64 槽/133 KB（后续二次审查进一步修正为 172 KB / 384 KB SRAM 44%）。
- **状态**：✅ 已修复（注：二次审查中进一步修正，详见 MEDIUM-11）

### MEDIUM-7 — display_task 失败返回 ESP_OK

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/display_task.c:118, 154`
- **现象**：分配失败或任务创建失败时返回 `ESP_OK`，误导调用者认为初始化成功。
- **影响**：显示子系统静默失败，难以诊断。
- **修复方案**：三处失败返回 `ESP_ERR_NOT_FOUND` / `ESP_ERR_NO_MEM` / `ESP_FAIL`。
- **状态**：✅ 已修复

### LOW-1 — z 轴位置魔法数字

- **严重度**：🟢 低
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/mat_pipeline.rs:411`
- **现象**：`raw_z = nz * 0.5` —— z 轴不依赖距离 d，仅用法向量 z 分量 ×0.5，魔法数字无解释。
- **影响**：z 轴高度估算不准确。
- **修复方案**：改为 `nz + d * 0.3`，加注释说明。
- **状态**：✅ 已修复

### LOW-2 — escapeHtml 注释过时

- **严重度**：🟢 低
- **位置**：`docs/triage-ui/js/triage-common.js:10-30`
- **现象**：注释说 "NOT safe for: attribute values that may contain user-controlled double quotes"，但函数实际上**确实编码引号**（' 和 "）。
- **影响**：维护者可能误以为需要额外转义。
- **修复方案**：注释更正，明确编码引号。
- **状态**：✅ 已修复

### LOW-3 — WebSocket 无鉴权

- **严重度**：🟢 低
- **位置**：`rust-server/crates/wifi-densepose-sensing-server/src/handlers/ws.rs`、`docs/triage-ui/triage.html`、`docs/triage-ui/triage-v1.html`
- **现象**：WebSocket 连接无任何鉴权，任何能访问服务端端口的客户端均可连接接收所有幸存者数据。
- **影响**：局域网内安全风险（虽可能是 intentionally local-only）。
- **修复方案**：可选 `WCES_WS_TOKEN` 环境变量鉴权，常数时间比较，前端支持 token 注入。未配置时完全向后兼容。
- **状态**：✅ 已修复

---

## 二、C5 固件二次审查（10 项）

基于 ESP32-C5 Datasheet v1.0、ESP-IDF v6.0 官方文档交叉验证。

### CRITICAL-2 — WASM runtime use-after-free 风险

- **严重度**：🔴 严重
- **位置**：`firmware/esp32-c5-csi-node/main/wasm_runtime.c:693-776`
- **现象**：`wasm_runtime_on_frame()` **不持 `s_mutex`** 直接遍历 `s_slots[]`，期间另一任务调用 `wasm_runtime_unload()` 会释放 `slot->runtime`，导致 `m3_CallV(slot->fn_on_frame, ...)` 访问已释放的 WASM3 runtime。
- **官方依据**：ESP-IDF FreeRTOS 是抢占式调度，DSP 任务（Core 1）与 HTTP 任务（Core 0）真正并行，无锁访问共享状态不安全。
- **影响**：DSP 任务崩溃或内存损坏。在 RVF 热加载场景下必然触发。
- **建议方案**：`on_frame` 内对每个 slot 用 `xSemaphoreTake(s_mutex, 0)` 非阻塞尝试锁，跳过被锁定（正在 unload）的 slot；或在 slot 内加 `volatile uint32_t ref_count`，on_frame 递增，unload 等待 ref_count==0。
- **状态**：⏳ 待修复

### HIGH-5 — Kconfig 错误声称 C5 支持 6GHz

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/main/Kconfig.projbuild:36-38`
- **现象**：
  ```
  config CSI_WIFI_CHANNEL
      int "WiFi Channel (1-13 for 2.4G, 36-177 for 5G, 1-233 for 6G on C5)"
      range 1 233
  ```
- **官方依据**：ESP32-C5 Datasheet v1.0 明确：「**2.4 & 5 GHz dual-band Wi-Fi 6**」，工作频率「2412~2484 MHz, 5180~5885 MHz」——**完全不支持 6GHz**。
- **影响**：用户配置信道 178-233 时会失败；误导用户认为 C5 是 6GHz 芯片。
- **建议方案**：改为 `range 1 177`，help 文本移除 6G 描述。
- **状态**：⏳ 待修复

### HIGH-6 — Kconfig GPIO 数量注释芯片/模组混淆

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/main/Kconfig.projbuild:102-103, 124, 131, 138, 145, 152, 159, 166, 173, 180`
- **现象**：第 102 行说 "C5 has only 22 GPIOs (0-21)"，但第 124-180 行多处说 "valid GPIO 0-28"。
- **官方依据**：
  - **ESP32-C5 芯片**：29 GPIOs（GPIO0-28），数据手册明确
  - **ESP32-C5-WROOM-1 模组**：仅引出 22 GPIOs（GPIO16-22 被 SPI flash/PSRAM 占用）
- **影响**：用户混淆芯片与模组能力，自相矛盾的注释导致困惑。
- **建议方案**：统一为「ESP32-C5 chip: 29 GPIOs (0-28); WROOM-1 module: 22 GPIOs available (16-22 reserved for flash/PSRAM)」。
- **状态**：⏳ 待修复

### MEDIUM-8 — 默认 QSPI D3 = GPIO7（Strapping 管脚）冲突

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/Kconfig.projbuild:154-159`
- **现象**：`DISPLAY_QSPI_D3` 默认 GPIO7。
- **官方依据**：ESP-IDF v6.0 GPIO 文档明确 GPIO7 是 **Strapping 管脚**，复位时电平决定 boot 模式。
- **影响**：QSPI 高速数据信号可能在 boot 期间被误读，导致 boot 模式错误；运行时 D3 信号也可能被 strapping 上下拉干扰。
- **建议方案**：默认改为 GPIO8（非 strapping，非 flash 占用）。
- **状态**：⏳ 待修复

### MEDIUM-9 — idf_component.yml 引入未使用的 CST816S 触屏组件

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/idf_component.yml:6-7`
- **现象**：声明依赖 `espressif/esp_lcd_touch_cst816s`，但 `display_hal.c:67` 实现的是 **FT3168** 触屏驱动，未使用 esp_lcd_touch 框架。
- **影响**：固件大小无谓增加（CST816S 驱动 ~3-5 KB），构建时间增加。
- **建议方案**：移除 idf_component.yml 中 esp_lcd_touch_cst816s 与 esp_lcd_touch 依赖。
- **状态**：⏳ 待修复

### MEDIUM-10 — Kconfig 与 display_hal 显示驱动芯片不一致

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/Kconfig.projbuild:95-97` vs `firmware/esp32-c5-csi-node/main/display_hal.c:3,8,225`
- **现象**：Kconfig 说 "RM67162 QSPI AMOLED"，display_hal.c 说 "SH8601 368x448"。
- **影响**：用户根据 Kconfig 文档寻找 datasheet 时困惑。
- **建议方案**：Kconfig 改为 "SH8601 QSPI AMOLED display"。
- **状态**：⏳ 待修复

### MEDIUM-11 — BSS 预算 SRAM 总量错误（修正 MEDIUM-6）

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/edge_processing.c:38-65`（MEDIUM-6 修复中的注释）
- **现象**：注释说 "C5 has 400 KB SRAM total" 和 "consume ~54% of app SRAM"。
- **官方依据**：ESP32-C5 Datasheet v1.0 明确 **HP SRAM: 384 KB**（不是 400 KB）。
- **影响**：预算百分比错误（54% 实际应为 133 KB / (384-80) KB ≈ 44%）。
- **建议方案**：修正 SRAM 总量为 384 KB，app SRAM 约 304 KB，s_ring 占 44%。
- **状态**：⏳ 待修复

### LOW-4 — mock_csi 跨频段信道扫描未模拟切换延迟

- **严重度**：🟢 低
- **位置**：`firmware/esp32-c5-csi-node/main/mock_csi.c:90, 385-398`
- **现象**：`s_sweep_channels[] = {1, 6, 11, 36}` 每 20 帧切换信道，但 mock 不模拟实际硬件的 `esp_wifi_set_channel` ~5-10ms 稳定时间。
- **影响**：QEMU 测试可能掩盖真实硬件上信道切换瞬间的 CSI 噪声。
- **建议方案**：mock 中在切换帧前注入 1-2 帧的低振幅噪声模拟切换瞬态。
- **状态**：⏳ 待修复

### LOW-6 — swarm_bridge heartbeat 中 presence 变量未使用

- **严重度**：🟢 低
- **位置**：`firmware/esp32-c5-csi-node/main/swarm_bridge.c:323`
- **现象**：`bool presence = vit_valid && (vit.flags & 0x01);` 定义后未在 heartbeat 中使用（heartbeat 总是发送）。
- **影响**：编译器 dead_code 告警；可能误导维护者认为 heartbeat 受 presence 控制。
- **建议方案**：移除变量，或在注释中说明「heartbeat 不受 presence 控制，always-on」。
- **状态**：⏳ 待修复

### LOW-7 — swarm_bridge 缩进错误

- **严重度**：🟢 低
- **位置**：`firmware/esp32-c5-csi-node/main/swarm_bridge.c:334, 354`
- **现象**：`if (swarm_post_json(...) == ESP_OK) {` 应缩进 16 空格但只有 8 空格。
- **影响**：可读性，无功能影响。
- **建议方案**：修复缩进。
- **状态**：⏳ 待修复

---

## 三、N8R8 实际运行问题根因分析（6 项）

针对用户反馈「连不上路由器」和「连上后发不出数据」两个实际问题的根因分析。

### 根因 1 — sdkconfig 与 sdkconfig.defaults 严重不一致（P0）

- **严重度**：🔴 最可能导致实际问题
- **位置**：`firmware/esp32-c5-csi-node/sdkconfig` vs `sdkconfig.defaults`
- **现象**：

| 配置项 | sdkconfig.defaults（apply-config 生成） | sdkconfig（实际构建用的） |
|--------|----------------------------------------|--------------------------|
| SSID | `"ORBI62"` | **`"SC"`** |
| target_ip | `"192.168.1.3"` | **`"10.172.111.195"`** |

- **原理**：ESP-IDF 配置优先级 `sdkconfig`（已存在）> `sdkconfig.defaults`。用户修改 `sdkconfig.defaults` 后若未执行 `idf.py fullclean`，**sdkconfig.defaults 的修改完全不生效**，固件仍用旧的 sdkconfig 编译。
- **影响**：
  - 固件编译时 SSID="SC" → 如果实际路由器是 "ORBI62"，**连不上**
  - 固件编译时 target_ip="10.172.111.195" → 如果服务端不在该 IP，**数据发到错误地址**
- **修复方案**：
  ```powershell
  .\apply-config.ps1 -NodeId 1
  cd firmware\esp32-c5-csi-node
  idf.py fullclean
  idf.py set-target esp32c5
  idf.py build
  idf.py -p COMx flash
  ```
- **状态**：⏳ 需用户在本地执行 fullclean 重建

### 根因 2 — WiFi BW40 与 5GHz 11ax AP 兼容性问题（P0）

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/main/main.c:222-226`
- **现象**：
  ```c
  wifi_bandwidths_t bandwidth = {
      .ghz_2g = WIFI_BW40,
      .ghz_5g = WIFI_BW40,   // C5 11ax 是 20MHz-only non-AP
  };
  ```
- **官方依据**：ESP32-C5 Datasheet v1.0 明确 11ax 模式下 STA 端为 **20 MHz-only non-AP mode**。
- **影响**：配置 BW40 后，11ax 模式下 PHY 自动降级 20MHz，但**协议协商层面 STA 仍声明 40MHz 能力**。Orbi 6（WiFi 6 Mesh）或严格 AP 可能因 STA 声明 40MHz 但实际不支持而**拒绝关联**。
- **修复方案**：
  ```c
  wifi_bandwidths_t bandwidth = {
      .ghz_2g = WIFI_BW20,
      .ghz_5g = WIFI_BW20,
  };
  ```
- **状态**：⏳ 待修复

### 根因 3 — PSRAM 未集成到 malloc，WiFi/lwIP 全挤在 SRAM（P0）

- **严重度**：🟠 高
- **位置**：`firmware/esp32-c5-csi-node/sdkconfig`（构建生成）
- **现象**：
  ```
  CONFIG_SPIRAM_USE_CAPS_ALLOC=y              # 只能 heap_caps_malloc 显式分配
  # CONFIG_SPIRAM_USE_MALLOC is not set       # 普通 malloc 不用 PSRAM
  # CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP is not set  # WiFi/lwIP 不用 PSRAM
  ```
- **内存预算**（PSRAM 未集成 malloc 时）：
  - HP SRAM: **384 KB**（官方 datasheet）
  - WiFi/BT/lwIP 协议栈：~80 KB（全在 SRAM）
  - `edge_processing.c` 静态 `s_ring`：~133 KB（SRAM）
  - 其他静态+堆：~50 KB
  - **剩余 ~121 KB** 给 WiFi 协议栈动态分配
- **影响**：5GHz 11ax 模式下 WiFi 协议栈需要更多缓冲区，121 KB 在连接高峰或 CSI burst 时可能不足，导致 `esp_wifi_connect()` 内部分配失败 → 连不上，或 `sendto()` 的 lwIP pbuf 分配失败 → 发不出数据。
- **修复方案**：在 sdkconfig.defaults 添加：
  ```
  CONFIG_SPIRAM_USE_MALLOC=y
  CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP=y
  CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=16384
  ```
- **状态**：⏳ 待修复

### 根因 4 — CSI 无主动探测，依赖被动 beacon（P1）

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/main.c`（缺失 NDP 调用）
- **现象**：`csi_inject_ndp_frame()` 已在 `csi_collector.c:698` 实现，但 **main.c 中从未调用**。
- **影响**：STA 模式下 CSI callback 只对接收到的帧触发。5GHz 11ax 模式下 beacon 间隔 ~102ms → CSI 约 10 Hz。如果 AP 流量稀少，CSI 数据量极少，服务端可能认为"没数据"。
- **修复方案**：在 main.c 中添加定时器，每 100ms 调用 `csi_inject_ndp_frame()` 主动触发 CSI。
- **状态**：⏳ 待修复

### 根因 5 — stream_sender 10ms 超时在 SRAM 紧张时丢包（P1）

- **严重度**：🟡 中
- **位置**：`firmware/esp32-c5-csi-node/main/stream_sender.c:33`
- **现象**：`.tv_usec = 10 * 1000`（10ms 超时）
- **影响**：SRAM 紧张时 lwIP `sendto()` 的 pbuf 分配可能阻塞，10ms 超时后返回 `EAGAIN`，帧被丢弃。
- **修复方案**：超时从 10ms 改为 50ms，给 lwIP 更多时间完成 pbuf 分配。
- **状态**：⏳ 待修复

### 根因 6 — 信道跳频配置风险（P2 预防性）

- **严重度**：🟡 中（当前未激活，潜在风险）
- **位置**：`wces.config.toml:107-113`
- **现象**：
  ```toml
  [firmware.hop]
  enabled = true
  channels = [1, 6, 11, 36, 40, 44]  # 混合 2.4G + 5G
  ```
- **现状**：`provision.py` **不写入** `hop_count`/`channel_list` 到 NVS，`nvs_config.c` 默认 `hop_count=1`（单信道），`csi_hop_next_channel()` 是 no-op。**当前不会触发**。
- **风险**：如果用户手动写入 NVS 跳表，STA 模式下 `esp_wifi_set_channel()` 会强制离开 AP 信道，导致 WiFi 反复断连。
- **修复方案**：`wces.config.toml` 跳表改为 `channels = [36, 40, 44]`（仅 5GHz，与 AP 同频段），或 `enabled = false`。
- **状态**：⏳ 待修复

---

## 四、已确认正确实现的部分

### 数据通路

| 项目 | 验证依据 | 代码位置 |
|------|----------|----------|
| ADR-018 二进制帧解析 + 边界检查 | MAX_SUBCARRIERS=512, MAX_ANTENNAS=4 | `rust-server/.../esp32_parser.rs` |
| 子载波索引正确跳过 DC | half = n/2, indices -half..-1, 1..half+1 | `esp32_parser.rs:157-169` |
| 两阶段锁模式 | 写锁最小化，纯计算在锁外 | `udp_receiver.rs:124-400, 541-625` |
| 动态采样率 EMA 测量 | α=0.15 | `udp_receiver.rs:148-158` |
| WhōFi + phase-doppler 混合定位 | 60% + 40% | `udp_receiver.rs:560-590` |
| TriageLevel 枚举顺序 | Unknown=0, Minor=1, Delayed=2, Immediate=3, Deceased=4 | `triage.rs`, `mat_pipeline.rs:162-175` |
| START 分诊协议 | br None + hr None → Unknown | `mat_pipeline.rs:226-270` |
| WebSocket 指数退避 | 2s, 4s, 8s, 16s, 30s cap, max 10 retries | `triage.html:1271, 583` |
| escapeHtml XSS 防护 | textContent → innerHTML + 额外编码 ' 和 " | `triage-common.js:23-29` |
| Agent 并发限制 + 超时 | Semaphore(4) + 30s timeout | `udp_receiver.rs:43, 502` |

### C5 固件

| 项目 | 官方依据 | 代码位置 |
|------|----------|----------|
| HE20 242-tone CSI（C5 11ax 20MHz-only） | Datasheet: "20 MHz-only non-AP mode" | `csi_collector.c:482` `acquire_csi_su=true` |
| `acquire_csi_force_lltf=false`（自动选择最佳 LTF） | WiFi Vendor Features 文档 | `csi_collector.c:486` |
| WIFI_BAND_MODE_AUTO（双频自动） | Datasheet: "2.4 & 5 GHz dual band" | `main.c:213` |
| 11B/G/N/AX on 2.4G + 11N/AX on 5G | Datasheet 协议支持列表 | `main.c:215-216` |
| ch14 频率 2484 MHz 特例 | Datasheet: 2412-2484 MHz | `csi_collector.c:192-219` |
| SPSC 环 `__sync_synchronize()` 内存屏障 | FreeRTOS 双核 SPSC 模式 | `edge_processing.c:64-101`, `csi_collector.c:81-107` |
| edge_vitals_pkt_t `__attribute__((packed))` | 32 字节对齐 UDP 传输 | `edge_processing.h:97` |
| OTA PSK 认证 + 大小验证 | 安全默认 | `ota_update.c:80, 89` |
| WiFi 重连指数退避 | 1s→16s cap, max 10 retries | `main.c:119-120` |
| RVF HMAC-SHA256 常数时间比较 | 防时序攻击 | `rvf_parser.c:250-253` |

### 单元测试

| 测试套件 | 结果 |
|----------|------|
| wifi-densepose-core | 28 passed |
| wifi-densepose-hardware | 106 passed |
| wifi-densepose-llm | 52 passed |
| wifi-densepose-mat | 162 passed |
| wifi-densepose-nn | 24 passed |
| wifi-densepose-sensing-server | 213 passed |
| wifi-densepose-signal | 363 passed (1 ignored) |
| wifi-densepose-vitals | 52 passed |
| sensing_server bin | 56 passed |
| integration tests | 15 passed |
| **合计** | **1058+ passed, 0 failed** |

---

## 五、修复状态总览

### 按严重度统计

| 严重度 | 总数 | 已修复 | 待修复 |
|--------|------|--------|--------|
| 🔴 CRITICAL | 2 | 1 | 1（CRITICAL-2 WASM use-after-free） |
| 🟠 HIGH | 6 | 4 | 2（HIGH-5/6 Kconfig 文档） |
| 🟡 MEDIUM | 11 | 7 | 4（MEDIUM-8/9/10/11） |
| 🟢 LOW | 5 | 3 | 2（LOW-4/6/7） |
| **合计** | **31** | **19** | **12** |

### 按模块统计

| 模块 | 已修复 | 待修复 |
|------|--------|--------|
| 固件 - edge_processing | 2 | 1（MEDIUM-11 SRAM 注释修正） |
| 固件 - stream_sender | 1 | 1（根因 5 超时调整） |
| 固件 - main.c | 1 | 2（根因 2 BW20, 根因 4 NDP 注入） |
| 固件 - display_task | 1 | 0 |
| 固件 - wasm_runtime | 0 | 1（CRITICAL-2） |
| 固件 - Kconfig | 0 | 3（HIGH-5/6, MEDIUM-8/10） |
| 固件 - idf_component | 0 | 1（MEDIUM-9） |
| 固件 - mock_csi | 0 | 1（LOW-4） |
| 固件 - swarm_bridge | 0 | 2（LOW-6/7） |
| 固件 - sdkconfig | 0 | 2（根因 1 fullclean, 根因 3 PSRAM malloc） |
| Rust - udp_receiver | 2 | 0 |
| Rust - ws.rs | 2 | 0 |
| Rust - mat_pipeline | 4 | 0 |
| Rust - vitals_bridge | 1 | 0 |
| 前端 - triage-ui | 2 | 0 |
| 配置 - wces.config.toml | 0 | 1（根因 6 跳表） |
| **合计** | **19** | **12** |

### 待修复项优先级

**P0（立即修复，最可能解决实际运行问题）**：
1. 根因 1 — sdkconfig fullclean 重建（用户本地执行）
2. 根因 2 — WiFi BW40 → BW20
3. 根因 3 — PSRAM 集成到 malloc

**P1（本迭代修复）**：
4. CRITICAL-2 — WASM runtime use-after-free
5. 根因 4 — 定期注入 NDP
6. 根因 5 — stream_sender 超时 10ms → 50ms

**P2（下迭代修复）**：
7. HIGH-5 — Kconfig 移除 6G 描述
8. HIGH-6 — Kconfig GPIO 描述统一
9. MEDIUM-8 — QSPI D3 从 GPIO7 改 GPIO8
10. MEDIUM-9 — 移除未使用的 CST816S 依赖
11. MEDIUM-10 — Kconfig RM67162 → SH8601
12. MEDIUM-11 — SRAM 总量 400KB → 384KB

**P3（机会修复）**：
13. 根因 6 — wces.config.toml 跳表
14. LOW-4 — mock 信道延迟
15. LOW-6 — swarm presence 死代码
16. LOW-7 — swarm 缩进

---

## 附录：项目硬约束符合性检查

| 约束 | 状态 | 验证位置 |
|------|------|----------|
| TriageLevel 枚举顺序 Unknown=0...Deceased=4 | ✅ 符合 | `triage.rs`, `mat_pipeline.rs:162-175` |
| SEG_CRYPTO=0x0E, SEG_EMBED=0x0C 唯一字节 | ✅ 符合 | `rvf_container.rs:41`, `rvf_pipeline.rs:19` |
| WebSocket 指数退避最多 10 次重试 | ✅ 符合 | `triage.html:583` WS_MAX_RECONNECT=10 |
| HTML 用户输入必须 escapeHtml | ✅ 符合 | `triage-common.js:23-29` |
| VecDeque 用于 FIFO 操作 | ⚠️ 已修复 | `mat_pipeline.rs` Vec→VecDeque（MEDIUM-2） |
| unwrap()/expect() 替换为 Result | ⚠️ 部分 | `ws.rs:97` 原有 unwrap_or(1) 已修复（HIGH-2） |
| CDN 引用替换为本地文件 | ✅ 符合 | 前端 HTML 检查无 CDN 引用 |
| 患者 PII 脱敏后发送 LLM | ✅ 符合 | `udp_receiver.rs` agent 路径使用 node_id |
| LLM prompt 字段 XML 标签包裹 | ✅ 符合 | agent 上下文构造路径 |

---

*报告结束*
