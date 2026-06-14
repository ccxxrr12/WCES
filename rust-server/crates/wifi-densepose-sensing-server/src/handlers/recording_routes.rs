//! Recording route handlers (list, start, stop, delete CSI recordings).

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::response::Json;
use tokio::sync::broadcast;
use tracing::{info, warn, debug};

use crate::SharedState;
use super::path_util::sanitize_path_segment;

// ── Recording Endpoints ─────────────────────────────────────────────────────

/// GET /api/v1/recording/list —list CSI recordings.
pub(crate) async fn list_recordings() -> Json<serde_json::Value> {
    let recordings = scan_recording_files();
    Json(serde_json::json!({ "recordings": recordings }))
}

/// POST /api/v1/recording/start —start recording CSI data.
pub(crate) async fn start_recording(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    if s.recording_active {
        return Json(serde_json::json!({
            "error": "recording already in progress",
            "success": false,
            "recording_id": s.recording_current_id,
        }));
    }
    let id = body.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            format!("rec_{}", chrono_timestamp())
        });

    // Sanitize: prevent path traversal
    let safe_id = match sanitize_path_segment(&id) {
        Ok(s) => s,
        Err(_) => return Json(serde_json::json!({
            "error": "invalid recording id",
            "success": false,
        })),
    };

    // Create the recording file
    let rec_path = PathBuf::from("data/recordings").join(format!("{}.jsonl", safe_id));
    let file = match std::fs::File::create(&rec_path) {
        Ok(f) => f,
        Err(e) => {
            warn!("Failed to create recording file {:?}: {}", rec_path, e);
            return Json(serde_json::json!({
                "error": format!("cannot create file: {e}"),
                "success": false,
            }));
        }
    };

    // Create a stop signal channel
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    s.recording_active = true;
    s.recording_start_time = Some(std::time::Instant::now());
    s.recording_current_id = Some(id.clone());
    s.recording_stop_tx = Some(stop_tx);

    // Subscribe to the broadcast channel to capture CSI frames
    let mut rx = s.tx.subscribe();

    // Add initial recording entry
    s.recordings.push(serde_json::json!({
        "id": id,
        "path": rec_path.display().to_string(),
        "status": "recording",
        "started_at": chrono_timestamp(),
        "frames": 0,
    }));

    let rec_id = id.clone();

    // Clone SharedState for the writer task to update on error.
    let writer_state = state.clone();

    // Spawn writer task in background
    tokio::spawn(async move {
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(file);
        let mut frame_count: u64 = 0;
        let mut write_error: Option<String> = None;
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(frame_json) => {
                            if writeln!(writer, "{}", frame_json).is_err() {
                                warn!("Recording {rec_id}: write error, stopping");
                                write_error = Some("disk write error".into());
                                break;
                            }
                            frame_count += 1;
                            // Flush every 100 frames
                            if frame_count % 100 == 0 {
                                let _ = writer.flush();
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!("Recording {rec_id}: lagged {n} frames");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Recording {rec_id}: broadcast closed, stopping");
                            break;
                        }
                    }
                }
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        info!("Recording {rec_id}: stop signal received ({frame_count} frames)");
                        break;
                    }
                }
            }
        }
        let _ = writer.flush();

        // If writer errored, mark the recording as failed in shared state
        // so that stop_recording returns accurate status instead of silently
        // reporting "completed" for a truncated file.
        if let Some(ref err_msg) = write_error {
            if let Ok(mut s) = writer_state.write().await {
                if s.recording_active && s.recording_current_id.as_deref() == Some(&rec_id) {
                    s.recording_active = false;
                    s.recording_start_time = None;
                    s.recording_current_id = None;
                    s.recording_stop_tx = None;
                    for rec in s.recordings.iter_mut() {
                        if rec.get("id").and_then(|v| v.as_str()) == Some(rec_id.as_str()) {
                            rec["status"] = serde_json::json!("failed");
                            rec["error"] = serde_json::json!(err_msg);
                            rec["frames"] = serde_json::json!(frame_count);
                        }
                    }
                }
            }
        }

        info!("Recording {rec_id} finished: {frame_count} frames written{}",
              if write_error.is_some() { " (with errors)" } else { "" });
    });

    info!("Recording started: {id}");
    Json(serde_json::json!({ "success": true, "recording_id": id }))
}

/// POST /api/v1/recording/stop —stop recording CSI data.
pub(crate) async fn stop_recording(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let mut s = state.write().await;
    if !s.recording_active {
        return Json(serde_json::json!({
            "error": "no recording in progress",
            "success": false,
        }));
    }
    // Signal the writer task to stop
    if let Some(tx) = s.recording_stop_tx.take() {
        let _ = tx.send(true);
    }
    let duration_secs = s.recording_start_time
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    let rec_id = s.recording_current_id.take().unwrap_or_default();
    s.recording_active = false;
    s.recording_start_time = None;

    // Update the recording entry status.
    // Do NOT overwrite "failed" — the writer task may have crashed due to
    // disk error before we signalled it to stop.
    for rec in s.recordings.iter_mut() {
        if rec.get("id").and_then(|v| v.as_str()) == Some(rec_id.as_str()) {
            if rec.get("status").and_then(|v| v.as_str()) != Some("failed") {
                rec["status"] = serde_json::json!("completed");
            }
            rec["duration_secs"] = serde_json::json!(duration_secs);
        }
    }

    info!("Recording stopped: {rec_id} ({duration_secs}s)");
    Json(serde_json::json!({
        "success": true,
        "recording_id": rec_id,
        "duration_secs": duration_secs,
    }))
}

/// DELETE /api/v1/recording/:id —delete a recording file.
pub(crate) async fn delete_recording(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    // ADR-050: Sanitize path to prevent directory traversal
    let safe_id = match sanitize_path_segment(&id) {
        Ok(s) => s,
        Err(_) => return Json(serde_json::json!({ "error": "invalid recording id", "success": false })),
    };
    let path = PathBuf::from("data/recordings").join(format!("{}.jsonl", safe_id));
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("Failed to delete recording {:?}: {}", path, e);
            return Json(serde_json::json!({ "error": format!("delete failed: {e}"), "success": false }));
        }
        let mut s = state.write().await;
        s.recordings.retain(|r| {
            r.get("id").and_then(|v| v.as_str()) != Some(safe_id)
        });
        info!("Recording deleted: {safe_id}");
        Json(serde_json::json!({ "success": true, "deleted": safe_id }))
    } else {
        Json(serde_json::json!({ "error": "recording not found", "success": false }))
    }
}

// ── Scanner helpers ─────────────────────────────────────────────────────────

/// Scan `data/recordings/` for `.jsonl` files and return metadata.
pub(crate) fn scan_recording_files() -> Vec<serde_json::Value> {
    let dir = PathBuf::from("data/recordings");
    let mut recordings = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
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
                // Count lines (frames) —approximate for large files
                let frame_count = std::fs::read_to_string(&path)
                    .map(|s| s.lines().count())
                    .unwrap_or(0);
                recordings.push(serde_json::json!({
                    "id": name,
                    "name": name,
                    "path": path.display().to_string(),
                    "size_bytes": size,
                    "frames": frame_count,
                    "modified_epoch": modified,
                    "status": "completed",
                }));
            }
        }
    }
    recordings
}

/// Generate a simple timestamp string (epoch seconds) for recording IDs.
pub(crate) fn chrono_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
