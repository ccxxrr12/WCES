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
    let data_dir = state.read().await.data_dir.clone();
    let models = scan_model_files(&data_dir);
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

    // Resolve the model file path: look up in discovered_models first (gives
    // the absolute path captured during scan), then fall back to the
    // conventional {data_dir}/data/models/{id}.rvf location.
    let model_path: PathBuf = {
        let s = state.read().await;
        let from_discovery = s.discovered_models.iter()
            .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(&model_id))
            .and_then(|m| m.get("path").and_then(|v| v.as_str()).map(String::from));
        match from_discovery {
            Some(p) => PathBuf::from(p),
            None => s.data_dir.join("data/models").join(format!("{model_id}.rvf")),
        }
    };

    // Actually load the model weights via ProgressiveLoader. Previously this
    // handler was a stub that only set active_model_id/model_loaded flags
    // without loading any weights, so the UI reported "loaded" while no
    // inference data was present.
    let data = match std::fs::read(&model_path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read model file {}: {e}", model_path.display());
            return Json(serde_json::json!({
                "error": format!("cannot read model file: {e}"),
                "success": false,
                "model_id": model_id,
            }));
        }
    };
    let mut loader = match crate::rvf_pipeline::ProgressiveLoader::new(&data) {
        Ok(l) => l,
        Err(e) => {
            warn!("ProgressiveLoader init failed for {model_id}: {e}");
            return Json(serde_json::json!({
                "error": format!("model load failed: {e}"),
                "success": false,
                "model_id": model_id,
            }));
        }
    };
    // Load Layer A (manifest + index). Non-fatal if it fails — ProgressiveLoader
    // itself is valid; matches main.rs load_layer_a behaviour.
    let layer_a = match loader.load_layer_a() {
        Ok(la) => {
            info!("  Layer A ready: model={} v{} ({} segments)", la.model_name, la.version, la.n_segments);
            serde_json::json!({
                "model_name": la.model_name,
                "version": la.version,
                "n_segments": la.n_segments,
            })
        }
        Err(e) => {
            warn!("Layer A load failed for {model_id}: {e}");
            serde_json::json!({})
        }
    };

    let mut s = state.write().await;
    s.active_model_id = Some(model_id.clone());
    s.model_loaded = true;
    s.progressive_loader = Some(loader);
    info!("Model loaded: {model_id} ({})", model_path.display());
    Json(serde_json::json!({
        "success": true,
        "model_id": model_id,
        "layer_a": layer_a,
    }))
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
    let data_dir = state.read().await.data_dir.clone();
    let path = data_dir.join("data/models").join(format!("{}.rvf", safe_id));
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
pub(crate) async fn list_lora_profiles(
    State(state): State<SharedState>,
) -> Json<serde_json::Value> {
    let data_dir = state.read().await.data_dir.clone();
    let profiles = scan_lora_profiles(&data_dir);
    Json(serde_json::json!({ "profiles": profiles }))
}

/// POST /api/v1/models/lora/activate —activate a LoRA adapter profile.
pub(crate) async fn activate_lora_profile(
    State(state): State<SharedState>,
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
    // Persist the active profile name in state so other handlers (e.g.
    // sona_profiles, model_layers) can report the actual active profile.
    // Previously this handler only logged + returned success without
    // persisting the selection.
    let mut s = state.write().await;
    s.active_sona_profile = Some(profile.clone());
    info!("LoRA profile activated: {profile}");
    Json(serde_json::json!({
        "success": true,
        "profile": profile,
        "active_sona_profile": s.active_sona_profile.clone(),
    }))
}

// ── Scanner helpers ─────────────────────────────────────────────────────────

/// Scan `{data_dir}/data/models/` for `.rvf` files and return metadata.
pub(crate) fn scan_model_files(data_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let dir = data_dir.join("data/models");
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

/// Scan `{data_dir}/data/models/` for `.lora.json` LoRA profile files.
pub(crate) fn scan_lora_profiles(data_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let dir = data_dir.join("data/models");
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
