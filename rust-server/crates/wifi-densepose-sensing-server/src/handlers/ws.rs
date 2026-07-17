//! WebSocket message handlers for sensing and pose streams.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::{info, warn};

use std::sync::LazyLock;
use std::time::Duration;

use crate::SharedState;
use crate::types::{BoundingBox, PersonDetection, PoseKeypoint, SensingUpdate};
use crate::signal_processing::derive_pose_from_sensing;

use wifi_densepose_llm::{
    PatientRecord, AgentVitalSnapshot, StructuredContext, TriggerSource, TrendSummary,
};
use crate::edge_module_engine::EdgeAlert;

// LOW-3 fix: optional WebSocket authentication via WCES_WS_TOKEN env var.
// If the env var is unset, auth is disabled (backward-compatible with
// local-network deployments — the historical default). If set, clients
// must connect with `?token=<value>` in the query string. Constant-time
// comparison prevents timing side-channels on the token check.
static WS_AUTH_TOKEN: LazyLock<Option<String>> =
    LazyLock::new(|| std::env::var("WCES_WS_TOKEN").ok().filter(|s| !s.is_empty()));

/// Query string parameters extracted from the WebSocket upgrade request.
/// Currently only `token` is consumed (LOW-3 auth).
#[derive(Deserialize)]
pub(crate) struct WsAuthQuery {
    token: Option<String>,
}

/// Constant-time string comparison to prevent timing attacks on the token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) { diff |= x ^ y; }
    diff == 0
}

// ── Sensing WebSocket handler ──────────────────────────────────────────────────

pub(crate) async fn ws_sensing_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsAuthQuery>,
    State(state): State<SharedState>,
) -> Response {
    // LOW-3 fix: enforce optional token auth before upgrading.
    // Returning Err(StatusCode) BEFORE calling ws.on_upgrade() causes axum
    // to short-circuit with an HTTP 401 and NOT perform the upgrade.
    if let Some(expected) = &*WS_AUTH_TOKEN {
        let provided = query.token.as_deref().unwrap_or("");
        if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            warn!("WebSocket upgrade rejected: missing/invalid token");
            return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
        }
    }
    ws.on_upgrade(|socket| handle_ws_client(socket, state)).into_response()
}

pub(crate) async fn handle_ws_client(mut socket: WebSocket, state: SharedState) {
    let mut rx = {
        let s = state.read().await;
        s.tx.subscribe()
    };

    info!("WebSocket client connected (sensing)");

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(json) => {
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WS sensing client lagged by {} messages, resuming from latest", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                            match msg.get("type").and_then(|v| v.as_str()) {
                                Some("ping") => {
                                    let pong = serde_json::json!({"type":"pong"});
                                    let _ = socket.send(Message::Text(pong.to_string().into())).await;
                                }
                                Some("patient_register") => {
                                    if let Some(ref engine) = {
                                        let s = state.read().await;
                                        s.llm_engine.clone()
                                    } {
                                        let pid = msg["patient_id"].as_str().unwrap_or("UNKNOWN");
                                        let age = msg["age"].as_u64().map(|a| a as u8);
                                        let gender = msg["gender"].as_str().unwrap_or("unknown");
                                        let name = msg["name"].as_str().map(|n| n.to_string());
                                        let node_id = msg["node_id"].as_u64().map(|n| n as u8);
                                        let pre_existing: Vec<String> = msg["pre_existing"]
                                            .as_array()
                                            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                            .unwrap_or_default();
                                        let chief_complaint = msg["chief_complaint"].as_str().map(|s| s.to_string());
                                        let medications: Vec<String> = msg["medications"]
                                            .as_array()
                                            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                            .unwrap_or_default();
                                        let allergies: Vec<String> = msg["allergies"]
                                            .as_array()
                                            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                            .unwrap_or_default();
                                        let notes = msg["notes"].as_str().map(|s| s.to_string());

                                        let mut record = PatientRecord::new(pid.to_string());
                                        record.age = age;
                                        record.gender = match gender {
                                            "male" => Some(wifi_densepose_llm::Gender::Male),
                                            "female" => Some(wifi_densepose_llm::Gender::Female),
                                            _ => Some(wifi_densepose_llm::Gender::Other),
                                        };
                                        record.name = name;
                                        record.node_id = node_id;
                                        record.pre_existing = pre_existing;
                                        record.chief_complaint = chief_complaint;
                                        record.medications = medications;
                                        record.allergies = allergies;
                                        record.notes = notes;

                                        if let Err(e) = engine.register_patient(record).await {
                                            warn!("Failed to register patient: {}", e);
                                        } else {
                                            let ack = serde_json::json!({"type": "patient_registered", "patient_id": pid});
                                            let _ = socket.send(Message::Text(ack.to_string().into())).await;
                                        }
                                    }
                                }
                                Some("agent_analyze_request") => {
                                    let patient_id = msg["patient_id"].as_str().unwrap_or("UNKNOWN");
                                    let pid_str = patient_id.to_string();

                                    // HIGH-2 fix: previously `patient_id.parse::<u32>().unwrap_or(1)`
                                    // silently defaulted to 1 because survivor IDs are strings like
                                    // "SURV-001". Now we look up the survivor in the latest triage
                                    // update to obtain its actual node_id (u8 → u32). Falls back to
                                    // parsing the numeric suffix of "SURV-NNN" (e.g. "SURV-001" → 1),
                                    // then to 1 only if all else fails. This preserves the LLM
                                    // engine's u32 patient_id contract without churning every
                                    // downstream type.
                                    let patient_id_num: u32 = {
                                        let s = state.read().await;
                                        s.latest_update.as_ref()
                                            .and_then(|u| u.triage_update.as_ref())
                                            .and_then(|t| t.survivors.iter()
                                                .find(|surv| surv.id == patient_id)
                                                .map(|surv| surv.node_id as u32))
                                            .or_else(|| {
                                                pid_str.split('-')
                                                    .nth(1)
                                                    .and_then(|n| n.parse::<u32>().ok())
                                            })
                                            .unwrap_or(1)
                                    };

                                    // Single read lock: capture medical_agent (primary path),
                                    // llm_engine (fallback path), raw vitals (for fallback),
                                    // structured-context inputs (vitals_snapshot, kb_matches), and tx.
                                    let (engine, agent, br, hr, motion, quality,
                                         triage_label, alerts, vitals_snapshot, kb_matches, tx) = {
                                        let s = state.read().await;
                                        let triage = s.latest_update.as_ref()
                                            .and_then(|u| u.triage_update.as_ref())
                                            .and_then(|t| t.survivors.iter()
                                                .find(|surv| surv.id == patient_id)
                                                .map(|surv| surv.triage.clone()))
                                            .unwrap_or_else(|| "Unknown".to_string());
                                        let a: Vec<String> = s.latest_update.as_ref()
                                            .and_then(|u| u.wasm_alerts.as_ref())
                                            .map(|alerts: &Vec<EdgeAlert>| alerts.iter().map(|a| a.event_name.clone()).collect())
                                            .unwrap_or_default();
                                        let vitals = &s.latest_vitals;
                                        let vitals_snapshot = AgentVitalSnapshot {
                                            breathing_rate_bpm: vitals.breathing_rate_bpm.map(|v| v as f32),
                                            heart_rate_bpm: vitals.heart_rate_bpm.map(|v| v as f32),
                                            breathing_confidence: vitals.breathing_confidence as f32,
                                            heartbeat_confidence: vitals.heartbeat_confidence as f32,
                                            signal_quality: vitals.signal_quality as f32,
                                            motion_class: Some(if s.smoothed_motion > 0.6 { "active" } else if s.smoothed_motion > 0.2 { "present_still" } else { "still" }.into()),
                                            person_count_estimate: Some(1),
                                            rssi: s.rssi_history.back().map(|&v| v as i16),
                                        };
                                        let kb_matches = s.medical_kb.match_vitals(&vitals_snapshot);
                                        (
                                            s.llm_engine.clone(),
                                            s.medical_agent.clone(),
                                            vitals.breathing_rate_bpm,
                                            vitals.heart_rate_bpm,
                                            s.smoothed_motion,
                                            vitals.signal_quality,
                                            triage,
                                            a,
                                            vitals_snapshot,
                                            kb_matches,
                                            s.tx.clone(),
                                        )
                                    };

                                    // Clone triage_label and alerts for the LlmAnalysisEngine
                                    // fallback path (the originals are moved into StructuredContext).
                                    let triage_label_fb = triage_label.clone();
                                    let alerts_fb = alerts.clone();

                                    let ctx = StructuredContext {
                                        patient_id: patient_id_num,
                                        node_id: 1,
                                        vitals_current: vitals_snapshot,
                                        vitals_trend_1min: TrendSummary {
                                            direction: wifi_densepose_llm::TrendDirection::Stable,
                                            delta: 0.0, delta_pct: 0.0,
                                            anomaly_score: 1.0, data_points: 10,
                                        },
                                        vitals_trend_5min: TrendSummary {
                                            direction: wifi_densepose_llm::TrendDirection::Stable,
                                            delta: 0.0, delta_pct: 0.0,
                                            anomaly_score: 1.0, data_points: 50,
                                        },
                                        triage_current: triage_label,
                                        triage_trajectory: vec![],
                                        patient_history: None,
                                        recent_alerts: alerts,
                                        kb_matches,
                                        triggered_by: TriggerSource::ManualRequest { patient_id: patient_id_num },
                                        built_at_ms: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64,
                                    };

                                    tokio::spawn(async move {
                                        // 方案 A: 优先调用 MedicalAgent.analyze()（非流式），
                                        // 结果作为单条 agent_analysis_complete 消息发送。
                                        // 若锁获取超时(1s)或 analyze 超时(30s)，回退到
                                        // LlmAnalysisEngine 流式路径。
                                        let agent_result = match tokio::time::timeout(
                                            Duration::from_secs(1),
                                            agent.lock(),
                                        ).await {
                                            Ok(mut agent_guard) => {
                                                match tokio::time::timeout(
                                                    Duration::from_secs(30),
                                                    agent_guard.analyze(ctx),
                                                ).await {
                                                    Ok(r) => Some(r),
                                                    Err(_elapsed) => {
                                                        warn!("MedicalAgent analyze timed out for patient {}, falling back to LlmAnalysisEngine", patient_id_num);
                                                        None
                                                    }
                                                }
                                            }
                                            Err(_elapsed) => {
                                                warn!("MedicalAgent lock acquisition timed out for patient {}, falling back to LlmAnalysisEngine", patient_id_num);
                                                None
                                            }
                                        };

                                        if let Some(result) = agent_result {
                                            if !result.text.is_empty() {
                                                let json = serde_json::json!({
                                                    "type": "agent_analysis_complete",
                                                    "patient_id": result.patient_id,
                                                    "text": result.text,
                                                    "source": result.source,
                                                    "degrade_level": result.degrade_level,
                                                    "risk_adjustment": result.risk_adjustment,
                                                    "generated_at_ms": result.generated_at_ms,
                                                    "trigger": "ws_request",
                                                });
                                                if let Ok(json_str) = serde_json::to_string(&json) {
                                                    let _ = tx.send(json_str);
                                                }
                                            } else {
                                                let json = serde_json::json!({
                                                    "type": "agent_analysis_error",
                                                    "patient_id": patient_id_num,
                                                    "error": "MedicalAgent returned empty result",
                                                });
                                                if let Ok(json_str) = serde_json::to_string(&json) {
                                                    let _ = tx.send(json_str);
                                                }
                                            }
                                            return;
                                        }

                                        // Fallback: LlmAnalysisEngine 流式路径（当 medical_agent
                                        // 锁获取超时或 analyze 超时时使用）。
                                        if let Some(engine) = engine {
                                            if let Some(mut rx) = engine.trigger_analysis_streaming(
                                                &pid_str, br, hr, motion, quality,
                                                &triage_label_fb, &alerts_fb,
                                            ).await {
                                                while let Ok(token) = rx.recv().await {
                                                    let json = serde_json::json!({
                                                        "type": if token.is_complete { "agent_analysis_complete" } else { "agent_stream" },
                                                        "patient_id": token.survivor_id,
                                                        "text": token.text,
                                                        "token_index": token.token_index,
                                                    });
                                                    if let Ok(json_str) = serde_json::to_string(&json) {
                                                        let _ = tx.send(json_str);
                                                    }
                                                }
                                            }
                                        } else {
                                            // 既无 MedicalAgent 结果也无 LlmAnalysisEngine — 发送明确错误
                                            let json = serde_json::json!({
                                                "type": "agent_analysis_error",
                                                "patient_id": patient_id_num,
                                                "error": "MedicalAgent unavailable and LlmAnalysisEngine not configured",
                                            });
                                            if let Ok(json_str) = serde_json::to_string(&json) {
                                                let _ = tx.send(json_str);
                                            }
                                        }
                                    });
                                }
                                _ => {} // ignore unknown messages
                            }
                        }
                    }
                    _ => {} // ignore non-text messages
                }
            }
        }
    }

    info!("WebSocket client disconnected (sensing)");
}

// ── Pose WebSocket handler (sends pose_data messages for Live Demo) ──────────

pub(crate) async fn ws_pose_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_pose_client(socket, state))
}

pub(crate) async fn handle_ws_pose_client(mut socket: WebSocket, state: SharedState) {
    let mut rx = {
        let s = state.read().await;
        s.tx.subscribe()
    };

    info!("WebSocket client connected (pose)");

    // Send connection established message
    let conn_msg = serde_json::json!({
        "type": "connection_established",
        "payload": { "status": "connected", "backend": "rust+ruvector" }
    });
    let _ = socket.send(Message::Text(conn_msg.to_string().into())).await;

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(json) => {
                        // Parse the sensing update and convert to pose format
                        if let Ok(sensing) = serde_json::from_str::<SensingUpdate>(&json) {
                            if sensing.msg_type == "sensing_update" {
                                // Determine pose estimation mode for the UI indicator.
                                // "model_inference"    —a trained RVF model is loaded.
                                // "signal_derived"     —keypoints estimated from raw CSI features.
                                let model_loaded = {
                                    let s = state.read().await;
                                    s.model_loaded
                                };
                                let pose_source = if model_loaded {
                                    "model_inference"
                                } else {
                                    "signal_derived"
                                };

                                let persons = if model_loaded {
                                    // When a trained model is loaded, prefer its keypoints if present.
                                    sensing.pose_keypoints.as_ref().map(|kps| {
                                        let kp_names = [
                                            "nose","left_eye","right_eye","left_ear","right_ear",
                                            "left_shoulder","right_shoulder","left_elbow","right_elbow",
                                            "left_wrist","right_wrist","left_hip","right_hip",
                                            "left_knee","right_knee","left_ankle","right_ankle",
                                        ];
                                        let keypoints: Vec<PoseKeypoint> = kps.iter()
                                            .enumerate()
                                            .map(|(i, kp)| PoseKeypoint {
                                                name: kp_names.get(i).unwrap_or(&"unknown").to_string(),
                                                x: kp[0], y: kp[1], z: kp[2], confidence: kp[3],
                                            })
                                            .collect();
                                        vec![PersonDetection {
                                            id: 1,
                                            confidence: sensing.classification.confidence,
                                            bbox: BoundingBox { x: 260.0, y: 150.0, width: 120.0, height: 220.0 },
                                            keypoints,
                                            zone: "zone_1".into(),
                                        }]
                                    }).unwrap_or_else(|| derive_pose_from_sensing(&sensing))
                                } else {
                                    derive_pose_from_sensing(&sensing)
                                };

                                let pose_msg = serde_json::json!({
                                    "type": "pose_data",
                                    "zone_id": "zone_1",
                                    "timestamp": sensing.timestamp,
                                    "payload": {
                                        "pose": {
                                            "persons": persons,
                                        },
                                        "confidence": if sensing.classification.presence { sensing.classification.confidence } else { 0.0 },
                                        "activity": sensing.classification.motion_level,
                                        // pose_source tells the UI which estimation mode is active.
                                        "pose_source": pose_source,
                                        "metadata": {
                                            "frame_id": format!("rust_frame_{}", sensing.tick),
                                            "processing_time_ms": 1,
                                            "source": sensing.source,
                                            "tick": sensing.tick,
                                            "signal_strength": sensing.features.mean_rssi,
                                            "motion_band_power": sensing.features.motion_band_power,
                                            "breathing_band_power": sensing.features.breathing_band_power,
                                            "estimated_persons": persons.len(),
                                        }
                                    }
                                });
                                if socket.send(Message::Text(pose_msg.to_string().into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WS pose client lagged by {} messages, resuming from latest", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Handle ping/pong
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if v.get("type").and_then(|t| t.as_str()) == Some("ping") {
                                let pong = serde_json::json!({"type": "pong"});
                                let _ = socket.send(Message::Text(pong.to_string().into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket client disconnected (pose)");
}
