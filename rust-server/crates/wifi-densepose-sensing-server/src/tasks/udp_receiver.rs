//! UDP receiver task: receives ESP32 CSI frames, parses vitals/WASM packets,
//! extracts features, processes vitals, and broadcasts sensing updates.

use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use std::sync::Arc;
use tracing::{info, warn, debug, error};

use crate::types::{
    NodeInfo, SensingUpdate, FeatureInfo, ClassificationInfo,
    TrackedSurvivor, FRAME_HISTORY_CAPACITY, MOTION_EMA_ALPHA,
};
use crate::vital_signs::VitalSigns;
use wifi_densepose_sensing_server::signal_pipeline::SignalPipelineOutput;
use crate::edge_module_engine::EdgeAlert;
use crate::SharedState;
use crate::signal_processing::*;
use crate::state_ops::adaptive_override;
use crate::parser::{parse_esp32_frame, parse_esp32_vitals, parse_wasm_output};
use crate::mat_pipeline::VitalSignsInput;

use wifi_densepose_llm::{
    AgentVitalSnapshot, StructuredContext, TriggerSource, TrendSummary,
};

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

    // Limit concurrent agent analyses to prevent unbounded task growth
    let agent_sem = Arc::new(Semaphore::new(4));

    // Broadcast throttle: max ~10 Hz to prevent WebSocket channel overflow
    let mut last_broadcast = Instant::now();
    const BROADCAST_INTERVAL_MS: u64 = 100; // 10 Hz max

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
                    let now = Instant::now();
                    if now.duration_since(last_broadcast) >= Duration::from_millis(BROADCAST_INTERVAL_MS) {
                        last_broadcast = now;
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
                    }
                    s.edge_vitals = Some(vitals);
                    continue;
                }

                // ADR-040: Try WASM output packet (magic 0xC511_0005).
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
                         _raw_motion, vitals, tick, motion_score,
                         triage_update, wasm_alerts, est_persons, rssi_mean,
                         prev_triage, agent_handle, node_snapshot,
                         field_perturbation, tracked_survivors) =
                    {
                        let mut s = state.write().await;
                        s.source = "esp32".to_string();
                        s.last_esp32_frame = Some(std::time::Instant::now());

                        let mut vitals: VitalSigns;
                        let features: FeatureInfo;
                        let mut classification: ClassificationInfo;
                        let br_hz: f64;
                        let variances: Vec<f64>;
                        let raw_motion: f64;
                        let tick: u64;
                        let motion_score: f64;
                        let smoothed_motion: f64;
                        let cir_distance_m: Option<f64>;
                        let signal_out: Option<SignalPipelineOutput>;
                        let sample_rate_hz: f64;
                        {
                            // ── Per-node independent pipeline ──
                            let ns = s.node_states.entry(frame.node_id)
                                .or_insert_with(|| crate::types::PerNodeState::new(20.0));

                            // Dynamic sample rate: measure actual frame arrival interval
                            // and smooth with EMA to adapt to real ESP32-C5 transmission rate.
                            let now = std::time::Instant::now();
                            if let Some(prev) = ns.last_frame_time {
                                let dt = now.duration_since(prev).as_secs_f64();
                                if dt > 0.001 && dt < 1.0 {
                                    // valid interval: 1ms–1s → 1–1000 Hz
                                    let instantaneous = 1.0 / dt;
                                    // EMA α=0.15 — adapts within ~1 second at 20 Hz
                                    ns.measured_sample_rate = ns.measured_sample_rate * 0.85
                                                           + instantaneous * 0.15;
                                }
                            }
                            ns.last_frame_time = Some(now);
                            ns.tick += 1;
                            ns.frame_history.push_back(frame.amplitudes.clone());
                            if ns.frame_history.len() > FRAME_HISTORY_CAPACITY { ns.frame_history.pop_front(); }

                            // ── Signal pipeline: phase sanitize → normalize → hampel → motion → coherence gate ──
                            signal_out = ns.signal_pipeline.process(&frame.amplitudes, &frame.phases);

                            // Use dynamically-measured sample rate instead of hardcoded 20 Hz
                            sample_rate_hz = ns.measured_sample_rate;
                            let (f, _c, b, v, rm) = extract_features_from_frame(&frame, &ns.frame_history, sample_rate_hz);
                            features = f; br_hz = b; variances = v; raw_motion = rm;

                            ns.rssi_history.push_back(features.mean_rssi);
                            if ns.rssi_history.len() > 60 { ns.rssi_history.pop_front(); }

                            tick = ns.tick;
                            // ── Motion detection: fully from signal_pipeline ──
                            // PhaseSanitizer→Hampel→MotionDetector pipeline (ADR-142).
                            // More accurate than the old hand-written 4-factor blend thanks to
                            // phase unwrapping, Hampel outlier filtering, and adaptive thresholding.
                            motion_score = signal_out.as_ref().map(|so| so.motion_score).unwrap_or(0.05);
                            classification = ClassificationInfo {
                                motion_level: if motion_score > 0.15 { "active".into() }
                                    else if motion_score > 0.08 { "present_moving".into() }
                                    else if motion_score > 0.03 { "present_still".into() }
                                    else { "absent".into() },
                                presence: motion_score > 0.03,
                                confidence: (0.4 + motion_score * 0.6).clamp(0.0, 1.0),
                            };
                            // EMA-smoothed motion for global backward-compat (used by periodic agent)
                            ns.smoothed_motion = ns.smoothed_motion * (1.0 - MOTION_EMA_ALPHA)
                                               + motion_score * MOTION_EMA_ALPHA;

                            // ── Vital signs: upstream-standard IIR bandpass path (ADR-142) ──
                            // VitalsBridge uses the same algorithm as wifi_densepose_wifiscan's
                            // CoarseBreathingExtractor: IIR bandpass + zero-crossing (breathing)
                            // and IIR bandpass + autocorrelation (heart rate).
                            // The old FFT+Goertzel VitalSignDetector and MAT DetectionBridge paths
                            // are removed — VitalsBridge is the sole vital sign source.
                            vitals = VitalSigns::default();

                            ns.latest_vitals = vitals.clone();
                            // Capture per-node smoothed_motion for global backward-compat
                            smoothed_motion = ns.smoothed_motion;
                            // ns dropped here — releases borrow on s.node_states
                        }

                        // Parallel: run vitals crate pipeline (Butterworth filtering).
                        // Use signal_pipeline cleaned data when available (dead data flow fix #1).
                        {
                            let vb = s.vitals_bridges.entry(frame.node_id)
                                .or_insert_with(|| wifi_densepose_sensing_server::vitals_bridge::VitalsBridge::new(
                                    frame.n_subcarriers as usize, sample_rate_hz));
                            vb.set_sample_rate(sample_rate_hz);
                            let (use_amps, use_phases): (&[f64], &[f64]) = if let Some(ref so) = signal_out {
                                (&so.cleaned_amplitudes, &so.cleaned_phases)
                            } else {
                                (&frame.amplitudes, &frame.phases)
                            };
                            let (vb_br, vb_hr, vb_br_conf, vb_hr_conf) = vb.extract(
                                use_amps, use_phases, tick,
                            );
                            if vb_br.is_some() { vitals.breathing_rate_bpm = vb_br; }
                            if vb_hr.is_some() { vitals.heart_rate_bpm = vb_hr; }
                            vitals.breathing_confidence = vb_br_conf.max(vitals.breathing_confidence);
                            vitals.heartbeat_confidence = vb_hr_conf.max(vitals.heartbeat_confidence);
                        }

                        // CIR bridge: ISTA sparse CIR estimation → ToF ranging
                        {
                            let cb = s.cir_bridges.entry(frame.node_id)
                                .or_insert_with(|| wifi_densepose_sensing_server::cir_bridge::CirBridge::new());
                            cb.process(&frame.amplitudes, &frame.phases);
                            cir_distance_m = cb.ranging_distance_m()
                                .or_else(|| cb.dominant_distance_m());
                        }

                        // Update global fields for backward compatibility
                        s.smoothed_motion = smoothed_motion;

                        // Field model calibration: learn empty-room electromagnetic baseline.
                        // After calibration (~30s), perturbation energy improves signal field.
                        let field_perturbation = s.field_bridge.as_mut()
                            .and_then(|fb| fb.feed(&frame.amplitudes));

                        // Apply adaptive model override if a trained classifier is loaded
                        adaptive_override(&s, &features, &mut classification);

                        // Update global fields for backward compatibility (latest frame wins)
                        s.latest_vitals = vitals.clone();
                        s.tick = tick;

                        // LLM analysis: push vitals into sliding windows for trend analysis
                        // Inline — push_vitals is fast (lock+window push), no need to spawn
                        if let Some(ref engine) = s.llm_engine {
                            engine.push_vitals(
                                frame.node_id,
                                vitals.breathing_rate_bpm.unwrap_or(0.0),
                                vitals.heart_rate_bpm.unwrap_or(0.0),
                                raw_motion as f64,
                                vitals.signal_quality,
                            ).await;
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
                        // Shared triage engine: all nodes feed the same survivors.
                        let mut triage_update = Some(s.triage_engine.process(&triage_input));

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
                        let est_persons = s.node_states.get_mut(&frame.node_id).map(|ns| {
                            ns.smoothed_person_score = ns.smoothed_person_score * 0.90 + raw_score * 0.10;
                            if classification.presence {
                                let count = score_to_person_count(ns.smoothed_person_score, ns.prev_person_count);
                                ns.prev_person_count = count;
                                count
                            } else { ns.prev_person_count = 0; 0 }
                        }).unwrap_or(0);

                        let model_loaded = s.model_loaded;
                        let rssi_mean = features.mean_rssi;

                        // ── Localization bridge: feed multi-node RSSI + CIR → triangulate ──
                        s.localization_bridge.feed_observation(
                            frame.node_id, features.mean_rssi, cir_distance_m,
                        );
                        let triangulated_pos: Option<[f64; 3]> = s.localization_bridge.estimate_position();

                        // ── Tracking bridge: Kalman + fingerprint re-ID ──
                        let track_obs = wifi_densepose_sensing_server::tracking_bridge::TrackObservation {
                            position: triangulated_pos,
                            breathing_rate_bpm: vitals.breathing_rate_bpm,
                            heart_rate_bpm: vitals.heart_rate_bpm,
                            signal_quality: vitals.signal_quality,
                            motion_score,
                            confidence: vitals.breathing_confidence.max(vitals.heartbeat_confidence),
                            node_id: frame.node_id,
                            person_id: Some(tick as u32),
                        };
                        let _track_result = s.tracking_bridge.update(&[track_obs]);

                        // Apply triangulated position from LocalizationBridge (cross-node)
                        // and Kalman-smoothed position from TrackingBridge.
                        if let Some(ref mut tu) = triage_update {
                            for survivor in &mut tu.survivors {
                                // Priority 1: Kalman-smoothed position from tracking bridge.
                                if let Some(smoothed) = s.tracking_bridge.smoothed_position(&survivor.id) {
                                    survivor.position = Some(smoothed);
                                    survivor.position_confidence =
                                        (survivor.position_confidence + 0.15).min(1.0);
                                }
                                // Priority 2: Raw triangulated position from localization bridge
                                // (applies when tracking bridge hasn't converged yet).
                                else if let Some(tri_pos) = triangulated_pos {
                                    survivor.position = Some(tri_pos);
                                    survivor.position_confidence =
                                        (survivor.position_confidence + 0.1).min(0.9);
                                }
                                survivor.reidentified = s.tracking_bridge.was_reidentified(&survivor.id);
                            }
                        }

                        // ── Collect tracked survivor snapshots from Kalman filter
                        // (dead data flow fix #3: wire tracking_bridge → SensingUpdate).
                        let tracked_survivors: Option<Vec<TrackedSurvivor>> = {
                            let snapshots = s.tracking_bridge.active_track_snapshots();
                            if snapshots.is_empty() {
                                None
                            } else {
                                Some(snapshots.iter().map(|ts| {
                                    TrackedSurvivor {
                                        survivor_id: ts.display_id.clone(),
                                        position: Some(ts.position),
                                        velocity: Some(ts.velocity),
                                        reidentified: s.tracking_bridge.was_reidentified(&ts.display_id),
                                        tracking_confidence: 0.5,
                                    }
                                }).collect())
                            }
                        };

                        // ── Alerting bridge: generate structured alert if triage warrants ──
                        if let Some(ref tu) = triage_update {
                            for survivor in &tu.survivors {
                                if survivor.triage != "Minor" && survivor.triage != "Green"
                                    && survivor.triage != "Unknown"
                                {
                                    let _alert = s.alerting_bridge.generate_alert(
                                        &survivor.id,
                                        &survivor.triage,
                                        survivor.breathing_rate,
                                        survivor.heart_rate,
                                        survivor.position,
                                    );
                                }
                            }
                        }

                        // Capture previous triage for agent deterioration trigger
                        let prev_triage = s.latest_update.as_ref()
                            .and_then(|u| u.triage_update.as_ref())
                            .and_then(|t| t.survivors.first().map(|s| s.triage.clone()));
                        let agent_handle = s.medical_agent.clone();

                        // Clone node data for building NodeInfo outside the lock
                        let node_snapshot: Vec<_> = s.node_states.iter().map(|(&id, ns)| {
                            (id, ns.last_frame_time, ns.rssi_history.back().copied().unwrap_or(0.),
                             ns.latest_vitals.breathing_rate_bpm, ns.latest_vitals.heart_rate_bpm,
                             ns.current_motion_level.clone(), ns.latest_vitals.breathing_rate_bpm.is_some())
                        }).collect();

                        (features, classification, br_hz, variances,
                         raw_motion, vitals, tick, motion_score,
                         triage_update, wasm_alerts, est_persons, rssi_mean,
                         prev_triage, agent_handle, node_snapshot,
                         field_perturbation, tracked_survivors)
                    }; // ── write lock released ──

                    // Build multi-node info from snapshot (no lock held)
                    let now = Instant::now();
                    let timeout = Duration::from_secs(5);
                    let all_nodes: Vec<NodeInfo> = node_snapshot.iter().map(|&(nid, last_t, rssi, br, hr, ref ml, pres): &(u8, Option<Instant>, f64, Option<f64>, Option<f64>, String, bool)| {
                        let active = last_t.map(|t| now.duration_since(t) < timeout).unwrap_or(false);
                        // Node positions from competition config (node_id → (x,y,z))
                        let pos = crate::mat_pipeline::node_positions_arr()
                            .get(&nid).copied().unwrap_or([2.,0.,1.5]);
                        if nid == frame.node_id {
                            NodeInfo { node_id: nid, rssi_dbm: rssi, position: pos, amplitude: frame.amplitudes.iter().take(56).cloned().collect(), subcarrier_count: frame.n_subcarriers as usize, breathing_rate_bpm: vitals.breathing_rate_bpm, heart_rate_bpm: vitals.heart_rate_bpm, motion_level: Some(classification.motion_level.clone()), presence: classification.presence, active: true, channel: frame.freq_mhz as u8, band: "5GHz".into() }
                        } else if active {
                            NodeInfo { node_id: nid, rssi_dbm: rssi, position: pos, amplitude: vec![], subcarrier_count: 0, breathing_rate_bpm: br, heart_rate_bpm: hr, motion_level: Some(ml.clone()), presence: pres, active: true, channel: 0, band: "5GHz".into() }
                        } else { NodeInfo { node_id: nid, rssi_dbm: 0., position: pos, amplitude: vec![], subcarrier_count: 0, breathing_rate_bpm: None, heart_rate_bpm: None, motion_level: None, presence: false, active: false, channel: 0, band: "".into() } }
                    }).collect();

                    // ── Agent trigger: spawn analysis on triage escalation only ──
                    let curr_triage: Option<String> = triage_update.as_ref()
                        .and_then(|t| t.survivors.first().map(|s| s.triage.clone()));
                    if let (Some(ref prev), Some(ref curr)) = (&prev_triage, &curr_triage) {
                        if is_triage_escalation(&prev, &curr) {
                            let trigger = TriggerSource::Deterioration {
                                patient_id: frame.node_id as u32,
                                from: prev.clone(),
                                to: curr.clone(),
                            };

                            let vitals_snapshot = AgentVitalSnapshot {
                                breathing_rate_bpm: vitals.breathing_rate_bpm.map(|v| v as f32),
                                heart_rate_bpm: vitals.heart_rate_bpm.map(|v| v as f32),
                                breathing_confidence: vitals.breathing_confidence as f32,
                                heartbeat_confidence: vitals.heartbeat_confidence as f32,
                                signal_quality: vitals.signal_quality as f32,
                                motion_class: Some(if motion_score > 0.6 { "active" } else if motion_score > 0.2 { "present_still" } else { "still" }.into()),
                                person_count_estimate: Some(1),
                                rssi: Some(rssi_mean as i16),
                            };
                            let alerts: Vec<String> = wasm_alerts.as_ref()
                                .map(|a: &Vec<EdgeAlert>| a.iter().map(|al| al.event_name.clone()).collect())
                                .unwrap_or_default();

                            let ctx = StructuredContext {
                                patient_id: frame.node_id as u32,
                                node_id: frame.node_id,
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
                                triage_current: curr.clone(),
                                triage_trajectory: vec![],
                                patient_history: None,
                                recent_alerts: alerts,
                                kb_matches: vec![],
                                triggered_by: trigger,
                                built_at_ms: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64,
                            };

                            let agent = agent_handle.clone();
                            let state_for_agent = state.clone();
                            let sem = agent_sem.clone();
                            tokio::spawn(async move {
                                // Load-shed if already overloaded with analyses
                                let Ok(permit) = sem.try_acquire_owned() else {
                                    warn!("Agent overload, dropping analysis for patient {}", ctx.patient_id);
                                    return;
                                };
                                let _permit = permit;

                                // DESIGN NOTE: The Mutex lock is held across the `.await` call
                                // in `agent_guard.analyze(ctx).await`. This is a known trade-off:
                                // MedicalAgent::analyze requires &mut self, so the lock must be
                                // held for the entire analysis duration. The Semaphore above
                                // (max 4 concurrent analyses) bounds the contention.
                                // A tokio::time::timeout wraps the analyze call so a hung LLM
                                // request cannot hold the lock indefinitely.
                                let patient_id = ctx.patient_id;
                                let mut agent_guard = agent.lock().await;
                                let result = match tokio::time::timeout(
                                    Duration::from_secs(30),
                                    agent_guard.analyze(ctx),
                                )
                                .await
                                {
                                    Ok(r) => r,
                                    Err(_elapsed) => {
                                        warn!(
                                            "Agent analysis timed out for patient {}",
                                            patient_id
                                        );
                                        return;
                                    }
                                };
                                drop(agent_guard);

                                if !result.text.is_empty() {
                                    let tx = {
                                        let s = state_for_agent.read().await;
                                        s.tx.clone()
                                    };
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
                                }
                            });
                        }
                    }

                    // ═══ Phase 2: Lock-free pure computation ═══

                    // DensePose skeleton — always generated from CSI signal heuristics.
                    // Uses amplitude mean → scale, motion_score → animation, no ML model needed.
                    let densepose_keypoints = generate_synthetic_pose(tick, &frame.amplitudes, motion_score);

                    let cls_confidence = classification.confidence;
                    // Build signal field grid, applying field-model perturbation when available
                    // (dead data flow fix #2: wire field_bridge perturbation into signal_field).
                    let mut signal_field_data = generate_signal_field(
                        rssi_mean, motion_score, breathing_rate_hz,
                        cls_confidence, &sub_variances,
                    );
                    if let Some(perturbation) = field_perturbation {
                        let scale = (perturbation * 0.3).clamp(-0.5, 0.5);
                        for v in &mut signal_field_data.values {
                            *v = (*v + scale).clamp(0.0, 1.0);
                        }
                    }
                    let mut update = SensingUpdate {
                        msg_type: "sensing_update".to_string(),
                        timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
                        source: "esp32".to_string(),
                        tick,
                        nodes: all_nodes,
                        features: features.clone(),
                        classification,
                        signal_field: signal_field_data,
                        vital_signs: Some(vitals),
                        triage_update,
                        wasm_alerts,
                        pose_keypoints: densepose_keypoints,
                        model_status: None,
                        persons: None,
                        estimated_persons: if est_persons > 0 { Some(est_persons) } else { None },
                        tracked_survivors,
                        alerts: None,  // populated by broadcast_tick via AlertingBridge drain
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

                    // ═══ Phase 3: Quick write lock — broadcast (throttled) ═══
                    let now = Instant::now();
                    if now.duration_since(last_broadcast) >= Duration::from_millis(BROADCAST_INTERVAL_MS) {
                        last_broadcast = now;
                        {
                            let mut s = state.write().await;
                            // Use try_send to avoid blocking if channel is full
                            let _ = s.tx.send(json);
                            s.latest_update = Some(update);
                        }
                    } else {
                        // Still update latest_update so broadcast_tick can pick it up
                        let mut s = state.write().await;
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

/// Returns true if the triage change is an escalation (worsening).
fn is_triage_escalation(from: &str, to: &str) -> bool {
    fn severity(t: &str) -> u8 {
        match t {
            // BUG 7 fix: "Deceased" is the most severe outcome, now correctly
            // triggers escalation when transitioning from any lower tier.
            "Deceased" | "Black" => 4,
            "Immediate" | "Red" => 3,
            "Delayed" | "Yellow" => 2,
            "Minor" | "Green" => 1,
            _ => 0,
        }
    }
    severity(to) > severity(from)
}

