//! HTTP and WebSocket server setup with graceful shutdown support.
//!
//! Extracted from `main.rs` to keep the entry point slim.

use std::net::SocketAddr;

use axum::{
    routing::{delete, get, post},
    Router,
};
use axum::http::HeaderValue;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{info, warn, error};

use crate::handlers::{ws, routes, model_routes, recording_routes, llm_routes};
use crate::rvf_container::{RvfBuilder, VitalSignConfig};
use crate::Args;
use crate::SharedState;
use wifi_densepose_sensing_server::graph_transformer;

/// Set up the WebSocket and HTTP servers, bind them to their ports,
/// serve with graceful shutdown, and save RVF on exit if configured.
pub(crate) async fn run_server(
    state: SharedState,
    args: &Args,
    bind_ip: std::net::IpAddr,
) -> anyhow::Result<()> {
    // ── WebSocket server on dedicated port ─────────────────────────────────────
    let ws_state = state.clone();
    let ws_app = Router::new()
        .route("/ws/sensing", get(ws::ws_sensing_handler))
        .route("/health", get(routes::health))
        .with_state(ws_state);

    let ws_addr = SocketAddr::from((bind_ip, args.ws_port));
    match tokio::net::TcpListener::bind(ws_addr).await {
        Ok(ws_listener) => {
            info!("WebSocket server listening on {ws_addr}");
            tokio::spawn(async move {
                axum::serve(ws_listener, ws_app).await.unwrap();
            });
        }
        Err(e) => {
            // WS is also available on the HTTP port, so this is non-fatal.
            warn!("WebSocket port {ws_addr} unavailable ({e}), using HTTP port for WS");
        }
    }

    // ── HTTP server (serves UI + full DensePose-compatible REST API) ──────────

    // UI files are served from project-root/ui/
    let project_ui_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()   // wifi-densepose-sensing-server/
        .parent().unwrap()   // crates/
        .parent().unwrap()   // rust-server/
        .join("ui");

    let http_app = Router::new()
        .route("/", get(routes::info_page))
        // Health endpoints (DensePose-compatible)
        .route("/health", get(routes::health))
        .route("/health/health", get(routes::health_system))
        .route("/health/live", get(routes::health_live))
        .route("/health/ready", get(routes::health_ready))
        .route("/health/version", get(routes::health_version))
        .route("/health/metrics", get(routes::health_metrics))
        // API info
        .route("/api/v1/info", get(routes::api_info))
        .route("/api/v1/status", get(routes::health_ready))
        .route("/api/v1/metrics", get(routes::health_metrics))
        // Sensing endpoints
        .route("/api/v1/sensing/latest", get(routes::latest))
        // Vital sign endpoints
        .route("/api/v1/vital-signs", get(routes::vital_signs_endpoint))
        .route("/api/v1/edge-vitals", get(routes::edge_vitals_endpoint))
        .route("/api/v1/wasm-events", get(routes::wasm_events_endpoint))
        // RVF model container info
        .route("/api/v1/model/info", get(routes::model_info))
        // Progressive loading & SONA endpoints (Phase 7-8)
        .route("/api/v1/model/layers", get(routes::model_layers))
        .route("/api/v1/model/segments", get(routes::model_segments))
        .route("/api/v1/model/sona/profiles", get(routes::sona_profiles))
        .route("/api/v1/model/sona/activate", post(routes::sona_activate))
        // Pose endpoints (WiFi-derived)
        .route("/api/v1/pose/current", get(routes::pose_current))
        .route("/api/v1/pose/stats", get(routes::pose_stats))
        .route("/api/v1/pose/zones/summary", get(routes::pose_zones_summary))
        // Stream endpoints
        .route("/api/v1/stream/status", get(routes::stream_status))
        .route("/api/v1/stream/pose", get(ws::ws_pose_handler))
        // Sensing WebSocket on the HTTP port so the UI can reach it without a second port
        .route("/ws/sensing", get(ws::ws_sensing_handler))
        // Model management endpoints (UI compatibility)
        .route("/api/v1/models", get(model_routes::list_models))
        .route("/api/v1/models/active", get(model_routes::get_active_model))
        .route("/api/v1/models/load", post(model_routes::load_model))
        .route("/api/v1/models/unload", post(model_routes::unload_model))
        .route("/api/v1/models/{id}", delete(model_routes::delete_model))
        .route("/api/v1/models/lora/profiles", get(model_routes::list_lora_profiles))
        .route("/api/v1/models/lora/activate", post(model_routes::activate_lora_profile))
        // Recording endpoints
        .route("/api/v1/recording/list", get(recording_routes::list_recordings))
        .route("/api/v1/recording/start", post(recording_routes::start_recording))
        .route("/api/v1/recording/stop", post(recording_routes::stop_recording))
        .route("/api/v1/recording/{id}", delete(recording_routes::delete_recording))
        // Training endpoints
        .route("/api/v1/train/status", get(routes::train_status))
        .route("/api/v1/train/start", post(routes::train_start))
        .route("/api/v1/train/stop", post(routes::train_stop))
        // Adaptive classifier endpoints
        .route("/api/v1/adaptive/train", post(routes::adaptive_train))
        .route("/api/v1/adaptive/status", get(routes::adaptive_status))
        .route("/api/v1/adaptive/unload", post(routes::adaptive_unload))
        // LLM analysis endpoints (P10d)
        .route("/api/v1/patients", get(llm_routes::llm_patients_list))
        .route("/api/v1/patients", post(llm_routes::llm_patient_register))
        .route("/api/v1/llm/analyze", post(llm_routes::llm_analyze))
        .route("/api/v1/llm/status", get(llm_routes::llm_status))
        // Agent analysis endpoints (Phase 4)
        .route("/api/v1/agent/analyze", post(llm_routes::agent_analyze))
        .route("/api/v1/agent/status", get(llm_routes::agent_status))
        // Static UI files — served from project-root/ui/
        .nest_service("/ui", ServeDir::new(&project_ui_root))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .with_state(state.clone());

    let http_addr = SocketAddr::from((bind_ip, args.http_port));
    let http_listener = match tokio::net::TcpListener::bind(http_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind HTTP {http_addr}: {e}");
            if cfg!(target_os = "windows") && (e.raw_os_error() == Some(10013)) {
                error!("  Port {} may be in Windows reserved range (Hyper-V/NAT).", args.http_port);
                error!("  Check: netsh interface ipv4 show excludedportrange protocol=tcp");
                error!("  Fix: use a different port, e.g. --http-port 3000");
                error!("  Or: net stop winnat && net start winnat (admin required, resets excluded ranges)");
            }
            std::process::exit(1);
        }
    };
    info!("HTTP server listening on {http_addr}");
    if bind_ip.is_unspecified() {
        info!("  Triage Dashboard: http://localhost:{}/ui/triage.html", args.http_port);
        info!("  Control Center:  http://localhost:{}/ui/index.html", args.http_port);
    } else {
        info!("  Triage Dashboard: http://{bind_ip}:{}/ui/triage.html", args.http_port);
        info!("  Control Center:  http://{bind_ip}:{}/ui/index.html", args.http_port);
    }

    // ── Run the HTTP server with graceful shutdown ────────────────────────────
    let shutdown_state = state.clone();
    let server = axum::serve(http_listener, http_app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C handler");
            info!("Shutdown signal received");
        });

    server.await?;

    // ── Save RVF container on shutdown if --save-rvf was specified ────────────
    let s = shutdown_state.read().await;
    if let Some(ref save_path) = s.save_rvf_path {
        info!("Saving RVF container to {}", save_path.display());
        let mut builder = RvfBuilder::new();
        builder.add_manifest(
            "wifi-densepose-sensing",
            env!("CARGO_PKG_VERSION"),
            "WiFi DensePose sensing model state",
        );
        builder.add_metadata(&serde_json::json!({
            "source": s.effective_source(),
            "total_ticks": s.tick,
            "total_detections": s.total_detections,
            "uptime_secs": s.start_time.elapsed().as_secs(),
        }));
        builder.add_vital_config(&VitalSignConfig::default());
        // Save transformer weights if a model is loaded, otherwise empty
        let weights: Vec<f32> = if s.model_loaded {
            // If we loaded via --model, the progressive loader has the weights
            // For now, save runtime state placeholder
            let tf = graph_transformer::CsiToPoseTransformer::new(Default::default());
            tf.flatten_weights()
        } else {
            Vec::new()
        };
        builder.add_weights(&weights);
        match builder.write_to_file(save_path) {
            Ok(()) => info!("  RVF saved ({} weight params)", weights.len()),
            Err(e) => error!("  Failed to save RVF: {e}"),
        }
    }

    info!("Server shut down cleanly");
    Ok(())
}
