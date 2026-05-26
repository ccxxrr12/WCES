//! WebSocket message handlers for sensing and pose streams.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use tracing::{info, warn};

use crate::SharedState;
use crate::types::{BoundingBox, PersonDetection, PoseKeypoint, SensingUpdate};
use crate::signal_processing::derive_pose_from_sensing;

use wifi_densepose_llm::{
    PatientRecord, AgentVitalSnapshot, StructuredContext, TriggerSource, TrendSummary,
};
use crate::edge_module_engine::EdgeAlert;

// ── Sensing WebSocket handler ──────────────────────────────────────────────────

pub(crate) async fn ws_sensing_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_client(socket, state))
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
                                    let engine = {
                                        let s = state.read().await;
                                        s.llm_engine.clone()
                                    };
                                    if let Some(ref engine) = engine {
                                        let (br, hr, motion, quality, triage_label, alerts) = {
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
                                            (s.latest_vitals.breathing_rate_bpm,
                                             s.latest_vitals.heart_rate_bpm,
                                             s.smoothed_motion,
                                             s.latest_vitals.signal_quality,
                                             triage,
                                             a)
                                        };

                                        let eng = engine.clone();
                                        let pid = patient_id.to_string();
                                        let tx = {
                                            let s = state.read().await;
                                            s.tx.clone()
                                        };
                                        tokio::spawn(async move {
                                            if let Some(mut rx) = eng.trigger_analysis_streaming(
                                                &pid, br, hr, motion, quality,
                                                &triage_label, &alerts,
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
                                        });
                                    }
                                }
                                Some("agent_analyze_request") => {
                                    let patient_id_str = msg["patient_id"].as_str().unwrap_or("1");
                                    let patient_id: u32 = patient_id_str.parse().unwrap_or(1);
                                    let (agent, vitals, triage_label, alerts, smoothed_motion) = {
                                        let s = state.read().await;
                                        let triage = s.latest_update.as_ref()
                                            .and_then(|u| u.triage_update.as_ref())
                                            .and_then(|t| t.survivors.iter()
                                                .find(|surv| surv.id == patient_id_str)
                                                .map(|surv| surv.triage.clone()))
                                            .unwrap_or_else(|| "Unknown".to_string());
                                        let a: Vec<String> = s.latest_update.as_ref()
                                            .and_then(|u| u.wasm_alerts.as_ref())
                                            .map(|alerts: &Vec<EdgeAlert>| alerts.iter().map(|al| al.event_name.clone()).collect())
                                            .unwrap_or_default();
                                        (s.medical_agent.clone(),
                                         s.latest_vitals.clone(),
                                         triage,
                                         a,
                                         s.smoothed_motion)
                                    };

                                    let vitals_snapshot = AgentVitalSnapshot {
                                        breathing_rate_bpm: vitals.breathing_rate_bpm.map(|v| v as f32),
                                        heart_rate_bpm: vitals.heart_rate_bpm.map(|v| v as f32),
                                        breathing_confidence: vitals.breathing_confidence as f32,
                                        heartbeat_confidence: vitals.heartbeat_confidence as f32,
                                        signal_quality: vitals.signal_quality as f32,
                                        motion_class: Some(if smoothed_motion > 0.6 { "active" } else if smoothed_motion > 0.2 { "present_still" } else { "still" }.into()),
                                        person_count_estimate: Some(1),
                                        rssi: Some(-45),
                                    };

                                    let ctx = StructuredContext {
                                        patient_id,
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
                                        kb_matches: vec![],
                                        triggered_by: TriggerSource::ManualRequest { patient_id },
                                        built_at_ms: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64,
                                    };

                                    let tx = {
                                        let s = state.read().await;
                                        s.tx.clone()
                                    };
                                    tokio::spawn(async move {
                                        let mut agent_guard = agent.lock().await;
                                        let result = agent_guard.analyze(ctx).await;
                                        drop(agent_guard);

                                        let json = serde_json::json!({
                                            "type": "agent_analysis",
                                            "patient_id": result.patient_id,
                                            "text": result.text,
                                            "source": result.source,
                                            "degrade_level": result.degrade_level,
                                            "risk_adjustment": result.risk_adjustment,
                                            "generated_at_ms": result.generated_at_ms,
                                        });
                                        if let Ok(json_str) = serde_json::to_string(&json) {
                                            let _ = tx.send(json_str);
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
