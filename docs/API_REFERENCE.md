# WCES WebSocket 数据接口文档

> 版本：v0.3 | 日期：2026-05-12
> 用途：供前端/AI开发新UI时参考，描述WebSocket推送的全部数据格式

---

## 连接信息

| 项目 | 值 |
|------|-----|
| 协议 | WebSocket |
| 地址 | `ws://{host}:8765/ws/sensing` |
| 频率 | ~10 FPS（模拟模式 100ms/帧） |
| 格式 | JSON 文本帧 |
| 过滤 | 客户端只处理 `type == "sensing_update"` 的消息 |

---

## 主消息：SensingUpdate

WebSocket 每秒推送约 10 条 JSON 消息，每条都是 `SensingUpdate` 结构：

```json
{
  "type": "sensing_update",
  "timestamp": 1747081200.123,
  "source": "simulated",
  "tick": 42,
  "nodes": [ { ... } ],
  "features": { ... },
  "classification": { ... },
  "signal_field": { ... },
  "vital_signs": { ... },
  "triage_update": { ... },
  "wasm_alerts": [ ... ],
  "pose_keypoints": [ ... ],
  "model_status": { ... },
  "persons": [ ... ],
  "estimated_persons": 1
}
```

### 顶层字段

| 字段 | 类型 | 始终存在 | 说明 |
|------|------|:--:|------|
| `type` | `string` | ✅ | 固定值 `"sensing_update"` |
| `timestamp` | `number` | ✅ | Unix 时间戳（秒，浮点） |
| `source` | `string` | ✅ | `"simulated"` 或 `"esp32"` |
| `tick` | `integer` | ✅ | 帧序号，单调递增 |
| `nodes` | `array` | ✅ | 节点信息数组（至少1个） |
| `features` | `object` | ✅ | CSI 信号特征 |
| `classification` | `object` | ✅ | 运动/存在分类 |
| `signal_field` | `object` | ✅ | 20×20 信号场热力图 |
| `vital_signs` | `object?` | ❌ | 生命体征（呼吸率/心率） |
| `triage_update` | `object?` | ❌ | MAT分诊数据（核心） |
| `wasm_alerts` | `array?` | ❌ | 边缘模块异常事件 |
| `pose_keypoints` | `array?` | ❌ | DensePose骨架（需加载模型） |
| `model_status` | `object?` | ❌ | 模型加载状态 |
| `persons` | `array?` | ❌ | 多人检测结果 |
| `estimated_persons` | `integer?` | ❌ | 估计人数（1-3） |

> 标记 `?` 的字段可能不存在（`skip_serializing_if = "None"`）

---

## 1. nodes — 节点信息

```json
[
  {
    "node_id": 1,
    "rssi_dbm": -38.5,
    "position": [2.0, 0.0, 1.5],
    "amplitude": [15.2, 14.8, 16.1, ...],
    "subcarrier_count": 56
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `node_id` | `integer` | ESP32 节点编号 (1/2/3) |
| `rssi_dbm` | `number` | 信号强度 dBm（用于距离估算） |
| `position` | `[x, y, z]` | 节点三维坐标（米） |
| `amplitude` | `number[]` | 56 个子载波振幅值 |
| `subcarrier_count` | `integer` | 子载波数量 |

---

## 2. features — CSI 信号特征

```json
{
  "mean_rssi": -38.5,
  "variance": 2.34,
  "motion_band_power": 0.12,
  "breathing_band_power": 0.45,
  "dominant_freq_hz": 0.25,
  "change_points": 3,
  "spectral_power": 1.87
}
```

| 字段 | 类型 | 范围 | 说明 |
|------|------|------|------|
| `mean_rssi` | `number` | -90~-20 | 平均信号强度 dBm |
| `variance` | `number` | 0~20 | 子载波幅度方差（运动指示） |
| `motion_band_power` | `number` | 0~10 | 运动频带能量（>0.5Hz） |
| `breathing_band_power` | `number` | 0~5 | 呼吸频带能量（0.1-0.5Hz） |
| `dominant_freq_hz` | `number` | 0~5 | 主频率 Hz |
| `change_points` | `integer` | 0~5 | 信号突变次数 |
| `spectral_power` | `number` | 0~10 | 总频谱功率 |

---

## 3. classification — 运动/存在分类

```json
{
  "motion_level": "present_still",
  "presence": true,
  "confidence": 0.87
}
```

| 字段 | 类型 | 可选值 | 说明 |
|------|------|------|------|
| `motion_level` | `string` | `"active"` `"present_still"` `"idle"` | 运动等级 |
| `presence` | `boolean` | — | 是否检测到人体存在 |
| `confidence` | `number` | 0~1 | 分类置信度 |

---

## 4. signal_field — 信号场热力图

```json
{
  "grid_size": [20, 20, 1],
  "values": [0.05, 0.12, 0.08, ...]
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `grid_size` | `[w, h, d]` | 网格尺寸（20×20×1） |
| `values` | `number[]` | 400 个浮点值（逐行），范围 0~1 |

> 可用于绘制顶视图热力图，高值区域表示信号活跃方向

---

## 5. vital_signs — 生命体征（⭐核心）

```json
{
  "breathing_rate_bpm": 14.5,
  "breathing_confidence": 0.82,
  "heart_rate_bpm": 72.3,
  "heartbeat_confidence": 0.75,
  "signal_quality": 0.68,
  "motion_score": 0.3,
  "n_persons": 1
}
```

| 字段 | 类型 | 范围 | 说明 |
|------|------|------|------|
| `breathing_rate_bpm` | `number?` | 6~30 | 呼吸率（次/分钟），null=未检出 |
| `breathing_confidence` | `number` | 0~1 | 呼吸率置信度 |
| `heart_rate_bpm` | `number?` | 40~120 | 心率（BPM），null=未检出 |
| `heartbeat_confidence` | `number` | 0~1 | 心率置信度 |
| `signal_quality` | `number` | 0~1 | 综合信号质量 |
| `motion_score` | `number` | 0~1 | 运动强度（0=静止，1=剧烈） |
| `n_persons` | `integer` | 0~3 | 检测到的人数 |

---

## 6. triage_update — MAT 分诊数据（⭐核心）

```json
{
  "type": "triage_update",
  "survivors": [
    {
      "id": "SURV-0042",
      "triage": "Minor",
      "triage_color": "green",
      "triage_priority": 3,
      "breathing_rate": 14.5,
      "heart_rate": 72.3,
      "motion_score": 0.3,
      "position": [0.5, -0.2, 0.0],
      "position_confidence": 0.68,
      "is_deteriorating": false,
      "tracked_seconds": 15.2,
      "node_id": 1,
      "estimated_age": "Adult",
      "status": "active"
    }
  ],
  "assessment": {
    "total": 3,
    "immediate": 0,
    "delayed": 1,
    "minor": 2,
    "deceased": 0,
    "unknown": 0,
    "severity": "Minimal",
    "rescuer_estimate": 1
  },
  "alerts": [
    {
      "time": "14:30:25",
      "survivor_id": "SURV-001a",
      "alert_type": "DETERIORATION",
      "message": "Minor → Immediate",
      "priority": 1
    }
  ]
}
```

### survivors[] — 伤员列表

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `string` | 伤员ID（如 `SURV-0042`） |
| `triage` | `string` | 分诊等级：`Immediate` `Delayed` `Minor` `Deceased` `Unknown` |
| `triage_color` | `string` | 颜色：`red` `yellow` `green` `black` `gray` |
| `triage_priority` | `integer` | 优先级（1=最高，5=最低） |
| `breathing_rate` | `number?` | 平滑呼吸率（3帧均值），null=无数据 |
| `heart_rate` | `number?` | 平滑心率（3帧均值），null=无数据 |
| `motion_score` | `number` | 运动强度 0~1 |
| `position` | `[x, y, z]` | 估计位置（米），基于三角定位 |
| `position_confidence` | `number` | 位置可信度 0~1 |
| `is_deteriorating` | `boolean` | 是否正在恶化 |
| `tracked_seconds` | `number` | 已追踪时长（秒） |
| `node_id` | `integer` | 来源节点 |
| `estimated_age` | `string` | 预估年龄：`Infant` `Child` `Adult` `Elderly` `Unknown` |
| `status` | `string` | 状态：`active` `rescued` `lost` `deceased` |

### assessment — 群体伤情评估

| 字段 | 类型 | 说明 |
|------|------|------|
| `total` | `integer` | 伤员总数 |
| `immediate` | `integer` | 🔴 紧急（红色）人数 |
| `delayed` | `integer` | 🟡 延迟（黄色）人数 |
| `minor` | `integer` | 🟢 轻伤（绿色）人数 |
| `deceased` | `integer` | ⚫ 死亡（黑色）人数 |
| `unknown` | `integer` | ⚪ 未知（灰色）人数 |
| `severity` | `string` | 整体伤情：`Minimal` `Moderate` `Major` `Critical` |
| `rescuer_estimate` | `integer` | 建议救援人员数量（红×4 + 黄×2 + 绿×0.5） |

### alerts[] — MAT告警列表

| 字段 | 类型 | 说明 |
|------|------|------|
| `time` | `string` | 告警时间（HH:MM:SS） |
| `survivor_id` | `string` | 关联伤员ID |
| `alert_type` | `string` | `DETERIORATION`（恶化） |
| `message` | `string` | 告警详情（如 `Minor → Immediate`） |
| `priority` | `integer` | 优先级 1~5 |

---

## 7. wasm_alerts — 边缘模块异常事件

```json
[
  {
    "module": "vital_trend",
    "event_type": 104,
    "event_name": "Tachycardia",
    "value": 135.0,
    "severity": "critical"
  },
  {
    "module": "attractor",
    "event_type": 737,
    "event_name": "BasinDeparture",
    "value": 3.5,
    "severity": "critical"
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `module` | `string` | 模块名（见下表） |
| `event_type` | `integer` | 事件类型编号 |
| `event_name` | `string` | 事件名称 |
| `value` | `number` | 事件数值（含义依模块而定） |
| `severity` | `string` | 严重度：`critical` `warning` `info` |

### 10个边缘模块事件枚举

| 模块 | 事件 | severity | 触发条件 |
|------|------|:--:|------|
| `vital_trend` | Bradypnea(101) | warning | 呼吸<12 BPM持续5秒 |
| `vital_trend` | Tachypnea(102) | warning | 呼吸>25 BPM持续5秒 |
| `vital_trend` | Bradycardia(103) | warning | 心率<50 BPM持续5秒 |
| `vital_trend` | Tachycardia(104) | critical | 心率>120 BPM持续5秒 |
| `vital_trend` | Apnea(105) | critical | 呼吸停止≥20秒 |
| `attractor` | LearningComplete(738) | info | 吸引子学习完成（200帧后） |
| `attractor` | BasinDeparture(737) | critical | CSI轨迹离开吸引子basin |
| `resp_distress` | Tachypnea(120) | warning | 呼吸>25持续8秒 |
| `resp_distress` | LaboredBreathing(121) | warning | 振幅方差超基线3倍 |
| `resp_distress` | CheyneStokes(122) | critical | 潮式呼吸模式检出 |
| `confined_space` | WorkerEntry(510) | info | 人员进入密闭空间 |
| `confined_space` | WorkerExit(511) | info | 人员离开密闭空间 |
| `confined_space` | ExtractionAlert(513) | critical | 无呼吸>15秒（需立即救援） |
| `confined_space` | ImmobileAlert(514) | critical | 无运动>60秒（可能昏迷） |
| `panic_motion` | PanicDetected(250) | critical | 高加加速度+高熵+高运动 |
| `panic_motion` | StrugglePattern(251) | warning | 中等加加速度+熵 |
| `panic_motion` | FleeingDetected(252) | warning | 持续高能量+低熵（奔跑） |
| `sleep_apnea` | ApneaStart(100) | critical | 呼吸<4 BPM持续10秒 |
| `sleep_apnea` | ApneaEnd(101) | info | 呼吸暂停结束 |
| `sleep_apnea` | AHIUpdate(102) | info | 每小时呼吸暂停指数 |
| `cardiac` | Tachycardia(110) | warning | 心率>100 BPM持续2秒 |
| `cardiac` | Bradycardia(111) | warning | 心率<50 BPM持续2秒 |
| `cardiac` | MissedBeat(112) | critical | 心率骤降>30% |
| `cardiac` | HRVAnomaly(113) | warning | RMSSD异常 |
| `seizure` | SeizureOnset(140) | critical | 癫痫发作开始 |
| `seizure` | SeizureTonic(141) | critical | 强直期 |
| `seizure` | SeizureClonic(142) | critical | 阵挛期 |
| `seizure` | PostIctal(143) | warning | 发作后恢复期 |
| `intrusion` | IntrusionAlert(200) | critical | 入侵检测触发 |
| `intrusion` | IntrusionArmed(202) | info | 系统已布防 |

---

## 8. pose_keypoints — 骨架数据（目前为 None）

```json
[
  [0.5, 1.2, 0.0, 0.85],
  [0.4, 1.3, 0.0, 0.82],
  ...
]
```

> 17个COCO关键点：[x, y, z, confidence] 数组
> 顺序：nose, left_eye, right_eye, left_ear, right_ear, left_shoulder, right_shoulder, left_elbow, right_elbow, left_wrist, right_wrist, left_hip, right_hip, left_knee, right_knee, left_ankle, right_ankle
> ⚠️ **当前状态**：模拟模式下 `pose_keypoints` 需要加载 .rvf 模型才生成（启动时加 `--model data/models/trained-supervised-20260302_165735.rvf`）。模型加载后 `pose_keypoints` 包含 17 点 COCO 骨架动画（含呼吸微动）。

---

## 9. persons — 多人检测（派生自信号）

```json
[
  {
    "id": 1,
    "confidence": 0.82,
    "keypoints": [
      { "name": "nose", "x": 0.5, "y": 1.2, "z": 0.0, "confidence": 0.8 },
      ...
    ],
    "bbox": { "x": 0.3, "y": 0.8, "width": 0.4, "height": 1.2, "depth": 0.3 },
    "zone": "zone_1"
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `integer` | 人员编号 |
| `confidence` | `number` | 检测置信度 |
| `keypoints` | `array` | PoseKeypoint 数组（同上17点） |
| `bbox` | `object` | 3D 包围盒 {x, y, width, height, depth} |
| `zone` | `string` | 区域标识 |

> 坐标范围：x≈[-2, 2], y≈[-1.5, 1.5], z≈[0, 2]（米）

---

## 其他 WebSocket 消息类型

### /ws/pose 端点

与 `/ws/sensing` 共用同一个 broadcast channel，但转换了格式：

```json
{
  "type": "pose_data",
  "zone_id": "zone_1",
  "timestamp": 1747081200.123,
  "payload": {
    "pose": {
      "persons": [ { "id": 1, "keypoints": [...], ... } ]
    },
    "confidence": 0.87,
    "activity": "present_still",
    "pose_source": "signal_derived"
  }
}
```

首次连接时收到：
```json
{
  "type": "connection_established",
  "payload": { "status": "connected", "backend": "rust+ruvector" }
}
```

---

## 建议的 UI 渲染策略

### 必须渲染（始终有数据）
- ✅ 统计栏：`triage_update.assessment`（total/immediate/delayed/minor/deceased）
- ✅ 伤员卡片：`triage_update.survivors[]`（ID/分诊/体征/位置/追踪时长/恶化标志）
- ✅ Canvas 2D 地图：`survivors[].position` 转画布坐标
- ✅ 节点标识：`nodes[].node_id` + `nodes[].position`（三节点三角标记）

### 推荐渲染（可能有数据）
- ✅ 告警列表：`triage_update.alerts[]` + `wasm_alerts[]`（合并或分栏）
- ✅ 生命体征面板：`vital_signs.breathing_rate_bpm` / `heart_rate_bpm`
- ✅ 信号质量指示器：`vital_signs.signal_quality` / `classification.confidence`
- ✅ 热力图：`signal_field.values` → Canvas 或 WebGL

### 可选渲染（当前无数据）
- ⚠️ 3D 骨架：`persons[].keypoints`（需模型加载，模拟模式无）
- ⚠️ pose_keypoints（同上）

---

## HTTP API 端点（补充参考）

| 端点 | 说明 | 返回 |
|------|------|------|
| `GET /` | 信息页 | HTML（含链接） |
| `GET /health` | 健康检查 | `{"status":"ok"}` |
| `GET /api/v1/sensing/latest` | 最新 SensingUpdate | JSON（同上结构） |
| `GET /api/v1/vital-signs` | 当前生命体征 | `{"breathing_rate_bpm":14.5,...}` |
| `GET /api/v1/wasm-events` | 最新WASM事件 | `{"wasm_events":[...]}` |
| `GET /api/v1/status` | 系统状态 | JSON |
