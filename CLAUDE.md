# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

WCES (WiFi CSI Sensing-based Shelter Vital Signs Monitoring) is a competition entry for the 9th National College Embedded Chip & System Design Competition (Renesas track). It uses ESP32-C5 WiFi CSI sensing + Renesas RZ/G2L edge computing for contactless vital sign monitoring and START triage in field shelters.

**Hardware**: 3× ESP32-C5 sensor nodes → Renesas RZ/G2L (ARM64 Cortex-A55 ×2, 1GB DDR4) as main controller
**Language**: Rust (server), C (ESP32 firmware), JS/HTML (web UI)
**Competition status**: Active development, all P0 bugs fixed, positioning overhaul complete

## Build & Run Commands

### Rust server — Native (development)

```bash
cd rust-server && cargo build --release
cargo run -p wifi-densepose-sensing-server -- --source simulate --ui-path ../docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080
```

### Rust server — Cross-compile to RZ/G2L (aarch64)

```bash
# In WSL Kali:
export PATH="/opt/poky/3.1.20/sysroots/x86_64-pokysdk-linux/usr/bin/aarch64-poky-linux:$PATH"
export CC_aarch64_unknown_linux_gnu=aarch64-poky-linux-gcc
export CXX_aarch64_unknown_linux_gnu=aarch64-poky-linux-g++
export AR_aarch64_unknown_linux_gnu=aarch64-poky-linux-ar
export CFLAGS_aarch64_unknown_linux_gnu="--sysroot=/opt/poky/3.1.20/sysroots/aarch64-poky-linux"
cd /root/WCES/rust-server
cargo build --target aarch64-unknown-linux-gnu --release -p wifi-densepose-sensing-server --no-default-features
```

**Critical**: `wifi-densepose-mat/Cargo.toml` requires `default-features = false` on `wifi-densepose-nn` to skip ONNX (ort-sys needs glibc 2.32+, Poky 3.1.20 has older glibc).

### ESP32-C5 firmware — Build & Flash

```powershell
cd D:\CODING\Repository\WCES

# Node 1
.\apply-config.ps1 -NodeId 1
cd firmware\esp32-c5-csi-node
idf.py fullclean && idf.py set-target esp32c5 && idf.py build
idf.py -p COM9 erase-flash flash

# Node 2
cd D:\CODING\Repository\WCES && .\apply-config.ps1 -NodeId 2
cd firmware\esp32-c5-csi-node
idf.py fullclean && idf.py set-target esp32c5 && idf.py build
idf.py -p COM10 erase-flash flash

# Node 3
cd D:\CODING\Repository\WCES && .\apply-config.ps1 -NodeId 3
cd firmware\esp32-c5-csi-node
idf.py fullclean && idf.py set-target esp32c5 && idf.py build
idf.py -p COM11 erase-flash flash
```

**Critical**: `erase-flash` is mandatory — clears NVS partition containing old `target_ip`, otherwise NVS value overrides sdkconfig.

### NVS-only IP update (fast, no rebuild)

```powershell
cd D:\CODING\Repository\WCES\firmware\esp32-c5-csi-node
python provision.py --port COM9 --target-ip <NEW_IP>
python provision.py --port COM10 --target-ip <NEW_IP>
python provision.py --port COM11 --target-ip <NEW_IP>
```

### Deploy to RZ/G2L

```bash
# Binary must be at D:\CODING\Repository\WCES\rust-server\sensing-server
# SCP to board:  scp sensing-server root@<IP>:/opt/WCES/rust-server/target/aarch64-unknown-linux-gnu/release/
# Start on board:
cd /opt/WCES && ./rust-server/target/aarch64-unknown-linux-gnu/release/sensing-server --source esp32 --ui-path ./docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080
```

## Architecture

### Complete data flow

```
ESP32-C5 (×3)                     RZ/G2L (sensing-server)                     Browser
─────────────  ───────────────────────────────────────────────  ─────────────────
CSI采集 → UDP:5005
  │
  ├─ parse_esp32_frame()          ← parser.rs (ADR-018 magic 0xC511_0001)
  ├─ signal_pipeline.process()    ← PhaseSanitize→Normalize→Hampel→MotionDetector
  ├─ extract_features_from_frame()← signal_processing.rs (14 functions)
  ├─ VitalsBridge.extract()       ← IIR bandpass + zero-crossing (breathing)
  │                                + temporal phase diff (heart rate, BUG 54)
  ├─ FieldBridge.feed()           ← SVD empty-room calibration → perturbation energy
  ├─ CIRBridge.process()          ← ISTA sparse CIR estimation → ToF ranging
  ├─ LocalizationBridge + TrackingBridge  ← multi-node triangulation + Kalman
  ├─ mat_pipeline::TriageEngine   ← START triage (Red/Yellow/Green/Black/Gray)
  │  ├─ generate_embedding        ← 8-dim biometric embedding for survivor matching
  │  ├─ match_or_create           ← cosine similarity matching (threshold 0.65)
  │  ├─ position: node centroid   ← EMA-smoothed weighted centroid (BUG 48)
  │  └─ calculate_triage          ← START protocol
  ├─ WhōFi+FieldBridge hybrid     ← Top-12 subcarrier variance (70%) + SVD perturbation (30%)
  │  ├─ adaptive baseline per node← auto-learns peak proximity
  │  └─ multi-node weighted centroid ← Σ(proximity[i] × pos[i]) / Σ(proximity[i])
  ├─ derive_pose_from_sensing()   ← survivor-driven person count (BUG 47)
  ├─ derive_single_person_pose()  ← uses survivor EMA position (BUG 52)
  ├─ edge_module_engine           ← 10 edge analytics modules
  └─ SensingUpdate JSON
                                  WebSocket :8765 →
                                  triage.html renders
```

### Workspace crate map (9 crates)

| Crate | Purpose |
|-------|---------|
| `wifi-densepose-core` | Base types shared across crates |
| `wifi-densepose-signal` | CSI signal processing (FFT, filters, features, field_model, CIR, motion) |
| `wifi-densepose-vitals` | Vital sign extraction (BreathingExtractor, HeartRateExtractor, CsiVitalPreprocessor) |
| `wifi-densepose-hardware` | CSI frame parsing (ADR-018 binary protocol) |
| `wifi-densepose-llm` | Medical Agent: cloud LLM + local template fallback + circuit breaker |
| `wifi-densepose-nn` | ONNX inference (DensePose 3D skeleton) — NOT used by sensing-server |
| `wifi-densepose-mat` | START triage pipeline + casualty tracking |
| `wifi-densepose-sensing-server` | **Main binary crate** — Axum HTTP/WS server, UDP receiver, all bridges |
| `wifi-densepose-config` | Deprecated placeholder (8 lines) — actual config in `app_config.rs` |
| `wifi-densepose-wasm-edge` | WASM edge modules (68 files, wasm32, **excluded from workspace**) |

### sensing-server internal modules (29 source files + handlers/ + tasks/)

**Core pipeline:**
- `main.rs` — CLI args (clap) + state init + task spawning
- `server.rs` — Axum HTTP/WS setup, graceful shutdown, API key auth
- `types.rs` — `Esp32Frame`, `SensingUpdate`, `NodeInfo`, all constants

**Data ingestion & parsing:**
- `parser.rs` — ADR-018 binary frame parsing (3 packet types)
- `tasks/udp_receiver.rs` — **ESP32 path**: parse → process → triage → position → broadcast
- `tasks/simulated_data.rs` — **Simulation path**: synthetic sine-wave CSI
- `tasks/broadcast_tick.rs` — Periodic rebroadcast + alert drain (0.5 Hz)

**Signal processing:**
- `signal_pipeline.rs` — PhaseSanitizer→Normalizer→HampelFilter→MotionDetector→CoherenceGate
- `signal_processing.rs` — 14 functions: FFT, feature extraction, signal field, pose generation
- `state_ops.rs` — Stateful smoothing: `smooth_and_classify`, `smooth_vitals`, `adaptive_override` **(simulation only)**
- `vital_signs.rs` — `VitalSignDetector` with FFT (Goertzel) — **simulation only**, ESP32 uses VitalsBridge

**Vital signs (ESP32):**
- `vitals_bridge.rs` — Bridges `wifi-densepose-vitals` crate: `BreathingExtractor` (IIR bandpass + zero-crossing, 30s window) + `HeartRateExtractor` (temporal phase diff, 30s window, BUG 54)
- `detection_bridge.rs` — Legacy MAT crate bridge, **not used** (removed from ESP32 path)

**Positioning:**
- `field_bridge.rs` — SVD empty-room calibration (~30s), extracts perturbation energy
- `field_localize.rs` — signal_field peak extraction and world-coordinate mapping (not currently used in ESP32 path; WhōFi hybrid used instead)
- `localization_bridge.rs` — Multi-node RSSI+CIR triangulation
- `tracking_bridge.rs` — Kalman filter + fingerprint re-ID
- `cir_bridge.rs` — ISTA sparse CIR estimation → ToF ranging

**Triage & analytics:**
- `mat_pipeline.rs` — `TriageEngine`: START protocol, survivor matching (cosine similarity, threshold 0.65), position smoothing (EMA), deterioration detection, casualty assessment
- `edge_module_engine.rs` — 10 edge modules (gait, arrhythmia, respiratory distress, seizure, etc.)
- `alerting_bridge.rs` — Structured alert generation + drain

**ML / training (not used in competition):**
- `trainer.rs`, `dataset.rs`, `embedding.rs`, `graph_transformer.rs`, `sparse_inference.rs`, `sona.rs`, `adaptive_classifier.rs`

**Model storage:**
- `rvf_container.rs`, `rvf_pipeline.rs` — RVF model format

**Web:**
- `handlers/` — `ws.rs` (WebSocket), `routes.rs`, `model_routes.rs`, `recording_routes.rs`, `llm_routes.rs`
- `app_config.rs` — TOML config loading

### Concurrency model

`SharedState = Arc<RwLock<AppStateInner>>` — all tasks share this. Two-phase write:
1. Quick write: state mutations (frame history, vitals, triage, bridges)
2. Release lock → pure computation (signal field, pose, field peaks) → broadcast

Broadcast uses `tokio::sync::broadcast::channel(2048)` for WebSocket push.

### Key protocols

- **ADR-018**: 20-byte binary header + IQ data pairs, Magic `0xC511_0001`, over UDP:5005
- **ADR-029**: Multi-channel hopping (2.4G + 5G bands)
- **ADR-040**: WASM edge crate excluded from workspace
- **WebSocket `/ws/sensing`**: `SensingUpdate` JSON with `vital_signs`, `triage_update`, `persons`, `signal_field`

### START Triage levels (matches START protocol)

| Level | Color | Criteria |
|-------|-------|----------|
| Immediate | Red | RR>30 or <10, HR>120 or <40 |
| Delayed | Yellow | Moderate abnormalities, stable |
| Minor | Green | Normal vitals + ambulatory |
| Deceased | Black | No vital signs **detected after adequate data** |
| Unknown | Gray | Insufficient data — e.g. VitalsBridge IIR warmup (BUG 50: was Deceased) |

### Positioning (WhōFi + FieldBridge hybrid)

Per-frame:
1. Top-12 subcarrier temporal variance mean → raw proximity (WhōFi)
2. FieldBridge SVD perturbation energy → secondary proximity (FieldBridge)
3. Blend: 70% WhōFi + 30% FieldBridge → EMA-smoothed per-node proximity [0,1]
4. Multi-node weighted centroid: `Σ(proximity[i] × node_pos[i]) / Σ(proximity[i])`
5. Adaptive baseline auto-calibrates per node (no hardcoded thresholds)

Survivors matched via 8-dim biometric embedding (vitals + RSSI + discretized motion, cosine similarity threshold 0.65). Position is EMA-smoothed centroid with stagger for multiple survivors.

### Vital signs algorithm (ESP32 path)

- **Breathing**: IIR bandpass (0.1-0.5 Hz) on amplitude residuals from CsiVitalPreprocessor → zero-crossing rate → BPM. 30s window, requires ~30s IIR warmup.
- **Heart rate**: Temporal phase difference formula (BUG 54: was amplitude residuals, which is ~10× weaker for cardiac signal). IIR bandpass (0.8-2.0 Hz) → autocorrelation peak → BPM. 30s window.
- **Sample rate**: Dynamically measured via EMA of frame arrival interval. Propagated to extractor filter coefficients via `set_sample_rate()` (BUG 49).

## Configuration

Single source: `wces.config.toml` at repo root. Covers firmware (Kconfig/NVS), server ([server] runtime section), Medical Agent, edge modules, deploy, competition, flash. Apply firmware config with `apply-config.ps1`.

Server runtime config ([server] sections) takes effect on restart, no rebuild needed.

## Key Design Decisions

- **Simulation mode**: Full pipeline with synthetic sine-wave CSI — no hardware required for UI/demo
- **Medical Agent**: Coordinator pattern — local signal processing + optional cloud LLM with circuit breaker and graceful degradation
- **WASM edge modules**: Compiled native Rust in competition mode (RZ/G2L FPU, 5-10× speedup vs WASM interpreter)
- **Two-phase lock**: Write lock minimized (state mutations only), pure computation outside lock prevents contention
- **No ONNX dependency**: `--no-default-features` + `default-features = false` on NN crate skips entire ort/ort-sys chain
- **ESP-IDF v6.0.1**: Full C5 CSI support, `WIFI_BAND_6G` removed, `rx_ant` removed from `esp_wifi_rxctrl_t`
