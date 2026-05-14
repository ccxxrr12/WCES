# WiFi-DensePose 机器学习架构详解

> 项目 ML 系统完整说明：模型架构、训练流程、推理部署、竞赛应用
---

## 一、模型总览

### 核心功能
将 **WiFi CSI 信号**（振幅 + 相位）转换为 **人体 3D 姿态**（17 个关键点 + DensePose 体表 UV 映射）。
### 论文来源
CMU "DensePose From WiFi" (arXiv:2301.00250)，证明了 WiFi CSI 信号包含足够的信息来重建人体姿态，精度接近 RGB 摄像头。
### 模型名称
`WiFiDensePoseModel`（端到端 CSI→姿态模型），位于 `crates/wifi-densepose-training/src/model.rs`

---

## 二、模型架构
```
                           输入层
         ┌──────────────────────────────────────┐
         │ CSI 振幅 [B, T×n_tx×n_rx, n_sub]    │
         │ CSI 相位 [B, T×n_tx×n_rx, n_sub]    │
         │ 维度: B=批大小, T=时间窗口帧数        │
         │ n_tx=发射天线, n_rx=接收天线         │
         │ n_sub=子载波数                       │
         └──────────────┬───────────────────────┘
                        │
         ┌──────────────▼───────────────────────┐
         │       ModalityTranslator             │
         │ "模态翻译器"：CSI→视觉特征空间       │
         │                                      │
         │ 结构: CNN Encoder + CNN Decoder      │
         │ 输入: [B, flat_csi] (展平后的CSI)    │
         │ 中间: [B, n_ant, n_sc] (恢复2D)     │
         │       + 多个 Conv2D + BatchNorm      │
         │       + ReLU + 可选 Attention        │
         │ 输出: [B, 256, H/4, W/4] 特征图     │
         │                                      │
         │ 作用: 将 1D 射频信号"翻译"成         │
         │ 2D 空间特征, 类似于摄像头的中间表示  │
         └──────────────┬───────────────────────┘
                        │
         ┌──────────────▼───────────────────────┐
         │        ResNet18 Backbone             │
         │ "骨干网络"：标准视觉特征提取器       │
         │                                      │
         │ BasicBlock × [2,2,2,2]               │
         │ 通道: 64→128→256→512                 │
         │ 输出: [B, 256, H/4, W/4]             │
         │                                      │
         │ 作用: 从翻译后的特征中提取更高层次    │
         │ 的人体结构信息                        │
         └──────┬────────────────┬──────────────┘
                │                │
    ┌───────────▼──────┐ ┌──────▼──────────────┐
    │ KeypointHead     │ │ DensePoseHead        │
    │ "关键点头"       │ │ "体表映射头"         │
    │                  │ │                      │
    │ ConvTranspose2d  │ │ ConvTranspose2d       │
    │ → 上采样到 H×W   │ │ → 上采样到 H×W        │
    │                  │ │                      │
    │ 输出:            │ │ 输出:                 │
    │ [B, 17, H, W]    │ │ [B, 25, H, W] 部件   │
    │ 17个COCO关键点   │ │ 24身体部位+1背景      │
    │ 每个一张热力图   │ │ [B, 48, H, W] UV     │
    │                  │ │ 24部位×2通道 UV坐标   │
    └──────────────────┘ └──────────────────────┘

    17 COCO 关键点:
    0-鼻子 1-左眼  2-右眼  3-左耳 4-右耳
    5-左肩 6-右肩 7-左肘  8-右肘 9-左腕 10-右腕
    11-左髋 12-右髋 13-左膝 14-右膝 15-左踝 16-右踝

    24 DensePose 部位:
    躯干/四肢：24个表面区域
    UV: 每个像素的(u,v)纹理坐标, 映射到人体表面
```

---

## 三、训练流程
### 3.1 数据集：MM-Fi

```
MM-Fi 目录结构:
<root>/
  S01/              ← 第1个受试者
    A01/            ← 第1个动作(走路/坐下/挥手...)
      wifi_csi.npy          [T, 3tx, 3rx, 114sc]  ← 振幅
      wifi_csi_phase.npy    [T, 3tx, 3rx, 114sc]  ← 相位
      gt_keypoints.npy      [T, 17, 3]             ← 真值(x,y,可见性)
    A02/
      ...
  S02/
    ...
```

### 3.2 数据预处理
```
1. 子载波插值:  114 → 56 (线性插值, 统一输入尺寸)
                 对C5应该设为: 484 → 242 或保持原值242

2. 滑动窗口:    取100帧CSI → 1个训练样本
                 窗口维度: [100, n_tx, n_rx, n_sub]

3. 归一化:      振幅归一化到[0,1], 相位归一化到[-π,π]

4. 关键点坐标:  归一化到[0,1], 原点在图像左上角

5. 热力图生成:  用高斯核将关键点(x,y)转为热力图
                 真值热力图 = exp(-((x-x0)²+(y-y0)²)/(2σ²))
```

### 3.3 损失函数

```
L_total = λ_kp × L_keypoint  +  λ_dp × L_densepose  +  λ_tr × L_transfer
           ↑                    ↑                      ↑
       权重~1.0              权重~0.5               权重~0.1

L_keypoint:  预测热力图 vs 真值热力图 → MSE (均方误差)
             仅对可见关键点计算 (visibility > 0)

L_densepose: 部件分类: CrossEntropy (25类)
             UV回归:   Smooth-L1 (Huber loss)

L_transfer:  学生特征 vs 教师特征 → MSE
             教师 = 预训练RGB模型的特征图
             作用: 知识蒸馏, 让WiFi模型学到类似视觉的特征
```

### 3.4 优化配置

| 参数 | 默认值 | 说明 |
|------|--------|------|
| 优化器 | AdamW | weight_decay=0.01 |
| 学习率 | 1e-3 | 初始 |
| LR 调度 | StepLR | 每10 epoch ×0.1 |
| 批大小 | 8 | |
| 梯度裁剪 | 1.0 | 防止梯度爆炸 |
| 早停 | patience=20 | val_loss 不降则停止 |
| Epoch | 100-300 | |
| 设备 | CUDA GPU | LibTorch 后端 |

### 3.5 评估指标

| 指标 | 说明 |
|------|------|
| **PCK@0.2** | Percentage of Correct Keypoints：预测关键点与真值距离 < 0.2×躯干长度的比例 |
| **OKS** | Object Keypoint Similarity：COCO 标准评估, 综合距离+可见性 |
| **val_loss** | 验证集总损失 |

---

## 四、推理部署
### 4.1 三种推理后端

| 后端 | 依赖 | 适用场景 | RZ/V2H 可用 |
|------|------|----------|:-----------:|
| **tch-rs** (LibTorch) | libtorch.so (~2GB) | 训练+GPU推理 | ❌ (太重) |
| **ONNX Runtime** | onnxruntime (~20MB) | **跨平台 CPU 推理** | ✅ |
| **Candle** | 纯 Rust, 无C++依赖 | 轻量级推理 | ✅ |

**竞赛推荐 ONNX Runtime**：最小的依赖体积, ARM64 原生支持。
### 4.2 ONNX 推理流程

```
1. 导出模型:
   python export_onnx.py --input densepose_model.pt --output densepose.onnx

2. RZ/V2H 推理:
   let model = OnnxModel::load("densepose.onnx")?;
   let output = model.run(vec![csi_input])?;
   // output[0] → keypoints [1,17,56,56]
   // output[1] → parts [1,25,56,56]
   // output[2] → uv [1,48,56,56]

3. 后处理:
   - 从热力图中提取关键点坐标 (argmax)
   - 连接关键点形成骨架
   - 通过 WebSocket 发送到 UI

4. UI 渲染:
   - Three.js 3D 骨架 (pose-fusion/)
   - 实时跟随人体动作
```

### 4.3 推理性能

| 后端 | 延迟 (CPU) | 内存 |
|------|-----------|------|
| ONNX Runtime | 100-200ms | ~200MB |
| Candle | 150-300ms | ~150MB |
| tch-rs | 200-400ms | ~500MB |

---

## 五、与信号处理方案的关系
```
项目中有两条并行管道:

管道 A: 信号处理 (不需要模型)          管道 B: ML推理 (DensePose模型)
─────────────────────────────────     ─────────────────────────────
CSI 振幅/相位                         CSI 振幅/相位
  │                                     │
  ├─ 特征提取 (features.rs)              ├─ ModalityTranslator
  │                                     │
  ├─ 呼吸率 (Fresnel + FFT)              ├─ ResNet18 Backbone
  ├─ 心率 (BVP + 带通滤波)               │
  ├─ 运动检测 (振幅方差)                  ├─ DensePose输出
  ├─ 存在检测 (自适应阈值)                │  (17关键点 + UV映射)
  └─ 人员定位 (三角测量)                  │
  │                                     │
START 分诊 (规则引擎)                     │
  │                                   3D骨架可视化
  │                                   (Three.js渲染)
告警 + 仪表板
─────────────────────────────────     ─────────────────────────────
延迟: <50ms                           延迟: 100-200ms
CPU占用: 低                            CPU占用: 中
无额外依赖                             需要 ONNX Runtime
竞赛必须 ✅                            竞赛可选加分
```

---

## 六、针对你的 C5 竞赛项目的适配建议

### 6.1 子载波数调整

当前模型默认 56 子载波（适配 S3）。C5 在 20MHz 下有 242 子载波：

```rust
// 修改 TrainingConfig 或推理时的预处理
let config = TrainingConfig {
    num_subcarriers: 242,        // C5 20MHz HE
    native_subcarriers: 484,     // C5 40MHz HE → 插值到 242
    ..TrainingConfig::default()
};
```

### 6.2 天线数调整
当前模型默认 3×3 MIMO。C5 可能是 1×1：
```rust
let config = TrainingConfig {
    num_antennas_tx: 1,     // C5 单天线
    num_antennas_rx: 1,     // C5 单天线
    ..
};
```

⚠️ 天线数减少会显著降低姿态估计精度。但你的 3 节点配置可以通过"多静态"弥补——3 个节点从不同角度感知 = 虚拟多天线。

### 6.3 UI 按钮控制方案

```
┌──────────────────────────────────────────┐
│ WCES — Triage Dashboard                  │
│                                          │
│ ┌──────────────────────────────────────┐ │
│ │ 伤员 #1 | 呼吸 15/min | 心率 72     │ │
│ │ START: Delayed (🟡)                  │ │
│ │ [ 3D 骨架: OFF ○ ]  ← 按钮切换      │ │
│ └──────────────────────────────────────┘ │
│                                          │
│ ┌──────────────────────────────────────┐ │
│ │ 伤员 #2 | 呼吸 32/min | 心率 115    │ │
│ │ START: Immediate (🔴) ⚠️             │ │
│ │ [ 3D 骨架: ON  ● ]  ← 按钮切换      │ │
│ └──────────────────────────────────────┘ │
└──────────────────────────────────────────┘

实现:
1. 默认关闭 → 节省 RZ/V2H CPU, 确保核心分诊功能流畅
2. 点击按钮 → 启动 ONNX 推理 → 3D 骨架渲染
3. 再次点击 → 停止推理, 释放资源
4. 同时最多开启 2 个伤员的骨架 (避免 CPU 过载)
```

---

## 七、训练你自己的模型
如果你有标注数据（CSI + 真值姿态），可以微调：

```bash
# 1. 准备数据
data/
  my_scenes/
    S01/
      scene01/
        wifi_csi.npy
        wifi_csi_phase.npy
        gt_keypoints.npy

# 2. 修改配置
# crates/wifi-densepose-training/src/config.rs
num_subcarriers: 242      # C5 20MHz
native_subcarriers: 242   # 不插值
window_frames: 100

# 3. 训练
cargo run --release --bin train -- \
    --data-root ./data/my_scenes \
    --epochs 50 \
    --batch-size 4 \
    --lr 1e-4 \
    --pretrained densepose_pretrained.pt

# 4. 导出 ONNX
python tools/export_onnx.py \
    --input checkpoints/best.pt \
    --output competition/densepose_c5.onnx
```

---

*文档版本: v1.0 | 2026-05-06*
