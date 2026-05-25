//! LLM Analysis endpoint handlers (patients, analyze, status).

use axum::extract::State;
use axum::response::Json;

use crate::SharedState;

use std::sync::Arc;
use wifi_densepose_llm::{
    LlmAnalysisEngine, PatientRecord,
    AgentVitalSnapshot, StructuredContext, TriggerSource, TrendSummary,
};
use crate::edge_module_engine::EdgeAlert;

// ── LLM Analysis endpoints ──────────────────────────────────────────────────

/// GET /api/v1/patients — list all registered patients
pub(crate) async fn llm_patients_list(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.llm_engine {
        Some(engine) => {
            match engine.list_patients().await {
                Ok(patients) => Json(serde_json::json!({
                    "status": "ok",
                    "patients": patients,
                    "count": patients.len(),
                })),
                Err(e) => Json(serde_json::json!({ "status": "error", "message": e.to_string() })),
            }
        }
        None => Json(serde_json::json!({ "status": "error", "message": "LLM engine not available" })),
    }
}

/// POST /api/v1/patients — register a patient
pub(crate) async fn llm_patient_register(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let engine = {
        let s = state.read().await;
        s.llm_engine.clone()
    };
    match engine {
        Some(engine) => {
            let pid = body["patient_id"].as_str().unwrap_or("UNKNOWN");
            let mut record = PatientRecord::new(pid);
            if let Some(age) = body["age"].as_u64() { record.age = Some(age as u8); }
            if let Some(name) = body["name"].as_str() { record.name = Some(name.to_string()); }
            if let Some(node_id) = body["node_id"].as_u64() { record.node_id = Some(node_id as u8); }
            if let Some(gender_str) = body["gender"].as_str() {
                record.gender = Some(match gender_str {
                    "male" => wifi_densepose_llm::Gender::Male,
                    "female" => wifi_densepose_llm::Gender::Female,
                    _ => wifi_densepose_llm::Gender::Other,
                });
            }
            if let Some(arr) = body["pre_existing"].as_array() {
                record.pre_existing = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            }
            match engine.register_patient(record).await {
                Ok(()) => Json(serde_json::json!({ "status": "ok", "patient_id": pid })),
                Err(e) => Json(serde_json::json!({ "status": "error", "message": e.to_string() })),
            }
        }
        None => Json(serde_json::json!({ "status": "error", "message": "LLM engine not available" })),
    }
}

/// POST /api/v1/llm/analyze — trigger LLM analysis for a patient
pub(crate) async fn llm_analyze(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let patient_id = body["patient_id"].as_str().unwrap_or("UNKNOWN").to_string();
    let (engine, br, hr, motion, quality, triage_label, alerts): (Option<Arc<LlmAnalysisEngine>>, Option<f64>, Option<f64>, f64, f64, String, Vec<String>) = {
        let s = state.read().await;
        let triage = s.latest_update.as_ref()
            .and_then(|u| u.triage_update.as_ref())
            .and_then(|t| t.survivors.iter()
                .find(|surv| surv.id == patient_id)
                .map(|surv| surv.triage.clone()))
            .unwrap_or_else(|| "Unknown".to_string());
        let a: Vec<String> = s.latest_update.as_ref()
            .and_then(|u| u.wasm_alerts.as_ref())
            .map(|alerts| alerts.iter().map(|al: &EdgeAlert| al.event_name.clone()).collect())
            .unwrap_or_default();
        (s.llm_engine.clone(),
         s.latest_vitals.breathing_rate_bpm,
         s.latest_vitals.heart_rate_bpm,
         s.smoothed_motion,
         s.latest_vitals.signal_quality,
         triage,
         a)
    };
    match engine {
        Some(engine) => {
            // Trigger sync analysis (non-streaming REST endpoint)
            match engine.trigger_analysis(
                &patient_id, br, hr, motion, quality,
                &triage_label, &alerts,
            ).await {
                Some(result) => Json(serde_json::json!({
                    "status": "ok",
                    "analysis": result,
                })),
                None => Json(serde_json::json!({
                    "status": "error",
                    "message": "Analysis could not be generated (insufficient data or cooldown active)",
                })),
            }
        }
        None => Json(serde_json::json!({ "status": "error", "message": "LLM engine not available" })),
    }
}

/// GET /api/v1/llm/status — LLM engine status
pub(crate) async fn llm_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.llm_engine {
        Some(engine) => {
            let status = engine.status().await;
            Json(serde_json::json!({
                "status": "ok",
                "llm": status,
            }))
        }
        None => Json(serde_json::json!({ "status": "ok", "llm": "disabled" })),
    }
}

// ── Agent endpoints (Phase 4) ──────────────────────────────────────────────

/// POST /api/v1/agent/analyze — trigger MedicalAgent analysis
pub(crate) async fn agent_analyze(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let patient_id_str = body["patient_id"].as_str().unwrap_or("1");
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
            .map(|alerts| alerts.iter().map(|al: &EdgeAlert| al.event_name.clone()).collect())
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

    let mut agent_guard = agent.lock().await;
    let result = agent_guard.analyze(ctx).await;
    drop(agent_guard);

    Json(serde_json::json!({
        "status": "ok",
        "analysis": {
            "patient_id": result.patient_id,
            "text": result.text,
            "source": result.source,
            "degrade_level": result.degrade_level,
            "risk_adjustment": result.risk_adjustment,
            "generated_at_ms": result.generated_at_ms,
        },
    }))
}

/// GET /api/v1/agent/status — MedicalAgent status
pub(crate) async fn agent_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    let is_open = s.medical_agent.lock().await.is_breaker_open().await;
    Json(serde_json::json!({
        "status": "ok",
        "agent": {
            "circuit_breaker_open": is_open,
        },
    }))
}
