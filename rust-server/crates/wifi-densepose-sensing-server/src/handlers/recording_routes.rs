//! Recording route handlers (list, start, stop, delete CSI recordings).

use axum::extract::{Path, State};
use axum::response::Json;
use tokio::sync::broadcast;
use tracing::{info, warn, debug, error};

use crate::SharedState;
use super::path_util::sanitize_path_segment;

// ── Panic safety guard ───────────────────────────────────────────────────────

/// Cleans up recording state on drop — safety net for writer task panics.
/// If the writer task panics before normal cleanup, this guard ensures
/// `recording_active` is not left permanently stuck at `true`.
///
/// IMPORTANT: Drop must be safe to run during panic unwind — no heap
/// allocation (serde_json::json!), no lock-acquiring macros (tracing::warn!),
/// only direct field assignments and eprintln for diagnostics.
struct RecordingGuard {
    state: SharedState,
    rec_id: String,
    should_cleanup: bool,
}

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        if !self.should_cleanup {
            return;
        }
        // Use try_write since we might be in a panic unwind context.
        if let Ok(mut s) = self.state.try_write() {
            if s.recording_current_id.as_deref() == Some(&self.rec_id) {
                s.recording_active = false;
                s.recording_start_time = None;
                s.recording_current_id = None;
                s.recording_stop_tx = None;
                for rec in s.recordings.iter_mut() {
                    if rec.get("id").and_then(|v| v.as_str()) == Some(self.rec_id.as_str()) {
                        rec["status"] = serde_json::Value::String("failed".into());
                        rec["error"] = serde_json::Value::String("writer task panicked".into());
                    }
                }
                // eprintln is async-signal-safe; tracing::warn! is NOT safe during unwind
                eprintln!("[WCES] Recording {}: writer task terminated unexpectedly, recording_active cleared", self.rec_id);
            }
        } else {
            // try_write failed — another task holds the lock. We can't block
            // (might be in unwind), so we rely on the operator noticing the
            // stuck recording_active via /api/v1/recording/list.
            eprintln!("[WCES] Recording {}: panic guard could not acquire write lock — recording_active may be stuck", self.rec_id);
        }
    }
}

// ── Recording Endpoints ─────────────────────────────────────────────────────

/// GET /api/v1/recording/list —list CSI recordings.
pub(crate) async fn list_recordings(
    State(state): State<SharedState>,
) -> Json<serde_json::Value> {
    let data_dir = state.read().await.data_dir.clone();
    let recordings = scan_recording_files(&data_dir);
    Json(serde_json::json!({ "recordings": recordings }))
}

/// POST /api/v1/recording/start —start recording CSI data.
pub(crate) async fn start_recording(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // ═══════════════════════════════════════════════════════════════════════
    // Block 1: validate request and build file path under read lock.
    // All operations are reads — write lock deferred to Block 2.
    // Does NOT subscribe to broadcast yet — that happens after
    // recording_active is committed, so pre-start frames are not captured.
    // ═══════════════════════════════════════════════════════════════════════
    let (rec_path, id) = {
        let s = state.read().await;
        if s.recording_active {
            return Json(serde_json::json!({
                "error": "recording already in progress",
                "success": false,
                "recording_id": s.recording_current_id,
            }));
        }
        // Auto-generate ID with sub-second precision to avoid collisions
        // from concurrent requests arriving in the same whole-second tick.
        let id = body.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                format!("rec_{}_{:03}", ts.as_secs(), ts.subsec_millis())
            });

        let safe_id = match sanitize_path_segment(&id) {
            Ok(s) => s,
            Err(_) => return Json(serde_json::json!({
                "error": "invalid recording id",
                "success": false,
            })),
        };

        let rec_path = s.data_dir.join("data/recordings").join(format!("{}.jsonl", safe_id));
        drop(s); // release read lock
        (rec_path, id)
    };

    // ═══════════════════════════════════════════════════════════════════════
    // Block 1.5: blocking filesystem I/O outside any lock.
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // Block 2: atomically commit all state changes under ONE write lock.
    // Re-check + subscribe + push entry + set recording_active + stop channel
    // all happen together — stop_recording always sees a consistent view.
    // ═══════════════════════════════════════════════════════════════════════
    let (rec_id, mut stop_rx, mut rx) = {
        let mut s = state.write().await;
        // Re-check: another request may have started a recording during I/O.
        if s.recording_active {
            let _ = std::fs::remove_file(&rec_path);
            return Json(serde_json::json!({
                "error": "recording already in progress",
                "success": false,
                "recording_id": s.recording_current_id,
            }));
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        // Subscribe AFTER we're about to commit — no pre-start frames leak in.
        let rx = s.tx.subscribe();
        s.recording_active = true;
        s.recording_start_time = Some(std::time::Instant::now());
        s.recording_current_id = Some(id.clone());
        s.recording_stop_tx = Some(stop_tx);
        // Push entry atomically — stops TOCTOU race with stop_recording.
        s.recordings.push(serde_json::json!({
            "id": id.clone(),
            "path": rec_path.display().to_string(),
            "status": "recording",
            "started_at": chrono_timestamp(),
            "frames": 0,
        }));
        let rec_id = id;
        drop(s); // Release lock immediately — don't block other handlers.
        (rec_id, stop_rx, rx)
    };

    let writer_state = state.clone();

    // Clone rec_id for the response — the original is moved into the spawn.
    let rec_id_response = rec_id.clone();

    // Spawn writer task with RecordingGuard for panic safety.
    tokio::spawn(async move {
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(file);
        let mut frame_count: u64 = 0;
        let mut write_error: Option<String> = None;

        // Panic safety net: if this task panics, the guard cleans up
        // recording_active so the system can recover without a restart.
        let mut guard = RecordingGuard {
            state: writer_state.clone(),
            rec_id: rec_id.clone(),
            should_cleanup: true,
        };

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
                result = stop_rx.changed() => {
                    match result {
                        Ok(()) => {
                            if *stop_rx.borrow() {
                                info!("Recording {rec_id}: stop signal received ({frame_count} frames)");
                                break;
                            }
                        }
                        Err(_closed) => {
                            // Sender dropped without sending — treat as stop.
                            info!("Recording {rec_id}: stop channel closed ({frame_count} frames)");
                            break;
                        }
                    }
                }
            }
        }
        let _ = writer.flush();

        // ── Always update recording entry and clean up active state ─────────
        {
            let mut s = writer_state.write().await;
            for rec in s.recordings.iter_mut() {
                if rec.get("id").and_then(|v| v.as_str()) == Some(rec_id.as_str()) {
                    rec["frames"] = serde_json::json!(frame_count);
                    if let Some(ref err_msg) = write_error {
                        rec["status"] = serde_json::json!("failed");
                        rec["error"] = serde_json::json!(err_msg);
                    }
                }
            }
            // Always reset active-recording fields so the system can recover
            // without a manual restart — covers broadcast close, stop signal,
            // and future exit paths that don't go through stop_recording.
            if s.recording_current_id.as_deref() == Some(&rec_id) {
                s.recording_active = false;
                s.recording_start_time = None;
                s.recording_current_id = None;
                s.recording_stop_tx = None;
            }
        }

        // Normal exit — disarm the panic guard.
        guard.should_cleanup = false;

        info!("Recording {rec_id} finished: {frame_count} frames written{}",
              if write_error.is_some() { " (with errors)" } else { "" });
    });

    info!("Recording started: {rec_id_response}");
    Json(serde_json::json!({ "success": true, "recording_id": rec_id_response }))
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

    let data_dir = state.read().await.data_dir.clone();
    let path = data_dir.join("data/recordings").join(format!("{}.jsonl", safe_id));

    // Atomically check active guard + remove from in-memory state under write lock.
    // File I/O (remove_file) happens outside the lock.
    {
        let mut s = state.write().await;
        // Reject if this recording is currently active — deleting an in-progress
        // recording's file while the writer task holds an open fd causes orphaned
        // writes (Linux) or delete failure (Windows), and leaves stale state.
        if s.recording_active && s.recording_current_id.as_deref() == Some(&safe_id) {
            return Json(serde_json::json!({
                "error": "cannot delete an active recording; stop it first",
                "success": false,
            }));
        }
        // Remove from in-memory list under the same lock.
        let existed = s.recordings.iter().any(|r| {
            r.get("id").and_then(|v| v.as_str()) == Some(safe_id)
        });
        if !existed && !path.exists() {
            return Json(serde_json::json!({ "error": "recording not found", "success": false }));
        }
        s.recordings.retain(|r| {
            r.get("id").and_then(|v| v.as_str()) != Some(safe_id)
        });
    }

    // File I/O outside any lock
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("Failed to delete recording {:?}: {}", path, e);
            return Json(serde_json::json!({ "error": format!("delete failed: {e}"), "success": false }));
        }
    }
    info!("Recording deleted: {safe_id}");
    Json(serde_json::json!({ "success": true, "deleted": safe_id }))
}

// ── Scanner helpers ─────────────────────────────────────────────────────────

/// Scan `{data_dir}/data/recordings/` for `.jsonl` files and return metadata.
pub(crate) fn scan_recording_files(data_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let dir = data_dir.join("data/recordings");
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
