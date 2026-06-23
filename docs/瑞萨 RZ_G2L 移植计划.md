# 瑞萨 RZ/G2L 移植计划

## 项目概述

将 WCES 项目的 Rust 推理服务移植到瑞萨 RZ/G2L ARM 平台。RZ/G2L 无 DRP-AI 加速器，所有推理在 CPU（Cortex-A55 ×2）上执行。

---

## 硬件规格

### 瑞萨 RZ/G2L (R9A07G044L)

| 规格 | 参数 |
|------|------|
| **CPU** | ARM Cortex-A55 ×2 (1.2 GHz) + Cortex-M33 (200 MHz) |
| **AI 加速器** | 无 (CPU-only 推理) |
| **RAM** | 1GB DDR4 |
| **存储** | microSD / QSPI NOR Flash (64MB) |
| **操作系统** | Linux (Yocto/CIP Kernel) |
| **GPU** | Mali-G31 |
| **网络** | 双千兆以太网 + 板载 WiFi/BT (Laird 802.11ac) |
| **接口** | USB Type-C 供电, HDMI, MIPI-DSI, MIPI-CSI, 40-pin GPIO |
| **板型** | Raspberry Pi 兼容 (82×50mm) |

**对比 RZ/V2H 的关键变化：**
| 方面 | RZ/V2H | RZ/G2L | 影响 |
|------|--------|--------|------|
| CPU 核心 | 4×A55 | 2×A55 + M33 | LLM 推理 ~2× 慢 |
| RAM | 4-8GB | 1GB | LLM 模式内存紧张 |
| DRP-AI | 有 | 无 | 全部 CPU 推理 |
| BSP | VLP 3.x | VLP 3.0.5+ (CIP Kernel) | Yocto 构建方式相同 |
| 价格 | ~¥2800 | ~¥600 | 成本大幅降低 |

---

## 软件架构设计

### 目标架构

```
┌─────────────────────────────────────────────────────┐
│                瑞萨 RZ/G2L SBC                       │
├─────────────────────────────────────────────────────┤
│                                                     │
│ ┌─────────────────────────────────────────────┐    │
│ │  sensing-server (Rust aarch64)              │    │
│ │  ├─ CSI 数据接收 (UDP 5005)                 │    │
│ │  ├─ 信号处理 (FFT/特征提取)                  │    │
│ │  ├─ START 分诊 + 伤员追踪                   │    │
│ │  ├─ 19 边缘模块引擎 (原生 FPU)               │    │
│ │  ├─ LLM 分析引擎 (可选, CPU-only)            │    │
│ │  └─ HTTP + WebSocket 服务                   │    │
│ └─────────────────────────────────────────────┘    │
│                       │                             │
│ ┌─────────────────────────────────────────────┐    │
│ │  Web UI (静态文件)                          │    │
│ │  ├─ 分诊仪表盘 (triage.html)               │    │
│ │  ├─ 3D 可视化                               │    │
│ │  └─ 观测台                                   │    │
│ └─────────────────────────────────────────────┘    │
│                                                     │
└─────────────────────────────────────────────────────┘
         │
         │ 千兆以太网 (板载双网口)
         │
┌─────────────────────────────────────────────────────┐
│       ESP32-C5 × 3 (CSI 感知节点)                   │
└─────────────────────────────────────────────────────┘
```

---

## 移植步骤

### 阶段 1：环境搭建（3-5 天）

#### 1.1 获取 RZ/G2L BSP

```bash
# 下载 RZ/G2L BSP (Verified Linux Package)
# 访问：https://www.renesas.com/rzg2l-bsp
# 参考：RZ/G2L SBC 快速入门指南
# https://www.renesas.cn/zh/document/qsg/rzg2l-sbc-single-board-computer-quick-start-guide

# 烧录 Yocto 镜像到 microSD
# 插入 microSD → USB-C 供电 → 启动
```

#### 1.2 安装交叉编译工具链

```bash
# 安装 ARM64 交叉编译器
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# 添加 Rust ARM64 目标
rustup target add aarch64-unknown-linux-gnu

# 配置 .cargo/config.toml
cat >> .cargo/config.toml << EOF
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF
```

#### 1.3 SSH 连接 RZ/G2L

```bash
# 默认通过串口或以太网连接
ssh root@192.168.1.100
# 确认系统信息
uname -a
cat /proc/cpuinfo | grep "processor\|model name"
free -h
```

---

### 阶段 2：交叉编译 + 部署（3-5 天）

#### 2.1 编译 sensing-server

```bash
cd rust-server
# 交叉编译 (在 x86_64 开发机上)
cargo build --target aarch64-unknown-linux-gnu --release

# 检查二进制
file target/aarch64-unknown-linux-gnu/release/sensing-server
# 期望输出: ELF 64-bit LSB executable, ARM aarch64...
```

#### 2.2 部署到 RZ/G2L

```bash
# 使用 deploy.sh 一键部署
# 或将二进制 + UI 文件 SCP 到设备
scp target/aarch64-unknown-linux-gnu/release/sensing-server root@192.168.1.100:/opt/WCES/
scp -r ui/ root@192.168.1.100:/opt/WCES/
scp -r docs/triage-ui/ root@192.168.1.100:/opt/WCES/

# SSH 到设备启动
ssh root@192.168.1.100
cd /opt/WCES
./deploy.sh
```

---

### 阶段 3：性能适配（1 周）

#### 3.1 LLM 模式决策

RZ/G2L 1GB DDR4 内存紧张，LLM 需评估：

| 模式 | 内存占用 | 可行性 |
|------|---------|--------|
| `llm` (Qwen2.5-0.5B INT4) | ~500MB 模型 + ~500MB 系统 | ⚠️ 勉强可行，需精简 OS |
| `template-only` | ~200MB 总量 | ✅ 推荐 |
| 禁用 | ~150MB 总量 | ✅ 最稳定 |

**建议**: 竞赛演示使用 `template-only` 模式，LLM 推理留作扩展能力展示。

#### 3.2 内存优化

```bash
# 精简 Linux 服务
systemctl disable --now bluetooth
systemctl disable --now wpa_supplicant  # 如用有线
systemctl disable --now avahi-daemon

# 检查可用内存
free -h
```

#### 3.3 CPU 性能预估

| 操作 | RZ/V2H (4×A55) | RZ/G2L (2×A55) |
|------|---------------|----------------|
| CSI 信号处理 | <1ms | <2ms |
| START 分诊 | <1ms | <1ms |
| 边缘模块 (19个) | <5ms | <10ms |
| LLM 推理 (100 token) | ~50s | ~100s |
| 端到端延迟 | <10ms | <20ms |

**核心分诊功能完全可行，LLM 推理延迟增加但功能不受影响。**

---

### 阶段 4：集成测试（3-5 天）

#### 4.1 功能验证

```bash
# 1. 模拟模式测试 (无需 ESP32 硬件)
./sensing-server --source simulate --http-port 8080

# 2. 浏览器验证
# http://192.168.1.1:8080/ui/triage.html

# 3. API 验证
curl http://192.168.1.1:8080/api/v1/health
```

#### 4.2 压力测试

```bash
# 24 小时稳定性测试
./sensing-server --source auto --http-port 8080 &
sleep 86400
# 检查内存泄漏
ps aux | grep sensing-server
```

---

## 性能优化

### 编译器优化

```bash
# 针对 Cortex-A55 优化
export RUSTFLAGS="-C target-cpu=cortex-a55 -C target-feature=+neon"
cargo build --target aarch64-unknown-linux-gnu --release
```

### 内存优化

```rust
// 减少 CSI 缓冲区大小 (适配 1GB RAM)
const FRAME_HISTORY_SIZE: usize = 128;  // 原 256
const RING_BUFFER_SIZE: usize = 30;     // 原 60
```

---

## 时间规划

| 阶段 | 任务 | 时间 | 交付物 |
|------|------|------|--------|
| **阶段 1** | 环境搭建 + BSP 烧录 | 3-5 天 | 可 SSH 的 RZ/G2L 设备 |
| **阶段 2** | 交叉编译 + 部署 | 3-5 天 | 设备上运行 sensing-server |
| **阶段 3** | 性能适配 + LLM 决策 | 1 周 | 优化后的配置 |
| **阶段 4** | 集成测试 | 3-5 天 | 测试报告 |

**总计：3-4 周**（比原 RZ/V2H 计划的 8-10 周更短，因为无需 DRP-AI 适配）

---

## 故障排查

### 问题 1：交叉编译链接错误

```bash
# 确认工具链安装
aarch64-linux-gnu-gcc --version

# 确认 Rust 目标
rustup target list --installed | grep aarch64
```

### 问题 2：RZ/G2L 启动无显示

```bash
# 通过串口调试 (40-pin GPIO 上的 UART)
# 或检查 HDMI 连接
# 参考快速入门手册的串口连接部分
```

### 问题 3：内存不足

```bash
# 检查内存使用
free -h
# 关闭不必要的服务
systemctl list-units --type=service --state=running
# 使用 template-only 替代 LLM 模式
```

---

## 参考资料

- RZ/G2L SBC 快速入门指南：https://www.renesas.cn/zh/document/qsg/rzg2l-sbc-single-board-computer-quick-start-guide
- RZ/G2L 产品页面：https://www.renesas.com/rzg2l-sbc
- RZ/G2L 数据手册：https://www.renesas.com/rzg2l-datasheet
- Rust 交叉编译：https://rust-lang.github.io/rustup/cross-compilation.html
