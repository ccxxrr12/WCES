# ESP32-C5 CSI Node Firmware

基于 ESP32-S3 固件移植到 **ESP32-C5 (RISC-V, WiFi 6)** 的 CSI 感知节点。

> ⚠️ **ESP-IDF v5.5+ 强烈推荐**。v5.4 有已知的 C5 5GHz CSI bug (IQ buffer 静态数据，issue #18493)，已在 v5.5 修复（commit 79cd306）。

## 与 S3 版本的关键差异

| 特性 | ESP32-S3 | ESP32-C5 |
|------|----------|----------|
| 架构 | Xtensa LX7 | RISC-V 32-bit |
| WiFi | WiFi 4 (802.11n) | **WiFi 6 (802.11ax)** |
| CSI 性能 | C3 ≈ S3 | **C5 > C6 > C3 ≈ S3** (官方排名) |
| 子载波数 (20MHz) | 56 | 242 (HE) |
| 子载波数 (40MHz) | 114 | 484 (HE) |
| CSI 配置结构体 | `.lltf_en` / `.htltf_en` 等 (旧 API) | `.acquire_csi_*` (新 API, IDF v5.4+) |
| SRAM | 512KB | 400KB |
| GPIO | 45 | 22 |
| 频段 | 2.4GHz | 2.4GHz + 5GHz |
| 工具链 | xtensa-esp-elf | riscv32-esp-elf |
| ESP-IDF 最低版本 | v5.0 | **v5.4+** (推荐 v5.5+) |

## 构建

```powershell
# 设置 ESP-IDF v5.5+ 环境
. C:\Espressif\frameworks\esp-idf-v5.5\export.ps1

# 进入目录
cd firmware\esp32-c5-csi-node

# 设置目标并构建
idf.py set-target esp32c5
idf.py build

# 烧录
idf.py -p COMx flash
```

或使用一键脚本（需先修改脚本中的路径）:
```powershell
.\build_firmware_c5.ps1
```

## 已验证的风险点

| # | 风险 | 状态 | 结论 |
|---|------|------|------|
| 1 | WiFi 6 Kconfig 选项 | ✅ 已修复 | WiFi 6 对 C5 target 隐式启用，无需额外 Kconfig |
| 2 | C5 target 支持 | ✅ 已确认 | IDF v5.4+ 完全支持 C5 (v5.3.5 部分支持) |
| 3 | 5GHz CSI IQ 静态 bug | ⚠️ 已知 | IDF v5.4 有 bug（issue #18493），**v5.5+ 已修复**（commit 79cd306）需 `esp_cache_msync()` 刷新 RISC-V DMA 缓存 |
| 4 | **CSI 配置结构体不兼容** | ✅ 已修复 | C5 用 `.acquire_csi_*` 字段（新 API），S3 用 `.lltf_en` 等（旧 API），已添加 `#if CONFIG_IDF_TARGET_ESP32C5` 条件编译 |

## CSI API 兼容性说明

ESP32-C5 使用与 S3 **相同的函数名**（`esp_wifi_set_csi_rx_cb`、`esp_wifi_set_csi_config`、`esp_wifi_set_csi`），但 **`wifi_csi_config_t` 结构体字段不同**：

| 芯片 | 结构体字段 |
|------|-----------|
| S3/C3/ESP32 | `.lltf_en`, `.htltf_en`, `.stbc_htltf2_en`, `.ltf_merge_en`, `.channel_filter_en`, `.manu_scale`, `.shift` |
| C5/C6/C61 | `.enable`, `.acquire_csi_legacy`, `.acquire_csi_ht20`, `.acquire_csi_ht40`, `.acquire_csi_su`, `.acquire_csi_mu`, `.acquire_csi_dcm`, `.acquire_csi_beamformed`, `.acquire_csi_force_lltf`, `.val_scale_cfg`, `.dump_ack_en` |

代码已通过 `#if CONFIG_IDF_TARGET_ESP32C5` 条件编译自动选择正确的结构体。

## 注意事项

1. **ESP-IDF 版本**: 推荐 v5.5+（v5.4 的 5GHz CSI 有缓存一致性问题，2.4GHz 不受影响）
2. **工具链**: 使用 riscv32-esp-elf（不是 xtensa-esp-elf）
3. **显示**: 默认禁用（C5 GPIO 引脚不足），如需启用需重配引脚
4. **内存**: 主任务栈 7168 字节（比 S3 的 8192 略小）
5. **帧缓冲**: `CSI_MAX_FRAME_SIZE` 增大至 4116 字节（适配 WiFi 6 484 子载波）

## 参考

- [ESP-CSI 方案介绍](https://docs.espressif.com/projects/esp-techpedia/zh_CN/latest/esp-friends/solution-introduction/esp-csi/esp-csi-solution.html)
- [ESP-CSI GitHub](https://github.com/espressif/esp-csi) — csi_recv 示例 (C5 CSI 配置参考)
- [Issue #18493](https://github.com/espressif/esp-idf/issues/18493) — C5 5GHz CSI bug（已在 v5.5 修复）
- [Commit 79cd306](https://github.com/espressif/esp-idf/commit/79cd306) — 修复：esp_cache_msync()
