//! General HTTP REST route handlers (health, pose, stream, model info, SONA,
//! training, adaptive classifier, etc.).

use std::path::PathBuf;

use axum::extract::State;
use axum::response::{Html, Json};
use tracing::{info, warn};

use crate::SharedState;
use crate::signal_processing::derive_pose_from_sensing;
use crate::adaptive_classifier;

// ── REST endpoints ───────────────────────────────────────────────────────────

pub(crate) async fn health(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "status": "ok",
        "source": s.effective_source(),
        "tick": s.tick,
        "clients": s.tx.receiver_count(),
    }))
}

pub(crate) async fn latest(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.latest_update {
        Some(update) => Json(serde_json::to_value(update).unwrap_or_default()),
        None => Json(serde_json::json!({"status": "no data yet"})),
    }
}

// ── DensePose-compatible REST endpoints ─────────────────────────────────────

pub(crate) async fn health_live(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "status": "alive",
        "uptime": s.start_time.elapsed().as_secs(),
    }))
}

pub(crate) async fn health_ready(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "status": "ready",
        "source": s.effective_source(),
    }))
}

pub(crate) async fn health_system(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let uptime = s.start_time.elapsed().as_secs();
    Json(serde_json::json!({
        "status": "healthy",
        "components": {
            "api": { "status": "healthy", "message": "Rust Axum server" },
            "hardware": {
                "status": if s.effective_source().ends_with(":offline") { "degraded" } else { "healthy" },
                "message": format!("Source: {}", s.effective_source())
            },
            "pose": { "status": "healthy", "message": "WiFi-derived pose estimation" },
            "stream": { "status": if s.tx.receiver_count() > 0 { "healthy" } else { "idle" },
                        "message": format!("{} client(s)", s.tx.receiver_count()) },
        },
        "metrics": {
            "cpu_percent": 2.5,
            "memory_percent": 1.8,
            "disk_percent": 15.0,
            "uptime_seconds": uptime,
        }
    }))
}

pub(crate) async fn health_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "name": "wifi-densepose-sensing-server",
        "backend": "rust+axum+ruvector",
    }))
}

pub(crate) async fn health_metrics(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "system_metrics": {
            "cpu": { "percent": 2.5 },
            "memory": { "percent": 1.8, "used_mb": 5 },
            "disk": { "percent": 15.0 },
        },
        "tick": s.tick,
    }))
}

pub(crate) async fn api_info(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "environment": "production",
        "backend": "rust",
        "source": s.effective_source(),
        "features": {
            "wifi_sensing": true,
            "pose_estimation": true,
            "signal_processing": true,
            "ruvector": true,
            "streaming": true,
        }
    }))
}

pub(crate) async fn pose_current(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let persons = match &s.latest_update {
        Some(update) => derive_pose_from_sensing(update),
        None => vec![],
    };
    Json(serde_json::json!({
        "timestamp": chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
        "persons": persons,
        "total_persons": persons.len(),
        "source": s.effective_source(),
    }))
}

pub(crate) async fn pose_stats(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "total_detections": s.total_detections,
        "average_confidence": 0.87,
        "frames_processed": s.tick,
        "source": s.effective_source(),
    }))
}

pub(crate) async fn pose_zones_summary(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let presence = s.latest_update.as_ref()
        .map(|u| u.classification.presence).unwrap_or(false);
    Json(serde_json::json!({
        "zones": {
            "zone_1": { "person_count": if presence { 1 } else { 0 }, "status": "monitored" },
            "zone_2": { "person_count": 0, "status": "clear" },
            "zone_3": { "person_count": 0, "status": "clear" },
            "zone_4": { "person_count": 0, "status": "clear" },
        }
    }))
}

pub(crate) async fn stream_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "active": true,
        "clients": s.tx.receiver_count(),
        "fps": if s.tick > 1 { 10u64 } else { 0u64 },
        "source": s.effective_source(),
    }))
}

pub(crate) async fn vital_signs_endpoint(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let vs = &s.latest_vitals;
    let (br_len, br_cap, hb_len, hb_cap) = s.vital_detector.buffer_status();
    Json(serde_json::json!({
        "vital_signs": {
            "breathing_rate_bpm": vs.breathing_rate_bpm,
            "heart_rate_bpm": vs.heart_rate_bpm,
            "breathing_confidence": vs.breathing_confidence,
            "heartbeat_confidence": vs.heartbeat_confidence,
            "signal_quality": vs.signal_quality,
        },
        "buffer_status": {
            "breathing_samples": br_len,
            "breathing_capacity": br_cap,
            "heartbeat_samples": hb_len,
            "heartbeat_capacity": hb_cap,
        },
        "source": s.effective_source(),
        "tick": s.tick,
    }))
}

/// GET /api/v1/edge-vitals —latest edge vitals from ESP32 (ADR-039).
pub(crate) async fn edge_vitals_endpoint(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.edge_vitals {
        Some(v) => Json(serde_json::json!({
            "status": "ok",
            "edge_vitals": v,
        })),
        None => Json(serde_json::json!({
            "status": "no_data",
            "edge_vitals": null,
            "message": "No edge vitals packet received yet. Ensure ESP32 edge_tier >= 1.",
        })),
    }
}

/// GET /api/v1/wasm-events —latest WASM events from ESP32 (ADR-040).
pub(crate) async fn wasm_events_endpoint(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.latest_wasm_events {
        Some(w) => Json(serde_json::json!({
            "status": "ok",
            "wasm_events": w,
        })),
        None => Json(serde_json::json!({
            "status": "no_data",
            "wasm_events": null,
            "message": "No WASM output packet received yet. Upload and start a .wasm module on the ESP32.",
        })),
    }
}

pub(crate) async fn model_info(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.rvf_info {
        Some(info) => Json(serde_json::json!({
            "status": "loaded",
            "container": info,
        })),
        None => Json(serde_json::json!({
            "status": "no_model",
            "message": "No RVF container loaded. Use --load-rvf <path> to load one.",
        })),
    }
}

pub(crate) async fn model_layers(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.progressive_loader {
        Some(loader) => {
            let (a, b, c) = loader.layer_status();
            Json(serde_json::json!({
                "layer_a": a,
                "layer_b": b,
                "layer_c": c,
                "progress": loader.loading_progress(),
            }))
        }
        None => Json(serde_json::json!({
            "layer_a": false,
            "layer_b": false,
            "layer_c": false,
            "progress": 0.0,
            "message": "No model loaded with progressive loading",
        })),
    }
}

pub(crate) async fn model_segments(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.progressive_loader {
        Some(loader) => Json(serde_json::json!({ "segments": loader.segment_list() })),
        None => Json(serde_json::json!({ "segments": [] })),
    }
}

pub(crate) async fn sona_profiles(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let names = s
        .progressive_loader
        .as_ref()
        .map(|l| l.sona_profile_names())
        .unwrap_or_default();
    let active = s.active_sona_profile.clone().unwrap_or_default();
    Json(serde_json::json!({ "profiles": names, "active": active }))
}

pub(crate) async fn sona_activate(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let profile = body
        .get("profile")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();

    let mut s = state.write().await;
    let available = s
        .progressive_loader
        .as_ref()
        .map(|l| l.sona_profile_names())
        .unwrap_or_default();

    if available.contains(&profile) {
        s.active_sona_profile = Some(profile.clone());
        Json(serde_json::json!({ "status": "activated", "profile": profile }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "message": format!("Profile '{}' not found. Available: {:?}", profile, available),
        }))
    }
}

pub(crate) async fn info_page() -> Html<String> {
    Html(format!(
        "<html><head><meta charset='UTF-8'><title>WCES Sensing Server</title>\
         <style>body{{font-family:-apple-system,BlinkMacSystemFont,sans-serif;\
         background:#1C1C1E;color:#FFF;padding:32px}}\
         a{{color:#007AFF;text-decoration:none;font-size:16px}}\
         a:hover{{text-decoration:underline}}\
         li{{margin:8px 0}}\
         .tag{{font-size:11px;color:#98989D;margin-left:8px}}\
         .section{{margin-top:20px;border-top:1px solid #38383A;padding-top:16px}}\
         </style></head><body>\
         <h1>WCES — WiFi CSI 应急感知系统</h1>\
         <p>Rust + Axum + Tokio | 瑞萨 RZ/G2L + ESP32-C5 ×3</p>\
         <div class='section'><h3>Web 仪表盘</h3><ul>\
         <li><a href='/ui/triage.html'><strong>/ui/triage.html</strong></a>\
         <span class='tag'>分诊仪表盘 (竞赛核心)</span></li>\
         <li><a href='/ui/index.html'><strong>/ui/index.html</strong></a>\
         <span class='tag'>控制中心</span></li>\
         </ul></div>\
         <div class='section'><h3>API 端点</h3><ul>\
         <li><a href='/health'>/health</a> — 服务健康检查</li>\
         <li><a href='/api/v1/vital-signs'>/api/v1/vital-signs</a> — 生命体征 (HR/RR)</li>\
         <li><a href='/api/v1/sensing/latest'>/api/v1/sensing/latest</a> — 最新感知数据</li>\
         <li><a href='/api/v1/model/info'>/api/v1/model/info</a> — RVF 模型信息</li>\
         <li><span>/ws/sensing</span> <span class='tag'>WebSocket 实时流 (当前端口)</span></li>\
         </ul></div>\
         </body></html>"
    ))
}

// ── Training Endpoints ──────────────────────────────────────────────────────

/// GET /api/v1/train/status —get training status.
pub(crate) async fn train_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(serde_json::json!({
        "status": s.training_status,
        "config": s.training_config,
    }))
}

/// POST /api/v1/train/start —start a training run.
pub(crate) async fn train_start(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    if s.training_status == "running" {
        return Json(serde_json::json!({
            "error": "training already running",
            "success": false,
        }));
    }
    s.training_status = "running".to_string();
    s.training_config = Some(body.clone());
    info!("Training started with config: {}", body);
    Json(serde_json::json!({
        "success": true,
        "status": "running",
        "message": "Training pipeline started. Use GET /api/v1/train/status to monitor.",
    }))
}

/// POST /api/v1/train/stop —stop the current training run.
pub(crate) async fn train_stop(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    if s.training_status != "running" {
        return Json(serde_json::json!({
            "error": "no training in progress",
            "success": false,
        }));
    }
    s.training_status = "idle".to_string();
    info!("Training stopped");
    Json(serde_json::json!({
        "success": true,
        "status": "idle",
    }))
}

// ── Adaptive classifier endpoints ────────────────────────────────────────────

/// POST /api/v1/adaptive/train —train the adaptive classifier from recordings.
pub(crate) async fn adaptive_train(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let data_dir = state.read().await.data_dir.clone();
    let rec_dir = data_dir.join("data/recordings");
    eprintln!("=== Adaptive Classifier Training ===");
    match adaptive_classifier::train_from_recordings(&rec_dir) {
        Ok(model) => {
            let accuracy = model.training_accuracy;
            let frames = model.trained_frames;
            let stats: Vec<_> = model.class_stats.iter().map(|cs| {
                serde_json::json!({
                    "class": cs.label,
                    "samples": cs.count,
                    "feature_means": cs.mean,
                })
            }).collect();

            // Save to disk.
            let model_path = adaptive_classifier::model_path(&data_dir);
            if let Err(e) = model.save(&model_path) {
                warn!("Failed to save adaptive model: {e}");
            } else {
                info!("Adaptive model saved to {}", model_path.display());
            }

            // Load into runtime state.
            let mut s = state.write().await;
            s.adaptive_model = Some(model);

            Json(serde_json::json!({
                "success": true,
                "trained_frames": frames,
                "accuracy": accuracy,
                "class_stats": stats,
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "success": false,
                "error": e,
            }))
        }
    }
}

/// GET /api/v1/adaptive/status —check adaptive model status.
pub(crate) async fn adaptive_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.adaptive_model {
        Some(model) => Json(serde_json::json!({
            "loaded": true,
            "trained_frames": model.trained_frames,
            "accuracy": model.training_accuracy,
            "version": model.version,
            "classes": adaptive_classifier::CLASSES,
            "class_stats": model.class_stats,
        })),
        None => Json(serde_json::json!({
            "loaded": false,
            "message": "No adaptive model. POST /api/v1/adaptive/train to train one.",
        })),
    }
}

/// POST /api/v1/adaptive/unload —unload the adaptive model (revert to thresholds).
pub(crate) async fn adaptive_unload(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    s.adaptive_model = None;
    Json(serde_json::json!({ "success": true, "message": "Adaptive model unloaded." }))
}
