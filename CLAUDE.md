# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

WCES (WiFi CSI Sensing-based Shelter Vital Signs Monitoring) is a competition entry for the 9th National College Embedded Chip & System Design Competition (Renesas track). It uses ESP32-C5 WiFi CSI sensing + Renesas RZ/G2L edge computing for contactless vital sign monitoring and START triage in field shelters.

**Hardware**: 3× ESP32-C5 sensor nodes → Renesas RZ/G2L (ARM64 Cortex-A55 ×2, 1GB DDR4) as main controller
**Language**: Rust (server), C (ESP32 firmware), JS/HTML (web UI)
**Competition status**: Active development, P0-P10f complete

## Build & Run Commands

### Rust server (main development target)

```bash
# Build everything
cd rust-server && cargo build --release

# Build for RZ/G2L (ARM64 cross-compile)
cargo build --target aarch64-unknown-linux-gnu --release

# Run in simulation mode (no hardware needed)
cargo run -p wifi-densepose-sensing-server -- --source simulate --ui-path ../docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080

# Run with hardware (auto-detect ESP32 UDP or fall back to simulate)
cargo run -p wifi-densepose-sensing-server -- --source auto --ui-path ../docs/triage-ui --bind-addr 0.0.0.0 --http-port 8080

# Run all workspace tests
cargo test --workspace

# Run a specific crate's tests
cargo test -p wifi-densepose-llm

# Run a single test
cargo test -p wifi-densepose-llm test_knowledge_base_loads_all_conditions

# Build WASM edge modules (excluded from workspace — build separately)
cargo build -p wifi-densepose-wasm-edge --target wasm32-unknown-unknown --release

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all -- --check
```

### ESP32-C5 firmware

```bash
cd firmware/esp32-c5-csi-node

# Set target and build
idf.py set-target esp32c5 && idf.py build

# Flash (per node — change port and node_id)
idf.py -p COM3 flash   # Node 1: COM3, Node 2: COM4, Node 3: COM5

# Monitor
idf.py -p COM3 monitor

# Provision via script (automates node_id config + flash)
python provision.py --chip esp32c5 --node-id 1 --port COM3
```

### Configuration management

```powershell
# Edit unified config
notepad wces.config.toml

# Apply config to each subsystem (Windows)
.\apply-config.ps1 -NodeId 1  # generates sdkconfig + updates deploy.sh

# Linux
./apply-config.sh
```

### Deploy to RZ/G2L

```bash
# One-click deploy (run on RZ/G2L after cross-compile)
ssh root@<RZ_IP> && cd /opt/WCES && ./deploy.sh  # RZ_IP from wces.config.toml [deploy]
```

## Architecture

### Layered data flow

```
ESP32-C5 (×3)                    RZ/G2L (sensing-server)              Browser
─────────────  ──────────────────────────────────────  ──────────────────
CSI采集         UDP:5005 → parse_esp32_frame()
                              → signal_processing (FFT, features)
                              → vital_signs (breathing/heart rate)
                              → mat_pipeline (START triage, tracking)
                              → edge_module_engine (19 modules)
                              → SensingUpdate (JSON)
                                                       WebSocket :8765 →
                                                       triage.html renders
```

### Workspace crate map (9 crates)

| Crate | Purpose |
|-------|---------|
| `wifi-densepose-core` | Base types shared across crates |
| `wifi-densepose-signal` | CSI signal processing (FFT, filters, features) |
| `wifi-densepose-vitals` | Vital sign extraction algorithms |
| `wifi-densepose-hardware` | CSI frame parsing (ADR-018 binary protocol) |
| `wifi-densepose-llm` | Medical Agent: cloud LLM integration + local template fallback + circuit breaker |
| `wifi-densepose-nn` | ONNX inference (DensePose 3D skeleton) |
| `wifi-densepose-mat` | START triage pipeline + casualty tracking |
| `wifi-densepose-sensing-server` | **Main binary crate** — Axum HTTP/WS server, UDP receiver, simulation, UI serving |
| `wifi-densepose-config` | Reserved namespace placeholder (deprecated) — actual config in `app_config.rs` |
| `wifi-densepose-wasm-edge` | WASM edge modules (68 `.rs` files, `wasm32-unknown-unknown`, **excluded from workspace**) |

### sensing-server internal module map (2026-05 refactor)

- `main.rs` — CLI args (clap) + state init + task spawning (1331 lines after refactor from 3868)
- `server.rs` — Axum HTTP/WS setup, graceful shutdown, API key auth middleware
- `types.rs` — `Esp32Frame`, `SensingUpdate`, `NodeInfo`, all constants
- `parser.rs` — ADR-018 binary frame parsing (`parse_esp32_frame`, `parse_esp32_vitals`, `parse_wasm_output`)
- `signal_processing.rs` — 14 pure signal functions (FFT, motion detection, signal field generation, person estimation)
- `state_ops.rs` — Stateful operations (smooth_and_classify, smooth_vitals, adaptive_override)
- `vital_signs.rs` — `VitalSignDetector` with FFT-based breathing rate (0.1-0.5Hz) and heart rate (0.8-2.0Hz)
- `mat_pipeline.rs` — `TriageEngine` implementing START protocol (Red/Yellow/Green/Black/Gray), casualty matching/tracking, deterioration detection, mass-casualty assessment, age estimation
- `edge_module_engine.rs` — 19 edge modules (gait, arrhythmia, respiratory distress, seizure, loitering, vibration, etc.)
- `handlers/` — `mod.rs`, `path_util.rs`, `ws.rs` (WebSocket), `routes.rs` (REST API), `model_routes.rs`, `recording_routes.rs`, `llm_routes.rs` (7 files)
- `tasks/` — `mod.rs`, `udp_receiver.rs` (hardware data), `simulated_data.rs` (synthetic CSI), `broadcast_tick.rs` (periodic rebroadcast) (4 files)
- `app_config.rs` — TOML config loading
- `rvf_container.rs` / `rvf_pipeline.rs` — RVF model format container + progressive loading
- `dataset.rs`, `embedding.rs`, `graph_transformer.rs`, `sparse_inference.rs`, `trainer.rs`, `adaptive_classifier.rs`, `sona.rs` — ML training/inference modules

### Concurrency model

`SharedState = Arc<RwLock<AppStateInner>>` — all tasks share this. The write lock is held in two phases:
1. Quick write: state mutations (frame history, vitals, triage)
2. Release lock, then do pure computation, then broadcast

Broadcast uses `tokio::sync::broadcast` channel for WebSocket push.

### Key protocols

- **ADR-018**: 20-byte binary header + IQ data pairs, Magic `0xC511_0001`, over UDP:5005
- **ADR-029**: Multi-channel hopping (2.4G + 5G bands)
- **ADR-040**: WASM edge crate excluded from workspace to not break `cargo test --workspace`
- **WebSocket `/ws/sensing`**: `SensingUpdate` JSON with `vital_signs`, `triage_update`, `wasm_alerts`, `signal_field`

### START Triage levels

| Level | Color | Criteria |
|-------|-------|----------|
| Immediate | Red | RR>30 or <10, HR>120 or <40 |
| Delayed | Yellow | Moderate abnormalities |
| Minor | Green | Normal vitals |
| Deceased | Black | No vital signs detected |
| Unknown | Gray | Insufficient signal quality |

## Configuration

Single source of truth: `wces.config.toml` at repo root. Covers firmware (Kconfig/NVS), server (CLI args), deploy, competition, and flash settings. Apply with `apply-config.ps1` (Windows) or `apply-config.sh` (Linux).

Environment variable `WCES_API_KEY` enables API key authentication on write endpoints (POST/DELETE). Leave unset for open access.

## Key Design Decisions

- **Simulation mode** runs the full pipeline with synthetic sine-wave CSI — no hardware required for UI/demo development
- **Medical Agent** uses Coordinator pattern: local signal processing + optional cloud LLM deep analysis with circuit breaker and graceful degradation to template-based analysis
- **WASM edge modules** are compiled as native Rust in competition mode (RZ/G2L hardware FPU, no WASM interpreter overhead) for 5-10× speedup
- **Lock contention** was the main source of bugs — write lock scope is deliberately minimized (two-phase: state mutation then computation outside lock)
- **No database required** for core operation — in-memory state with optional sled patient DB
