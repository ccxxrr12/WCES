//! Simulated data task: generates synthetic WiFi CSI frames at a fixed interval,
//! processes them through the same pipeline as real data, and broadcasts updates.

use std::time::Duration;

use tracing::info;

use crate::types::{
    NodeInfo, SensingUpdate,
    FRAME_HISTORY_CAPACITY,
};
use crate::SharedState;
use crate::signal_processing::*;
use crate::state_ops::{smooth_and_classify, adaptive_override, smooth_vitals};
use crate::vital_signs::VitalSigns;
use crate::mat_pipeline::VitalSignsInput;

pub(crate) async fn simulated_data_task(state: SharedState, tick_ms: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(tick_ms));
    info!("Simulated data source active (tick={}ms)", tick_ms);

    loop {
        interval.tick().await;

        // ═══ Phase 1a: Brief write lock — increment tick only ═══
        // The tick counter must be updated under the write lock to preserve
        // ordering with frame_history, but the heavy CPU work of synthesising
        // the CSI frame is deferred to the lock-free section below to
        // minimise contention with reader tasks (HTTP handlers, WS, etc.).
        let tick = {
            let mut s = state.write().await;
            s.tick += 1;
            s.tick
        };

        // ═══ Phase 1b: Lock-free pure computation ═══
        // generate_simulated_frame is pure CPU work (no shared state access)
        // and the f32 conversions are simple iterator passes — both run
        // outside the write lock to keep critical sections short.
        let frame = generate_simulated_frame(tick);
        let amps_f32: Vec<f32> = frame.amplitudes.iter().map(|a| *a as f32).collect();
        let phases_f32: Vec<f32> = frame.phases.iter().map(|p| *p as f32).collect();

        // ═══ Phase 1c: Write lock — stateful pipeline processing ═══
        // Operations below mutate shared sub-engines (VitalsBridge,
        // triage_engine, edge_engine) and history buffers, so they must run
        // under the write lock. A fuller refactor would also move
        // extract_features_from_frame outside by cloning frame_history, but
        // that is deferred because smooth_and_classify takes &mut s and
        // depends on the freshly-pushed frame in the same critical section.
        let (features, classification, breathing_rate_hz, sub_variances,
             _raw_motion, vitals, motion_score,
             triage_update, wasm_alerts, est_persons, frame_amplitudes,
             frame_n_sub, model_status, rssi_mean) =
        {
            let mut s = state.write().await;

            // Append current amplitudes to history before feature extraction.
            s.frame_history.push_back(frame.amplitudes.clone());
            if s.frame_history.len() > FRAME_HISTORY_CAPACITY {
                s.frame_history.pop_front();
            }

            let sample_rate_hz = 1000.0 / tick_ms as f64;
            let (features, mut classification, br_hz, variances, raw_motion) =
                extract_features_from_frame(&frame, &s.frame_history, sample_rate_hz);
            smooth_and_classify(&mut s, &mut classification, raw_motion);
            adaptive_override(&s, &features, &mut classification);

            s.rssi_history.push_back(features.mean_rssi);
            if s.rssi_history.len() > 60 {
                s.rssi_history.pop_front();
            }

            let motion_score = if classification.motion_level == "active" { 0.8 }
                else if classification.motion_level == "present_still" { 0.3 }
                else { 0.05 };

            // VitalsBridge: use the same IIR bandpass pipeline as the UDP path.
            // Replaces the old FFT+Goertzel VitalSignDetector (removed ADR-142).
            let sample_rate_hz = 1000.0 / tick_ms as f64;
            let mut raw_vitals = VitalSigns::default();
            {
                let vb = s.vitals_bridges.entry(1)
                    .or_insert_with(|| {
                        wifi_densepose_sensing_server::vitals_bridge::VitalsBridge::new(
                            frame.n_subcarriers as usize, sample_rate_hz)
                    });
                vb.set_sample_rate(sample_rate_hz);
                let (vb_br, vb_hr, vb_br_conf, vb_hr_conf) =
                    vb.extract(&frame.amplitudes, &frame.phases, tick);
                if vb_br.is_some() { raw_vitals.breathing_rate_bpm = vb_br; }
                if vb_hr.is_some() { raw_vitals.heart_rate_bpm = vb_hr; }
                raw_vitals.breathing_confidence = vb_br_conf.max(raw_vitals.breathing_confidence);
                raw_vitals.heartbeat_confidence = vb_hr_conf.max(raw_vitals.heartbeat_confidence);
                raw_vitals.signal_quality = (raw_vitals.breathing_confidence
                    .max(raw_vitals.heartbeat_confidence) * 0.8 + 0.2).clamp(0.0, 1.0);
            }
            let vitals = smooth_vitals(&mut s, &raw_vitals);
            s.latest_vitals = vitals.clone();

            // LLM analysis: push vitals into sliding windows for trend analysis
            if let Some(ref engine) = s.llm_engine {
                let eng = engine.clone();
                let br = vitals.breathing_rate_bpm.unwrap_or(0.0);
                let hr = vitals.heart_rate_bpm.unwrap_or(0.0);
                let sq = vitals.signal_quality;
                tokio::spawn(async move {
                    eng.push_vitals(1u8, br, hr, raw_motion as f64, sq).await;
                });
            }

            // MAT triage: compute START triage from simulated vital signs
            let triage_input = VitalSignsInput {
                breathing_rate_bpm: vitals.breathing_rate_bpm,
                breathing_confidence: vitals.breathing_confidence,
                heart_rate_bpm: vitals.heart_rate_bpm,
                heartbeat_confidence: vitals.heartbeat_confidence,
                signal_quality: vitals.signal_quality,
                motion_score,
                person_id: Some(tick as u32),
                node_id: 1,
                rssi: features.mean_rssi,
            };
            let triage_update = Some(s.triage_engine.process(&triage_input));

            // Edge module engine: run all 10 modules
            // (amps_f32/phases_f32 pre-computed in Phase 1b to avoid doing
            // the f32 conversion work while holding the write lock.)
            let wasm_alerts = Some(s.edge_engine.process_frame(
                &phases_f32, &amps_f32, raw_motion as f32,
                vitals.breathing_rate_bpm, vitals.heart_rate_bpm,
                classification.presence,
            ));

            let frame_amplitudes = frame.amplitudes.clone();
            let frame_n_sub = frame.n_subcarriers;

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
            let model_status = if s.model_loaded {
                Some(serde_json::json!({
                    "loaded": true,
                    "layers": s.progressive_loader.as_ref()
                        .map(|l| { let (a,b,c) = l.layer_status(); a as u8 + b as u8 + c as u8 })
                        .unwrap_or(0),
                    "sona_profile": s.active_sona_profile.as_deref().unwrap_or("default"),
                }))
            } else {
                None
            };
            let rssi_mean = features.mean_rssi;

            (features, classification, br_hz, variances,
             raw_motion, vitals, motion_score,
             triage_update, wasm_alerts, est_persons, frame_amplitudes,
             frame_n_sub, model_status, rssi_mean)
        }; // ── write lock released ──

        // ═══ Phase 2: Lock-free pure computation ═══

        // DensePose skeleton — always generated from CSI signal heuristics
        let densepose_keypoints = generate_synthetic_pose(tick, &frame_amplitudes, motion_score);

        let cls_confidence = classification.confidence;
        let mut update = SensingUpdate {
            msg_type: "sensing_update".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
            source: "simulated".to_string(),
            tick,
            nodes: vec![NodeInfo {
                node_id: 1,
                rssi_dbm: rssi_mean,
                position: [2.0, 0.0, 1.5],
                amplitude: frame_amplitudes,
                subcarrier_count: frame_n_sub as usize,
                // BUG 9 fix: use actual detected vitals instead of hardcoded constants
                breathing_rate_bpm: vitals.breathing_rate_bpm,
                heart_rate_bpm: vitals.heart_rate_bpm,
                motion_level: Some("present_still".into()),
                presence: true,
                active: true,
                channel: 6,
                band: "2.4GHz".into(),
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
            model_status,
            persons: None,
            estimated_persons: if est_persons > 0 { Some(est_persons) } else { None },
            tracked_survivors: None,
            alerts: None,
        };

        // Populate persons from the sensing update.
        let persons = derive_pose_from_sensing(&update);
        if !persons.is_empty() {
            update.persons = Some(persons);
        }

        let json = match serde_json::to_string(&update) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!("JSON serialize failed: {e}");
                continue;
            }
        };

        // ═══ Phase 3: Quick write lock — broadcast ═══
        {
            let mut s = state.write().await;
            if update.classification.presence {
                s.total_detections += 1;
            }
            let _ = s.tx.send(json);
            s.latest_update = Some(update);
        }
    }
}
