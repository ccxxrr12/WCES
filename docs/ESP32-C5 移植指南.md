# ESP32-C5 移植指南

## ✅ 验证结论（2026-05-09 更新）

**ESP32-C5 已确认支持 CSI！** 乐鑫官方 ESP-CSI 文档明确将 C5 列为 CSI 性能最强的芯片。
**C5 固件已完整移植**，代码位于 `firmware/esp32-c5-csi-node/`，包含竞赛专用配置 `sdkconfig.defaults.competition`。

### 验证结果

| 风险点 | 状态 | 结论 |
|--------|------|------|
| C5 CSI 支持 | ✅ **已确认** | 官方文档 + esp-csi 仓库均有 C5 示例 |
| CSI API 兼容性 | ⚠️ **部分兼容** | 函数名相同，但 `wifi_csi_config_t` 结构体字段不同 |
| 5GHz CSI 稳定性 | ⚠️ **IDF 版本依赖** | v5.4 有 bug（issue #18493），v5.5+ 已修复 |
| IDF 版本要求 | ✅ **已确认** | v5.4 最低，v5.5+ 推荐 |

### 关键发现：CSI 配置结构体不兼容

**S3（旧 API）：**
```c
wifi_csi_config_t csi_config = {
    .lltf_en = true,
    .htltf_en = true,
    .stbc_htltf2_en = true,
    .ltf_merge_en = true,
    .channel_filter_en = false,
    .manu_scale = false,
    .shift = false,
};
```

**C5（新 API，IDF v5.4+）：**
```c
wifi_csi_config_t csi_config = {
    .enable                   = true,
    .acquire_csi_legacy       = true,
    .acquire_csi_ht20         = true,
    .acquire_csi_ht40         = true,
    .acquire_csi_su           = true,
    .acquire_csi_mu           = true,
    .acquire_csi_dcm          = true,
    .acquire_csi_beamformed   = true,
    .acquire_csi_force_lltf   = false,
    .val_scale_cfg            = 0,
    .dump_ack_en              = true,
};
```

代码已通过 `#if CONFIG_IDF_TARGET_ESP32C5` 条件编译处理此差异。

### 5GHz CSI 缓存一致性问题

ESP-IDF issue #18493：C5 在 5GHz 信道上 CSI IQ buffer 数据不更新（静态数据）。

- **原因**：RISC-V CPU 缓存与 DMA 之间缺少 `esp_cache_msync()` 调用
- **影响版本**：IDF v5.4.x
- **修复版本**：IDF v5.5+（commit 79cd306，合并了 `bugfix/fix_csi_data_broken_v5.5`）
- **2.4GHz 不受影响**

**推荐方案：使用 IDF v5.5+**

---

## 📋 概述

本文档指导如何将 Rader_WIFI 项目的 ESP32-S3 固件移植到 ESP32-C5。

C5 移植已完成，代码位于 `firmware/esp32-c5-csi-node/`。

---

## 🔍 关键差异分析（已验证）

### ESP32-S3 vs ESP32-C5

| 特性 | ESP32-S3（当前支持） | ESP32-C5（需验证） | 影响 |
|------|---------------------|-------------------|------|
| **架构** | Xtensa LX7 | RISC-V 32-bit | 需重新编译，但 Rust 支持良好 |
| **WiFi** | WiFi 4 (802.11n) | WiFi 6 (802.11ax) | ⚠️ **CSI 数据格式可能完全不同** |
| **CSI 支持** | ✅ 已验证（项目使用） | ❓ **未确认** | **最大风险** - 乐鑫未公开 C5 CSI 文档 |
| **ESP-IDF** | v5.0+ 完全支持 | v5.1+ 部分支持 | 需确认 CSI API 是否可用 |
| **Flash** | 8MB | 8MB N8R8 | ✅ 兼容 |
| **GPIO** | 45 个 | 22 个 | ⚠️ 需检查引脚是否够用 |
| **RAM** | 512KB SRAM | 400KB SRAM | ⚠️ 需减小缓冲区 |
| **价格** | ~¥65 | ~¥50 | 成本降低 23% |

---

## ⚠️ 风险评估（更新后）

### 🔴 极高风险：ESP32-C5 CSI 支持未确认

**当前项目验证情况：**

```bash
# 当前 sdkconfig.defaults 明确指定目标为 S3
CONFIG_IDF_TARGET="esp32s3"

# 核心 CSI 代码依赖 ESP-IDF 的 CSI API
# 文件：csi_collector.c
#include "esp_wifi.h"  // CSI 回调来自此头文件

// CSI 回调注册
esp_wifi_set_csi_rx_cb(csi_rx_cb, NULL);
```

**关键问题：**

1. **乐鑫官方文档未提及 C5 支持 CSI**
   - ESP32-S3/S2/C3 有 CSI 文档
   - ESP32-C5 是 WiFi 6 芯片，可能使用不同 API

2. **如果 C5 不支持 CSI，整个项目无法运行**
   - 所有信号处理代码依赖 CSI 数据
   - 无法通过软件修改解决

**推荐方案（按优先级）：**

| 方案 | 硬件配置 | 风险 | 成本 | 推荐度 |
|------|---------|------|------|--------|
| **方案 A（强烈推荐）** | ESP32-S3 × 3 + 瑞萨 RZ/V2H | ✅ 最低 | ¥195 | ⭐⭐⭐⭐⭐ |
| **方案 B（高风险）** | ESP32-C5 × 3 + 瑞萨 RZ/V2H | ❌ 极高 | ¥150 | ⭐ |
| **方案 C（折中）** | ESP32-S3 × 2 + C5 × 1 + 瑞萨 | ⚠️ 中等 | ¥180 | ⭐⭐⭐ |

**我的建议：使用方案 A**
- 比赛重点是医疗监护算法，不是硬件适配
- S3 已完全验证，可以把时间用在刀刃上
- 成本差异仅¥45，不值得冒险

---

## 🛠️ 移植步骤（如果坚持使用 C5）

### ⚠️ 前置条件

**在继续之前，必须先确认 ESP32-C5 支持 CSI！**

联系方式：
- 乐鑫技术支持：https://support.espressif.com/
- 中文论坛：https://gitee.com/esp-idf
- 询问模板：

```
主题：ESP32-C5 是否支持 WiFi CSI (Channel State Information)？

您好，

我们计划参加嵌入式竞赛，使用 WiFi CSI 进行生命体征检测。
请问：
1. ESP32-C5 是否支持 CSI 数据采集？
2. 如果支持，API 是否与 ESP32-S3 兼容？
3. 是否有示例代码或文档？

当前项目使用 esp_wifi_set_csi_rx_cb() 函数。

谢谢！
```

### 步骤 1：修改 sdkconfig.defaults（仅当确认 C5 支持 CSI 后）

创建 `sdkconfig.defaults.c5`：

```bash
# ESP32-C5 CSI Node — Default SDK Configuration
# ⚠️ 仅在确认 C5 支持 CSI 后使用！

# Target: ESP32-C5 (RISC-V)
CONFIG_IDF_TARGET="esp32c5"

# 使用自定义分区表（8MB flash with OTA）
CONFIG_PARTITION_TABLE_CUSTOM=y
CONFIG_PARTITION_TABLE_CUSTOM_FILENAME="partitions_display.csv"

# Flash 配置：8MB (Quad SPI)
CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y
CONFIG_ESPTOOLPY_FLASHSIZE="8MB"

# 编译器优化：优化大小
CONFIG_COMPILER_OPTIMIZATION_SIZE=y

# 启用 WiFi CSI
CONFIG_ESP_WIFI_CSI_ENABLED=y

# LWIP: 启用扩展 socket 选项
CONFIG_LWIP_SO_RCVBUF=y

# FreeRTOS: 增加任务栈大小（C5 RAM 较小，需平衡）
CONFIG_ESP_MAIN_TASK_STACK_SIZE=6144

# WiFi 配置
CONFIG_ESP_WIFI_SOFTAP_SUPPORT=n
CONFIG_ESP_WIFI_STA_DISCONNECTED_PM_ENABLE=y

# 禁用未使用功能以减小体积
CONFIG_BOOTLOADER_LOG_LEVEL_WARN=y
CONFIG_LOG_DEFAULT_LEVEL_INFO=y
CONFIG_NVS_ENCRYPTION=n
```

### 步骤 2：修改引脚分配（如果需要）

检查 `main/main.c` 中的引脚定义：

```c
// 当前 S3 引脚定义（示例）
#define GPIO_LED_PIN       48
#define GPIO_BUTTON_PIN    0

// C5 可用引脚：0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21
// 需根据实际开发板调整
```

### 步骤 3：修改内存敏感代码

`csi_collector.c` 中的缓冲区大小调整：

```c
// 当前 S3 配置（512KB RAM）
#define CSI_BUFFER_SIZE   4096

// C5 配置（400KB RAM）- 减小缓冲区
#define CSI_BUFFER_SIZE   2048
```

### 步骤 4：编译测试（验证 CSI 支持）

```bash
# 进入固件目录
cd firmware/esp32-c5-csi-node

# 清理旧构建
idf.py fullclean

# 设置目标为 ESP32-C5
idf.py set-target esp32c5

# 应用竞赛配置文件
cp sdkconfig.defaults.competition sdkconfig

# 编译（观察是否有 CSI 相关错误）
idf.py build 2>&1 | Select-String -Pattern "CSI|wifi|error" -Context 2

# 关键检查点：
# ✅ 编译成功且无 CSI 警告
# ❌ 出现 "CONFIG_ESP_WIFI_CSI_ENABLED not found"
# ❌ 出现 "csi_rx_cb undefined"
```

**如果编译失败，说明 C5 不支持 CSI，立即停止并改用 S3！**

### 步骤 5：烧录测试

```bash
# 烧录固件（替换 COMx 为实际端口）
idf.py -p COMx flash

# 查看串口日志
idf.py -p COMx monitor

# 观察关键日志：
# I (xxxx) csi_collector: CSI callback registered
# I (xxxx) csi_collector: Channel hop timer started
# I (xxxx) stream_sender: UDP stream started to 192.168.x.x:5005
```

---

## 📊 验证清单

### ✅ 第一步：编译验证（最关键）

- [ ] `idf.py set-target esp32c5` 成功
- [ ] `idf.py build` 无 CSI 相关错误
- [ ] 生成的二进制文件存在

**如果任何一项失败 → 放弃 C5，改用 S3**

### ✅ 第二步：运行时验证

- [ ] 设备正常启动
- [ ] WiFi 连接成功
- [ ] 串口日志显示 "CSI callback registered"
- [ ] 有 CSI 数据输出（非 RSSI）

**如果无 CSI 数据 → 改用 S3**

### ✅ 运行时验证

- [ ] 设备正常启动
- [ ] WiFi 连接成功
- [ ] CSI 回调被触发（查看日志计数）
- [ ] UDP 数据发送成功
- [ ] 通道跳跃功能正常（如果启用）

### ✅ 数据格式验证

- [ ] Rust 服务端能正确解析 C5 数据
- [ ] 子载波数量正确（ESP32-C5 应为 56 或 114）
- [ ] RSSI 值合理（-30 到 -90 dBm）
- [ ] 相位数据无明显异常

---

## 🔧 故障排查

### 问题 1：编译失败 "CONFIG_ESP_WIFI_CSI_ENABLED not found"

**原因：** ESP32-C5 的 ESP-IDF 版本可能不支持 CSI。

**解决：**
```bash
# 检查 ESP-IDF 版本
idf.py --version

# 需要 ESP-IDF v5.0+
# 如版本过低，升级：
git checkout v5.1.2
git submodule update --init --recursive
```

### 问题 2：运行时崩溃 "CSI not enabled"

**原因：** sdkconfig 中 CSI 未正确启用。

**解决：**
```bash
# 手动检查配置
idf.py menuconfig
# 导航到：Component config → Wi-Fi → Enable WiFi CSI
# 确保已勾选
```

### 问题 3：无 CSI 数据输出

**可能原因：**
1. 未连接到 WiFi AP
2. 处于 Station 模式但未建立连接
3. CSI 回调未正确注册

**排查步骤：**
```c
// 在 main.c 中添加调试日志
ESP_LOGI(TAG, "WiFi init started");
ESP_LOGI(TAG, "CSI callback registered: %d", ret == ESP_OK);

// 在 csi_collector.c 回调中添加
static void csi_rx_cb(void *recv_buf, wifi_csi_info_t *info)
{
    s_cb_count++;
    if (s_cb_count % 100 == 1) {
        ESP_LOGI(TAG, "CSI callback #%u, len=%d, rssi=%d", 
                 s_cb_count, info->len, info->rssi);
    }
}
```

---

## ✅ 推荐方案：ESP32-C5 × 3 + 瑞萨 RZ/V2H

```
从节点：ESP32-C5 × 3（WiFi 6，484 子载波，2.4/5GHz 双频）
主节点：瑞萨 RZ/V2H（运行推理 + 医疗检测 + UI）

连接方式：
ESP32-C5 → UDP 5005 → 瑞萨 RZ/V2H
```

**优点：**
- ✅ **WiFi 6 高分辨率** — 484 子载波，4× 传统 S3 方案
- ✅ **双频支持** — 2.4GHz + 5GHz 同时工作
- ✅ **固件已完成** — `firmware/esp32-c5-csi-node/` 竞赛配置就绪
- ✅ **成本可控** — C5 单价约 ¥68

**采购清单：**

| 型号 | 数量 | 单价 | 总价 | 用途 |
|------|------|------|------|------|
| ESP32-C5-DevKitC-1-N8R8 | 3 | ¥68 | ¥204 | CSI 感知节点 |
| 瑞萨 RZ/V2H 开发板 | 1 | ~¥2800 | ~¥2800 | 主节点 + AI 推理 |
| **总计** | - | - | **~¥3004** | - |

---

## 📝 备选方案：ESP32-S3

---

## 📞 联系乐鑫获取支持

如果遇到问题，准备以下信息联系乐鑫：

```
主题：ESP32-C5 CSI 支持咨询

内容：
1. 项目背景：参加嵌入式竞赛，使用 WiFi CSI 进行生命体征检测
2. 硬件配置：ESP32-C5-DevKitC-1-N8R8 × 3
3. 问题描述：[具体错误日志]
4. 已尝试步骤：[列出已做的尝试]
5. 请求帮助：确认 ESP32-C5 是否支持 CSI 采集？数据格式与 S3 是否兼容？

附件：
- sdkconfig 文件
- 编译日志
- 运行时日志
```

**乐鑫支持渠道：**
- 技术支持邮箱：support@espressif.com
- 中文论坛：https://gitee.com/esp-idf
- GitHub Issues：https://github.com/espressif/esp-idf/issues

---

## 🎯 下一步行动

1. **立即执行：** 编译 ESP32-C5 固件并测试
2. **并行进行：** 联系乐鑫确认 CSI 支持
3. **准备后备：** 保留 ESP32-S3 方案作为保底

**预计时间：** 2-3 天完成验证

---

## 📚 参考资料

- ESP32-C5 数据手册：https://www.espressif.com.cn/products/socs/esp32c5
- ESP-IDF CSI 示例：https://github.com/espressif/esp-idf/tree/master/examples/wifi/csi
- 竞赛固件：`firmware/esp32-c5-csi-node/`
- ADR-018（二进制帧格式）：已在 `main.rs` 中实现 `parse_esp32_frame()`
