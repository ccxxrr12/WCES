# WiFi DensePose 前端用户界面

基于模块化、现代化 Web 技术构建的 WiFi DensePose 人体追踪系统用户界面。提供实时监控、WiFi 感知可视化以及来自 CSI（信道状态信息）的姿态估计功能。

## 架构设计

本 UI 采用模块化架构设计，遵循清晰的职责分离原则：

```
ui/
├── app.js                    # 主应用入口点
├── index.html                # 带选项卡结构的 HTML 外壳
├── style.css                 # 完整的 CSS 设计系统
├── config/
│   └── api.config.js         # API 端点与配置文件
├── services/
│   ├── api.service.js        # HTTP API 客户端
│   ├── websocket.service.js  # WebSocket 连接管理器
│   ├── websocket-client.js   # 底层 WebSocket 客户端
│   ├── pose.service.js       # 姿态估计 API 封装
│   ├── sensing.service.js    # WiFi 感知数据服务（实时 + 模拟降级）
│   ├── health.service.js     # 健康监测 API 封装
│   ├── stream.service.js     # 流式 API 封装
│   └── data-processor.js     # 信号数据处理工具
├── components/
│   ├── TabManager.js         # 选项卡导航组件
│   ├── DashboardTab.js       # 实时系统指标仪表盘
│   ├── SensingTab.js         # WiFi 感知可视化（3D 信号场、指标）
│   ├── LiveDemoTab.js        # 带设置指南的实时姿态检测
│   ├── HardwareTab.js        # 硬件配置面板
│   ├── SettingsPanel.js      # 设置面板
│   ├── PoseDetectionCanvas.js # 基于 Canvas 的姿态骨架渲染器
│   ├── gaussian-splats.js    # 3D 高斯斑点信号场渲染器（Three.js）
│   ├── body-model.js         # 3D 人体模型
│   ├── scene.js              # Three.js 场景管理
│   ├── signal-viz.js         # 信号可视化工具
│   ├── environment.js        # 环境/房间可视化
│   └── dashboard-hud.js      # 仪表盘抬头显示
├── utils/
│   ├── backend-detector.js   # 自动检测后端可用性
│   ├── mock-server.js        # 用于测试的模拟服务器
│   └── pose-renderer.js      # 姿态渲染工具
└── tests/
    ├── test-runner.html       # 测试运行器 UI
    ├── test-runner.js         # 测试框架与用例
    └── integration-test.html  # 集成测试页面
```

## 核心功能

### WiFi 感知选项卡
- 3D 高斯斑点信号场可视化（Three.js）
- 实时 RSSI、方差、运动频带、呼吸频带指标
- 带置信度评分的存在/运动分类
- **数据源横幅**：绿色 "LIVE - ESP32"、黄色 "RECONNECTING..." 或红色 "SIMULATED DATA"
- RSSI 历史折线图
- "关于本数据"卡片，解释每个传感器数量的 CSI 能力

### 实时演示选项卡
- 基于 WebSocket 的实时姿态骨架渲染
- **估计模式徽章**：绿色 "Signal-Derived"（信号衍生）或蓝色 "Model Inference"（模型推理）
- **设置指南面板**，展示不同 ESP32 数量提供的能力：
  - 单个 ESP32：存在检测、呼吸监测、粗粒度运动
  - 2-3 个 ESP32：人体定位、运动方向
  - 4+ 个 ESP32 + 已训练模型：独立肢体跟踪、完整姿态
- 支持调试模式与日志导出
- 区域选择与强制重连控制
- 性能指标侧边栏（帧数、运行时间、错误数）

### 仪表盘
- 实时系统健康监测
- 实时姿态检测统计
- 区域占用追踪
- 系统指标（CPU、内存、磁盘）
- API 状态指示器

### 硬件配置
- 交互式天线阵列可视化
- 实时 CSI 数据显示
- 配置面板
- 硬件状态监测

## 数据源状态

感知服务 (`sensing.service.js`) 支持三种连接状态：

| 状态 | 横幅颜色 | 描述 |
|------|---------|------|
| **LIVE - ESP32** | 绿色 | 已连接至 Rust 感知服务器，接收真实 CSI 数据 |
| **RECONNECTING** | 黄色（闪烁） | WebSocket 断开，正在重试（最多 20 次） |
| **SIMULATED DATA** | 红色 | 超过 5 次重连失败后降级为客户端本地模拟 |

模拟帧中包含 `_simulated: true` 标记，使代码能识别合成数据。

## 后端支持

### Rust 感知服务器（主后端）
基于 Rust 的 `wifi-densepose-sensing-server` 服务提供 UI 并支持以下接口：

- `GET /health` —— 服务器健康状态
- `GET /api/v1/sensing/latest` —— 最新感知特征
- `GET /api/v1/vital-signs` —— 生命体征估计（心率/呼吸率）
- `GET /api/v1/model/info` —— RVF 模型容器信息
- `WS /ws/sensing` —— 实时感知数据流
- `WS /api/v1/stream/pose` —— 实时姿态关键点流

### Python FastAPI（遗留支持）
原始 Python 后端（8000 端口）仍受支持。UI 通过 `backend-detector.js` 自动检测可用后端类型。

## 快速启动

### 使用 Docker（推荐）

```bash
cd docker/

# 默认：自动检测 5005 端口 UDP 的 ESP32，失败后降级为模拟
docker-compose up

# 强制使用真实 ESP32 数据
CSI_SOURCE=esp32 docker-compose up

# 强制模拟（无需任何硬件）
CSI_SOURCE=simulated docker-compose up
```

打开 http://localhost:3000/ui/index.html

### 使用本地 Rust 二进制文件

```bash
cd rust-port/wifi-densepose-rs
cargo build -p wifi-densepose-sensing-server --no-default-features

# 使用模拟数据运行
../../target/debug/sensing-server --source simulated --tick-ms 100 --ui-path ../../ui --http-port 3000

# 使用真实 ESP32 运行
../../target/debug/sensing-server --source esp32 --tick-ms 100 --ui-path ../../ui --http-port 3000
```

打开 http://localhost:3000/ui/index.html

### 使用 Python HTTP 服务器（遗留）

```bash
# 在 8000 端口启动 FastAPI 后端
wifi-densepose start

# 在 3000 端口提供 UI 服务
cd ui/
python -m http.server 3000
```

打开 http://localhost:3000

## 姿态估计模式

| 模式 | 徽章 | 要求 | 准确度 |
|------|------|------|-------|
| **Signal-Derived**（信号衍生） | 绿色 | 1+ 个 ESP32，无需模型 | 存在检测、呼吸监测、粗粒度运动 |
| **Model Inference**（模型推理） | 蓝色 | 4+ 个 ESP32 + 已训练 `.rvf` 模型 | 完整 17 关键点 COCO 姿态 |

要使用模型推理，请使用已训练模型启动服务器：

```bash
sensing-server --source esp32 --model path/to/model.rvf --ui-path ./ui
```

## 配置说明

### API 配置

编辑 `config/api.config.js`：

```javascript
export const API_CONFIG = {
  BASE_URL: window.location.origin,
  API_VERSION: '/api/v1',
  WS_CONFIG: {
    RECONNECT_DELAY: 5000,     // 重连延迟：5 秒
    MAX_RECONNECT_ATTEMPTS: 20, // 最大重连次数：20
    PING_INTERVAL: 30000         // 心跳间隔：30 秒
  }
};
```

## 测试运行

打开 `tests/test-runner.html` 运行测试套件：

```bash
cd ui/
python -m http.server 3000
# 打开 http://localhost:3000/tests/test-runner.html
```

测试类别：API 配置、API 服务、WebSocket、姿态服务、健康服务、UI 组件、集成测试。

## 样式系统

采用带有自定义属性、深色/浅色主题、响应式布局和基于组件的样式的 CSS 设计系统。关键变量定义在 `style.css` 的 `:root` 中。

## 许可协议

属于 WiFi-DensePose 系统的一部分。详见主项目 LICENSE 文件。