//! UDP receiver task: receives ESP32 CSI frames, parses vitals/WASM packets,
//! extracts features, processes vitals, and broadcasts sensing updates.

use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{info, warn, debug, error};

use crate::types::{
    NodeInfo, SensingUpdate,
    FRAME_HISTORY_CAPACITY,
};
use crate::SharedState;
use crate::signal_processing::*;
use crate::state_ops::{smooth_and_classify, adaptive_override, smooth_vitals};
use crate::parser::{parse_esp32_frame, parse_esp32_vitals, parse_wasm_output};
use crate::mat_pipeline::VitalSignsInput;

pub(crate) async fn udp_receiver_task(state: SharedState, udp_port: u16) {
    let addr = format!("0.0.0.0:{udp_port}");
    let socket = match UdpSocket::bind(&addr).await {
        Ok(s) => {
            info!("UDP listening on {addr} for ESP32 CSI frames");
            s
        }
        Err(e) => {
            error!("Failed to bind UDP {addr}: {e}");
            return;
        }
    };

    let mut buf = [0u8; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, src)) => {
                // ADR-039: Try edge vitals packet first (magic 0xC511_0002).
                if let Some(vitals) = parse_esp32_vitals(&buf[..len]) {
                    debug!("ESP32 vitals from {src}: node={} br={:.1} hr={:.1} pres={}",
                           vitals.node_id, vitals.breathing_rate_bpm,
                           vitals.heartrate_bpm, vitals.presence);
                    let mut s = state.write().await;
                    // Broadcast vitals via WebSocket.
                    if let Ok(json) = serde_json::to_string(&serde_json::json!({
                        "type": "edge_vitals",
                        "node_id": vitals.node_id,
                        "presence": vitals.presence,
                        "fall_detected": vitals.fall_detected,
                        "motion": vitals.motion,
                        "breathing_rate_bpm": vitals.breathing_rate_bpm,
                        "heartrate_bpm": vitals.heartrate_bpm,
                        "n_persons": vitals.n_persons,
                        "motion_energy": vitals.motion_energy,
                        "presence_score": vitals.presence_score,
                        "rssi": vitals.rssi,
                    })) {
                        let _ = s.tx.send(json);
                    }
                    s.edge_vitals = Some(vitals);
                    continue;
                }

                // ADR-040: Try WASM output packet (magic 0xC511_0004).
                if let Some(wasm_output) = parse_wasm_output(&buf[..len]) {
                    debug!("WASM output from {src}: node={} module={} events={}",
                           wasm_output.node_id, wasm_output.module_id,
                           wasm_output.events.len());
                    let mut s = state.write().await;
                    // Broadcast WASM events via WebSocket.
                    if let Ok(json) = serde_json::to_string(&serde_json::json!({
                        "type": "wasm_event",
                        "node_id": wasm_output.node_id,
                        "module_id": wasm_output.module_id,
                        "events": wasm_output.events,
                    })) {
                        let _ = s.tx.send(json);
                    }
                    s.latest_wasm_events = Some(wasm_output);
                    continue;
                }

                if let Some(frame) = parse_esp32_frame(&buf[..len]) {
                    debug!("ESP32 frame from {src}: node={}, subs={}, seq={}",
                           frame.node_id, frame.n_subcarriers, frame.sequence);

                    // ═══ Phase 1: Quick write lock — state mutations ═══
                    let (features, classification, breathing_rate_hz, sub_variances,
                         _raw_motion, vitals, tick, motion_score, model_loaded,
                         triage_update, wasm_alerts, est_persons, rssi_mean) =
                    {
                        let mut s = state.write().await;
                        s.source = "esp32".to_string();
                        s.last_esp32_frame = Some(std::time::Instant::now());

                        // Append current amplitudes to history before extracting features so
                        // that temporal analysis includes the most recent frame.
                        s.frame_history.push_back(frame.amplitudes.clone());
                        if s.frame_history.len() > FRAME_HISTORY_CAPACITY {
                            s.frame_history.pop_front();
                        }

                        let sample_rate_hz = 50.0; // ESP32 CSI frames arrive at ~20-100 Hz via lwIP
                        let (features, mut classification, br_hz, variances, raw_motion) =
                            extract_features_from_frame(&frame, &s.frame_history, sample_rate_hz);
                        smooth_and_classify(&mut s, &mut classification, raw_motion);
                        adaptive_override(&s, &features, &mut classification);

                        // Update RSSI history
                        s.rssi_history.push_back(features.mean_rssi);
                        if s.rssi_history.len() > 60 {
                            s.rssi_history.pop_front();
                        }

                        s.tick += 1;
                        let tick = s.tick;

                        let motion_score = if classification.motion_level == "active" { 0.8 }
                            else if classification.motion_level == "present_still" { 0.3 }
                            else { 0.05 };

                        // 子载波灵敏度选择: 取 top-30 高方差子载波提升生命体征SNR
                        let sensitive_sc = select_sensitive_subcarriers(
                            &s.frame_history, frame.n_subcarriers as usize, 30
                        );
                        let selected_amps = extract_selected_amplitudes(&frame.amplitudes, &sensitive_sc);
                        let selected_phases = extract_selected_amplitudes(&frame.phases, &sensitive_sc);

                        let raw_vitals = s.vital_detector.process_frame(
                            if selected_amps.len() >= 10 { &selected_amps } else { &frame.amplitudes },
                            if selected_phases.len() >= 10 { &selected_phases } else { &frame.phases },
                        );
                        let vitals = smooth_vitals(&mut s, &raw_vitals);
                        s.latest_vitals = vitals.clone();

                        // LLM analysis: push vitals into sliding windows for trend analysis
                        if let Some(ref engine) = s.llm_engine {
                            let eng = engine.clone();
                            let node_id = frame.node_id;
                            let br = vitals.breathing_rate_bpm.unwrap_or(0.0);
                            let hr = vitals.heart_rate_bpm.unwrap_or(0.0);
                            let sq = vitals.signal_quality;
                            tokio::spawn(async move {
                                eng.push_vitals(node_id, br, hr, raw_motion as f64, sq).await;
                            });
                        }

                        // MAT triage: compute START triage from vital signs
                        let triage_input = VitalSignsInput {
                            breathing_rate_bpm: vitals.breathing_rate_bpm,
                            breathing_confidence: vitals.breathing_confidence,
                            heart_rate_bpm: vitals.heart_rate_bpm,
                            heartbeat_confidence: vitals.heartbeat_confidence,
                            signal_quality: vitals.signal_quality,
                            motion_score,
                            person_id: Some(tick as u32),
                            node_id: frame.node_id,
                            rssi: features.mean_rssi,
                        };
                        let triage_update = Some(s.triage_engine.process(&triage_input));

                        // Edge module engine: run all 10 modules
                        let amps_f32: Vec<f32> = frame.amplitudes.iter().map(|a| *a as f32).collect();
                        let phases_f32: Vec<f32> = frame.phases.iter().map(|p| *p as f32).collect();
                        let wasm_alerts = Some(s.edge_engine.process_frame(
                            &phases_f32, &amps_f32, raw_motion as f32,
                            vitals.breathing_rate_bpm, vitals.heart_rate_bpm,
                            classification.presence,
                        ));

                        // Multi-person estimation with temporal smoothing (EMA α=0.10).
                        let raw_score = compute_person_score(&features);
                        s.smoothed_person_score = s.smoothed_person_score * 0.90 + raw_score * 0.10;
                        let est_persons = if classification.presence {
                            let count = score_to_person_count(s.smoothed_person_score, s.prev_person_count);
                            s.prev_person_count = count;
                            count
                        } else {
                            s.prev_person_count = 0;
                            0
                        };

                        let model_loaded = s.model_loaded;
                        let rssi_mean = features.mean_rssi;

                        (features, classification, br_hz, variances,
                         raw_motion, vitals, tick, motion_score, model_loaded,
                         triage_update, wasm_alerts, est_persons, rssi_mean)
                    }; // ── write lock released ──

                    // ═══ Phase 2: Lock-free pure computation ═══

                    // DensePose skeleton (always generated for simulated source)
                    let densepose_keypoints = if model_loaded {
                        generate_synthetic_pose(tick, &frame.amplitudes, motion_score)
                    } else {
                        None
                    };

                    let cls_confidence = classification.confidence;
                    let mut update = SensingUpdate {
                        msg_type: "sensing_update".to_string(),
                        timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
                        source: "esp32".to_string(),
                        tick,
                        nodes: vec![NodeInfo {
                            node_id: frame.node_id,
                            rssi_dbm: rssi_mean,
                            position: [2.0, 0.0, 1.5],
                            amplitude: frame.amplitudes.iter().take(56).cloned().collect(),
                            subcarrier_count: frame.n_subcarriers as usize,
                        }],
                        features: features.clone(),
                        classification,
                        signal_field: generate_signal_field(
                            rssi_mean, motion_score, breathing_rate_hz,
                            cls_confidence, &sub_variances,
                        ),
                        vital_signs: Some(vitals),
                        triage_update,
                        wasm_alerts,
                        pose_keypoints: densepose_keypoints,
                        model_status: None,
                        persons: None,
                        estimated_persons: if est_persons > 0 { Some(est_persons) } else { None },
                    };

                    let persons = derive_pose_from_sensing(&update);
                    if !persons.is_empty() {
                        update.persons = Some(persons);
                    }

                    let json = match serde_json::to_string(&update) {
                        Ok(json) => json,
                        Err(e) => {
                            warn!("JSON serialize failed: {e}");
                            continue;
                        }
                    };

                    // ═══ Phase 3: Quick write lock — broadcast ═══
                    {
                        let mut s = state.write().await;
                        let _ = s.tx.send(json);
                        s.latest_update = Some(update);
                    }
                }
            }
            Err(e) => {
                warn!("UDP recv error: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
