//! Model management route handlers (list, load, unload, delete models and LoRA profiles).

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::response::Json;
use tracing::{info, warn};

use crate::SharedState;
use super::path_util::sanitize_path_segment;

// ── Model Management Endpoints ──────────────────────────────────────────────

/// GET /api/v1/models —list discovered RVF model files.
pub(crate) async fn list_models(State(state): State<SharedState>) -> Json<serde_json::Value> {
    // Re-scan directory each call so newly-added files are visible.
    let models = scan_model_files();
    let total = models.len();
    {
        let mut s = state.write().await;
        s.discovered_models = models.clone();
    }
    Json(serde_json::json!({ "models": models, "total": total }))
}

/// GET /api/v1/models/active —return currently loaded model or null.
pub(crate) async fn get_active_model(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    match &s.active_model_id {
        Some(id) => {
            let model = s.discovered_models.iter().find(|m| {
                m.get("id").and_then(|v| v.as_str()) == Some(id.as_str())
            });
            Json(serde_json::json!({
                "active": model.cloned().unwrap_or_else(|| serde_json::json!({ "id": id })),
            }))
        }
        None => Json(serde_json::json!({ "active": serde_json::Value::Null })),
    }
}

/// POST /api/v1/models/load —load a model by ID.
pub(crate) async fn load_model(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let model_id = body.get("id")
        .or_else(|| body.get("model_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if model_id.is_empty() {
        return Json(serde_json::json!({ "error": "missing 'id' field", "success": false }));
    }
    let mut s = state.write().await;
    s.active_model_id = Some(model_id.clone());
    s.model_loaded = true;
    info!("Model loaded: {model_id}");
    Json(serde_json::json!({ "success": true, "model_id": model_id }))
}

/// POST /api/v1/models/unload —unload the current model.
pub(crate) async fn unload_model(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    let prev = s.active_model_id.take();
    s.model_loaded = false;
    info!("Model unloaded (was: {:?})", prev);
    Json(serde_json::json!({ "success": true, "previous": prev }))
}

/// DELETE /api/v1/models/:id —delete a model file.
pub(crate) async fn delete_model(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    // ADR-050: Sanitize path to prevent directory traversal
    let safe_id = match sanitize_path_segment(&id) {
        Ok(s) => s,
        Err(_) => return Json(serde_json::json!({ "error": "invalid model id", "success": false })),
    };
    let path = PathBuf::from("data/models").join(format!("{}.rvf", safe_id));
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("Failed to delete model file {:?}: {}", path, e);
            return Json(serde_json::json!({ "error": format!("delete failed: {e}"), "success": false }));
        }
        // If this was the active model, unload it
        let mut s = state.write().await;
        if s.active_model_id.as_deref() == Some(safe_id) {
            s.active_model_id = None;
            s.model_loaded = false;
        }
        s.discovered_models.retain(|m| {
            m.get("id").and_then(|v| v.as_str()) != Some(safe_id)
        });
        info!("Model deleted: {safe_id}");
        Json(serde_json::json!({ "success": true, "deleted": safe_id }))
    } else {
        Json(serde_json::json!({ "error": "model not found", "success": false }))
    }
}

/// GET /api/v1/models/lora/profiles —list LoRA adapter profiles.
pub(crate) async fn list_lora_profiles() -> Json<serde_json::Value> {
    // LoRA profiles are discovered from data/models/*.lora.json
    let profiles = scan_lora_profiles();
    Json(serde_json::json!({ "profiles": profiles }))
}

/// POST /api/v1/models/lora/activate —activate a LoRA adapter profile.
pub(crate) async fn activate_lora_profile(
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let profile = body.get("profile")
        .or_else(|| body.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if profile.is_empty() {
        return Json(serde_json::json!({ "error": "missing 'profile' field", "success": false }));
    }
    info!("LoRA profile activated: {profile}");
    Json(serde_json::json!({ "success": true, "profile": profile }))
}

// ── Scanner helpers ─────────────────────────────────────────────────────────

/// Scan `data/models/` for `.rvf` files and return metadata.
pub(crate) fn scan_model_files() -> Vec<serde_json::Value> {
    let dir = PathBuf::from("data/models");
    let mut models = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rvf") {
                let name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let modified = entry.metadata().ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                models.push(serde_json::json!({
                    "id": name,
                    "name": name,
                    "path": path.display().to_string(),
                    "size_bytes": size,
                    "format": "rvf",
                    "modified_epoch": modified,
                }));
            }
        }
    }
    models
}

/// Scan `data/models/` for `.lora.json` LoRA profile files.
pub(crate) fn scan_lora_profiles() -> Vec<serde_json::Value> {
    let dir = PathBuf::from("data/models");
    let mut profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".lora.json") {
                let profile_name = name.trim_end_matches(".lora.json").to_string();
                // Try to read the profile JSON
                let config = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                profiles.push(serde_json::json!({
                    "name": profile_name,
                    "path": path.display().to_string(),
                    "config": config,
                }));
            }
        }
    }
    profiles
}
