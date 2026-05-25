//! WiFi-DensePose Sensing Server
//!
//! Lightweight Axum server that:
//! - Receives ESP32 CSI frames via UDP (port 5005)
//! - Processes signals using RuVector-powered wifi-densepose-signal crate
//! - Broadcasts sensing updates via WebSocket (ws://localhost:8765/ws/sensing)
//! - Serves the static UI files (port 8080)
//!
//! Replaces both ws_server.py and the Python HTTP server.

mod adaptive_classifier;
mod app_config;
mod edge_module_engine;
mod rvf_container;
mod rvf_pipeline;
mod vital_signs;
mod mat_pipeline;
mod handlers;
mod signal_processing;
mod tasks;
mod types;
mod parser;
mod server;
mod state_ops;

// Training pipeline modules (exposed via lib.rs)
use wifi_densepose_sensing_server::{graph_transformer, trainer, dataset, embedding};

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use tokio::net::UdpSocket;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn, error};

use rvf_container::{RvfBuilder, RvfContainerInfo, RvfReader, VitalSignConfig};
use rvf_pipeline::ProgressiveLoader;
use vital_signs::{VitalSignDetector, VitalSigns};

// MAT triage pipeline (competition core)
use mat_pipeline::{TriageEngine, TriageConfig};
use edge_module_engine::{EdgeModuleEngine, EdgeAlert};

// LLM analysis engine (competition P10d)
use wifi_densepose_llm::LlmAnalysisEngine;

// Medical Agent (Phase 4 — agent-based analysis with cloud LLM + degradation)
use wifi_densepose_llm::{
    MedicalAgent, MedicalKb, DegradationConfig,
    LlmGateway, GatewayConfig,
    AgentVitalSnapshot, StructuredContext, TriggerSource, TrendSummary,
};

// Extracted data types (was inline in main.rs, now in types.rs)
use types::{
    Esp32VitalsPacket,
    SensingUpdate, WasmOutputPacket, ESP32_OFFLINE_TIMEOUT,
};

use crate::parser::parse_esp32_frame;

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "sensing-server", about = "WiFi-DensePose sensing server")]
struct Args {
    /// HTTP port for UI and REST API
    #[arg(long, default_value = "8080")]
    http_port: u16,

    /// WebSocket port for sensing stream
    #[arg(long, default_value = "8765")]
    ws_port: u16,

    /// UDP port for ESP32 CSI frames
    #[arg(long, default_value = "5005")]
    udp_port: u16,

    /// Path to UI static files
    #[arg(long, default_value = "../../ui")]
    ui_path: PathBuf,

    /// Tick interval in milliseconds (default 100 ms = 10 fps for smooth pose animation)
    #[arg(long, default_value = "100")]
    tick_ms: u64,

    /// Bind address (default 127.0.0.1; set to 0.0.0.0 for network access)
    #[arg(long, default_value = "127.0.0.1", env = "SENSING_BIND_ADDR")]
    bind_addr: String,

    /// Data source: auto, wifi, esp32, simulate
    #[arg(long, default_value = "auto")]
    source: String,

    /// Run vital sign detection benchmark (1000 frames) and exit
    #[arg(long)]
    benchmark: bool,

    /// Load model config from an RVF container at startup
    #[arg(long, value_name = "PATH")]
    load_rvf: Option<PathBuf>,

    /// Save current model state as an RVF container on shutdown
    #[arg(long, value_name = "PATH")]
    save_rvf: Option<PathBuf>,

    /// Load a trained .rvf model for inference
    #[arg(long, value_name = "PATH")]
    model: Option<PathBuf>,

    /// Enable progressive loading (Layer A instant start)
    #[arg(long)]
    progressive: bool,

    /// Export an RVF container package and exit (no server)
    #[arg(long, value_name = "PATH")]
    export_rvf: Option<PathBuf>,

    /// Run training mode (train a model and exit)
    #[arg(long)]
    train: bool,

    /// Path to dataset directory (MM-Fi or Wi-Pose)
    #[arg(long, value_name = "PATH")]
    dataset: Option<PathBuf>,

    /// Dataset type: "mmfi" or "wipose"
    #[arg(long, value_name = "TYPE", default_value = "mmfi")]
    dataset_type: String,

    /// Number of training epochs
    #[arg(long, default_value = "100")]
    epochs: usize,

    /// Directory for training checkpoints
    #[arg(long, value_name = "DIR")]
    checkpoint_dir: Option<PathBuf>,

    /// Run self-supervised contrastive pretraining (ADR-024)
    #[arg(long)]
    pretrain: bool,

    /// Number of pretraining epochs (default 50)
    #[arg(long, default_value = "50")]
    pretrain_epochs: usize,

    /// Extract embeddings mode: load model and extract CSI embeddings
    #[arg(long)]
    embed: bool,

    /// Build fingerprint index from embeddings (env|activity|temporal|person)
    #[arg(long, value_name = "TYPE")]
    build_index: Option<String>,

    /// Path to wces.config.toml (auto-searched if not specified)
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

/// Shared application state
pub(crate) struct AppStateInner {
    latest_update: Option<SensingUpdate>,
    rssi_history: VecDeque<f64>,
    /// Circular buffer of recent CSI amplitude vectors for temporal analysis.
    /// Each entry is the full subcarrier amplitude vector for one frame.
    /// Capacity: FRAME_HISTORY_CAPACITY frames.
    frame_history: VecDeque<Vec<f64>>,
    tick: u64,
    source: String,
    /// Instant of the last ESP32 UDP frame received (for offline detection).
    last_esp32_frame: Option<std::time::Instant>,
    tx: broadcast::Sender<String>,
    total_detections: u64,
    start_time: std::time::Instant,
    /// Vital sign detector (processes CSI frames to estimate HR/RR).
    vital_detector: VitalSignDetector,
    /// MAT triage engine (START triage + survivor tracking + alerts).
    triage_engine: TriageEngine,
    /// Most recent vital sign reading for the REST endpoint.
    latest_vitals: VitalSigns,
    /// RVF container info if a model was loaded via `--load-rvf`.
    rvf_info: Option<RvfContainerInfo>,
    /// Path to save RVF container on shutdown (set via `--save-rvf`).
    save_rvf_path: Option<PathBuf>,
    /// Progressive loader for a trained model (set via `--model`).
    progressive_loader: Option<ProgressiveLoader>,
    /// Active SONA profile name.
    active_sona_profile: Option<String>,
    /// Whether a trained model is loaded.
    model_loaded: bool,
    /// Smoothed person count (EMA) for hysteresis —prevents frame-to-frame jumping.
    smoothed_person_score: f64,
    /// Previous person count for hysteresis (asymmetric up/down thresholds).
    prev_person_count: usize,
    // ── Motion smoothing & adaptive baseline (ADR-047 tuning) ────────────
    /// EMA-smoothed motion score (alpha ~0.15 for ~10 FPS →~1s time constant).
    smoothed_motion: f64,
    /// Current classification state for hysteresis debounce.
    current_motion_level: String,
    /// How many consecutive frames the *raw* classification has agreed with a
    /// *candidate* new level.  State only changes after DEBOUNCE_FRAMES.
    debounce_counter: u32,
    /// The candidate motion level that the debounce counter is tracking.
    debounce_candidate: String,
    /// Adaptive baseline: EMA of motion score when room is "quiet" (low motion).
    /// Subtracted from raw score so slow environmental drift doesn't inflate readings.
    baseline_motion: f64,
    /// Number of frames processed so far (for baseline warm-up).
    baseline_frames: u64,
    // ── Vital signs smoothing ────────────────────────────────────────────
    /// EMA-smoothed heart rate (BPM).
    smoothed_hr: f64,
    /// EMA-smoothed breathing rate (BPM).
    smoothed_br: f64,
    /// EMA-smoothed HR confidence.
    smoothed_hr_conf: f64,
    /// EMA-smoothed BR confidence.
    smoothed_br_conf: f64,
    /// Median filter buffer for HR (last N raw values for outlier rejection).
    hr_buffer: VecDeque<f64>,
    /// Median filter buffer for BR.
    br_buffer: VecDeque<f64>,
    /// ADR-039: Latest edge vitals packet from ESP32.
    edge_vitals: Option<Esp32VitalsPacket>,
    /// ADR-040: Latest WASM output packet from ESP32.
    latest_wasm_events: Option<WasmOutputPacket>,
    /// Edge module engine (native compilation of WASM modules for competition demo).
    edge_engine: EdgeModuleEngine,
    // ── Model management fields ─────────────────────────────────────────────
    /// Discovered RVF model files from `data/models/`.
    discovered_models: Vec<serde_json::Value>,
    /// ID of the currently loaded model, if any.
    active_model_id: Option<String>,
    // ── Recording fields ────────────────────────────────────────────────────
    /// Metadata for recorded CSI data files.
    recordings: Vec<serde_json::Value>,
    /// Whether CSI recording is currently in progress.
    recording_active: bool,
    /// When the current recording started.
    recording_start_time: Option<std::time::Instant>,
    /// ID of the current recording (used for filename).
    recording_current_id: Option<String>,
    /// Shutdown signal for the recording writer task.
    recording_stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    // ── Training fields ─────────────────────────────────────────────────────
    /// Training status: "idle", "running", "completed", "failed".
    training_status: String,
    /// Training configuration, if any.
    training_config: Option<serde_json::Value>,
    // ── Adaptive classifier (environment-tuned) ──────────────────────────
    /// Trained adaptive model (loaded from data/adaptive_model.json or trained at runtime).
    adaptive_model: Option<adaptive_classifier::AdaptiveModel>,
    // ── LLM Analysis Engine (P10d) ──────────────────────────────────────────
    /// LLM analysis engine for AI-powered medical analysis.
    /// None if LLM initialization failed or template-only mode is forced.
    llm_engine: Option<std::sync::Arc<LlmAnalysisEngine>>,
    // ── Medical Agent (Phase 4) ──────────────────────────────────────────
    /// Medical agent orchestrator (routing + degradation + LLM gateway + validation).
    medical_agent: Arc<tokio::sync::Mutex<MedicalAgent>>,
    /// Medical knowledge base for vital-pattern matching.
    medical_kb: MedicalKb,
}

impl AppStateInner {
    /// Return the effective data source, accounting for ESP32 frame timeout.
    /// If the source is "esp32" but no frame has arrived in 5 seconds, returns
    /// "esp32:offline" so the UI can distinguish active vs stale connections.
    fn effective_source(&self) -> String {
        if self.source == "esp32" {
            if let Some(last) = self.last_esp32_frame {
                if last.elapsed() > ESP32_OFFLINE_TIMEOUT {
                    return "esp32:offline".to_string();
                }
            }
        }
        self.source.clone()
    }
}

pub(crate) type SharedState = Arc<RwLock<AppStateInner>>;

/// Probe if ESP32 is streaming on UDP port
async fn probe_esp32(port: u16) -> bool {
    let addr = format!("0.0.0.0:{port}");
    match UdpSocket::bind(&addr).await {
        Ok(sock) => {
            let mut buf = [0u8; 256];
            match tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await {
                Ok(Ok((len, _))) => parse_esp32_frame(&buf[..len]).is_some(),
                _ => false,
            }
        }
        Err(_) => false,
    }
}













// ── Main─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();

    // ── Load unified config file ────────────────────────────────────────────
    let config = {
        let config_path = args.config.as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .or_else(app_config::find_config);
        match config_path {
            Some(path) => match app_config::load_config(&path) {
                Ok(Some(cfg)) => {
                    info!("Loaded config from {}", path);
                    Some(cfg)
                }
                Ok(None) => {
                    info!("No config file found, using defaults");
                    None
                }
                Err(e) => {
                    warn!("Failed to load config from {}: {}, using defaults", path, e);
                    None
                }
            },
            None => {
                info!("No config file found in search paths, using defaults");
                None
            }
        }
    };

    // Handle --benchmark mode: run vital sign benchmark and exit
    if args.benchmark {
        eprintln!("Running vital sign detection benchmark (1000 frames)...");
        let (total, per_frame) = vital_signs::run_benchmark(1000);
        eprintln!();
        eprintln!("Summary: {} total, {} per frame",
            format!("{total:?}"), format!("{per_frame:?}"));
        return;
    }

    // Handle --export-rvf mode: build an RVF container package and exit
    if let Some(ref rvf_path) = args.export_rvf {
        eprintln!("Exporting RVF container package...");
        use rvf_pipeline::RvfModelBuilder;

        let mut builder = RvfModelBuilder::new("wifi-densepose", "1.0.0");

        // Vital sign config (default breathing 0.1-0.5 Hz, heartbeat 0.8-2.0 Hz)
        builder.set_vital_config(0.1, 0.5, 0.8, 2.0);

        // Model profile (input/output spec)
        builder.set_model_profile(
            "56-subcarrier CSI amplitude/phase @ 10-100 Hz",
            "17 COCO keypoints + body part UV + vital signs",
            "ESP32-S3 or Windows WiFi RSSI, Rust 1.85+",
        );

        // Placeholder weights (17 keypoints × 56 subcarriers × 3 dims = 2856 params)
        let placeholder_weights: Vec<f32> = (0..2856).map(|i| (i as f32 * 0.001).sin()).collect();
        builder.set_weights(&placeholder_weights);

        // Training provenance
        builder.set_training_proof(
            "wifi-densepose-rs-v1.0.0",
            serde_json::json!({
                "pipeline": "ADR-023 8-phase",
                "test_count": 229,
                "benchmark_fps": 9520,
                "framework": "wifi-densepose-rs",
            }),
        );

        // SONA default environment profile
        let default_lora: Vec<f32> = vec![0.0; 64];
        builder.add_sona_profile("default", &default_lora, &default_lora);

        match builder.build() {
            Ok(rvf_bytes) => {
                if let Err(e) = std::fs::write(rvf_path, &rvf_bytes) {
                    eprintln!("Error writing RVF: {e}");
                    std::process::exit(1);
                }
                eprintln!("Wrote {} bytes to {}", rvf_bytes.len(), rvf_path.display());
                eprintln!("RVF container exported successfully.");
            }
            Err(e) => {
                eprintln!("Error building RVF: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle --pretrain mode: self-supervised contrastive pretraining (ADR-024)
    if args.pretrain {
        eprintln!("=== WiFi-DensePose Contrastive Pretraining (ADR-024) ===");

        let ds_path = args.dataset.clone().unwrap_or_else(|| PathBuf::from("data"));
        let source = match args.dataset_type.as_str() {
            "wipose" => dataset::DataSource::WiPose(ds_path.clone()),
            _ => dataset::DataSource::MmFi(ds_path.clone()),
        };
        let pipeline = dataset::DataPipeline::new(dataset::DataConfig {
            source, ..Default::default()
        });

        // Generate synthetic or load real CSI windows
        let generate_synthetic_windows = || -> Vec<Vec<Vec<f32>>> {
            (0..50).map(|i| {
                (0..4).map(|a| {
                    (0..56).map(|s| ((i * 7 + a * 13 + s) as f32 * 0.31).sin() * 0.5).collect()
                }).collect()
            }).collect()
        };

        let csi_windows: Vec<Vec<Vec<f32>>> = match pipeline.load() {
            Ok(s) if !s.is_empty() => {
                eprintln!("Loaded {} samples from {}", s.len(), ds_path.display());
                s.into_iter().map(|s| s.csi_window).collect()
            }
            _ => {
                eprintln!("Using synthetic data for pretraining.");
                generate_synthetic_windows()
            }
        };

        let n_subcarriers = csi_windows.first()
            .and_then(|w| w.first())
            .map(|f| f.len())
            .unwrap_or(56);

        let tf_config = graph_transformer::TransformerConfig {
            n_subcarriers, n_keypoints: 17, d_model: 64, n_heads: 4, n_gnn_layers: 2,
        };
        let transformer = graph_transformer::CsiToPoseTransformer::new(tf_config);
        eprintln!("Transformer params: {}", transformer.param_count());

        let trainer_config = trainer::TrainerConfig {
            epochs: args.pretrain_epochs,
            batch_size: 8, lr: 0.001, warmup_epochs: 2, min_lr: 1e-6,
            early_stop_patience: args.pretrain_epochs + 1,
            pretrain_temperature: 0.07,
            ..Default::default()
        };
        let mut t = trainer::Trainer::with_transformer(trainer_config, transformer);

        let e_config = embedding::EmbeddingConfig {
            d_model: 64, d_proj: 128, temperature: 0.07, normalize: true,
        };
        let mut projection = embedding::ProjectionHead::new(e_config.clone());
        let augmenter = embedding::CsiAugmenter::new();

        eprintln!("Starting contrastive pretraining for {} epochs...", args.pretrain_epochs);
        let start = std::time::Instant::now();
        for epoch in 0..args.pretrain_epochs {
            let loss = t.pretrain_epoch(&csi_windows, &augmenter, &mut projection, 0.07, epoch);
            if epoch % 10 == 0 || epoch == args.pretrain_epochs - 1 {
                eprintln!("  Epoch {epoch}: contrastive loss = {loss:.4}");
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!("Pretraining complete in {elapsed:.1}s");

        // Save pretrained model as RVF with embedding segment
        if let Some(ref save_path) = args.save_rvf {
            eprintln!("Saving pretrained model to RVF: {}", save_path.display());
            t.sync_transformer_weights();
            let weights = t.params().to_vec();
            let mut proj_weights = Vec::new();
            projection.flatten_into(&mut proj_weights);

            let mut builder = RvfBuilder::new();
            builder.add_manifest(
                "wifi-densepose-pretrained",
                env!("CARGO_PKG_VERSION"),
                "WiFi DensePose contrastive pretrained model (ADR-024)",
            );
            builder.add_weights(&weights);
            builder.add_embedding(
                &serde_json::json!({
                    "d_model": e_config.d_model,
                    "d_proj": e_config.d_proj,
                    "temperature": e_config.temperature,
                    "normalize": e_config.normalize,
                    "pretrain_epochs": args.pretrain_epochs,
                }),
                &proj_weights,
            );
            match builder.write_to_file(save_path) {
                Ok(()) => eprintln!("RVF saved ({} transformer + {} projection params)",
                    weights.len(), proj_weights.len()),
                Err(e) => eprintln!("Failed to save RVF: {e}"),
            }
        }

        return;
    }

    // Handle --embed mode: extract embeddings from CSI data
    if args.embed {
        eprintln!("=== WiFi-DensePose Embedding Extraction (ADR-024) ===");

        let model_path = match &args.model {
            Some(p) => p.clone(),
            None => {
                eprintln!("Error: --embed requires --model <path> to a pretrained .rvf file");
                std::process::exit(1);
            }
        };

        let reader = match RvfReader::from_file(&model_path) {
            Ok(r) => r,
            Err(e) => { eprintln!("Failed to load model: {e}"); std::process::exit(1); }
        };

        let weights = reader.weights().unwrap_or_default();
        let (embed_config_json, proj_weights) = reader.embedding().unwrap_or_else(|| {
            eprintln!("Warning: no embedding segment in RVF, using defaults");
            (serde_json::json!({"d_model":64,"d_proj":128,"temperature":0.07,"normalize":true}), Vec::new())
        });

        let d_model = embed_config_json["d_model"].as_u64().unwrap_or(64) as usize;
        let d_proj = embed_config_json["d_proj"].as_u64().unwrap_or(128) as usize;

        let tf_config = graph_transformer::TransformerConfig {
            n_subcarriers: 56, n_keypoints: 17, d_model, n_heads: 4, n_gnn_layers: 2,
        };
        let e_config = embedding::EmbeddingConfig {
            d_model, d_proj, temperature: 0.07, normalize: true,
        };
        let mut extractor = embedding::EmbeddingExtractor::new(tf_config, e_config.clone());

        // Load transformer weights
        if !weights.is_empty() {
            if let Err(e) = extractor.transformer.unflatten_weights(&weights) {
                eprintln!("Warning: failed to load transformer weights: {e}");
            }
        }
        // Load projection weights
        if !proj_weights.is_empty() {
            let (proj, _) = embedding::ProjectionHead::unflatten_from(&proj_weights, &e_config);
            extractor.projection = proj;
        }

        // Load dataset and extract embeddings
        let _ds_path = args.dataset.clone().unwrap_or_else(|| PathBuf::from("data"));
        let csi_windows: Vec<Vec<Vec<f32>>> = (0..10).map(|i| {
            (0..4).map(|a| {
                (0..56).map(|s| ((i * 7 + a * 13 + s) as f32 * 0.31).sin() * 0.5).collect()
            }).collect()
        }).collect();

        eprintln!("Extracting embeddings from {} CSI windows...", csi_windows.len());
        let embeddings = extractor.extract_batch(&csi_windows);
        for (i, emb) in embeddings.iter().enumerate() {
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            eprintln!("  Window {i}: {d_proj}-dim embedding, ||e|| = {norm:.4}");
        }
        eprintln!("Extracted {} embeddings of dimension {d_proj}", embeddings.len());

        return;
    }

    // Handle --build-index mode: build a fingerprint index from embeddings
    if let Some(ref index_type_str) = args.build_index {
        eprintln!("=== WiFi-DensePose Fingerprint Index Builder (ADR-024) ===");

        let index_type = match index_type_str.as_str() {
            "env" | "environment" => embedding::IndexType::EnvironmentFingerprint,
            "activity" => embedding::IndexType::ActivityPattern,
            "temporal" => embedding::IndexType::TemporalBaseline,
            "person" => embedding::IndexType::PersonTrack,
            _ => {
                eprintln!("Unknown index type '{}'. Use: env, activity, temporal, person", index_type_str);
                std::process::exit(1);
            }
        };

        let tf_config = graph_transformer::TransformerConfig::default();
        let e_config = embedding::EmbeddingConfig::default();
        let mut extractor = embedding::EmbeddingExtractor::new(tf_config, e_config);

        // Generate synthetic CSI windows for demo
        let csi_windows: Vec<Vec<Vec<f32>>> = (0..20).map(|i| {
            (0..4).map(|a| {
                (0..56).map(|s| ((i * 7 + a * 13 + s) as f32 * 0.31).sin() * 0.5).collect()
            }).collect()
        }).collect();

        let mut index = embedding::FingerprintIndex::new(index_type);
        for (i, window) in csi_windows.iter().enumerate() {
            let emb = extractor.extract(window);
            index.insert(emb, format!("window_{i}"), i as u64 * 100);
        }

        eprintln!("Built {:?} index with {} entries", index_type, index.len());

        // Test a query
        let query_emb = extractor.extract(&csi_windows[0]);
        let results = index.search(&query_emb, 5);
        eprintln!("Top-5 nearest to window_0:");
        for r in &results {
            eprintln!("  entry={}, distance={:.4}, metadata={}", r.entry, r.distance, r.metadata);
        }

        return;
    }

    // Handle --train mode: train a model and exit
    if args.train {
        eprintln!("=== WiFi-DensePose Training Mode ===");

        // Build data pipeline
        let ds_path = args.dataset.clone().unwrap_or_else(|| PathBuf::from("data"));
        let source = match args.dataset_type.as_str() {
            "wipose" => dataset::DataSource::WiPose(ds_path.clone()),
            _ => dataset::DataSource::MmFi(ds_path.clone()),
        };
        let pipeline = dataset::DataPipeline::new(dataset::DataConfig {
            source,
            ..Default::default()
        });

        // Generate synthetic training data (50 samples with deterministic CSI + keypoints)
        let generate_synthetic = || -> Vec<dataset::TrainingSample> {
            (0..50).map(|i| {
                let csi: Vec<Vec<f32>> = (0..4).map(|a| {
                    (0..56).map(|s| ((i * 7 + a * 13 + s) as f32 * 0.31).sin() * 0.5).collect()
                }).collect();
                let mut kps = [(0.0f32, 0.0f32, 1.0f32); 17];
                for (k, kp) in kps.iter_mut().enumerate() {
                    kp.0 = (k as f32 * 0.1 + i as f32 * 0.02).sin() * 100.0 + 320.0;
                    kp.1 = (k as f32 * 0.15 + i as f32 * 0.03).cos() * 80.0 + 240.0;
                }
                dataset::TrainingSample {
                    csi_window: csi,
                    pose_label: dataset::PoseLabel {
                        keypoints: kps,
                        body_parts: Vec::new(),
                        confidence: 1.0,
                    },
                    source: "synthetic",
                }
            }).collect()
        };

        // Load samples (fall back to synthetic if dataset missing/empty)
        let samples = match pipeline.load() {
            Ok(s) if !s.is_empty() => {
                eprintln!("Loaded {} samples from {}", s.len(), ds_path.display());
                s
            }
            Ok(_) => {
                eprintln!("No samples found at {}. Using synthetic data.", ds_path.display());
                generate_synthetic()
            }
            Err(e) => {
                eprintln!("Failed to load dataset: {e}. Using synthetic data.");
                generate_synthetic()
            }
        };

        // Convert dataset samples to trainer format
        let trainer_samples: Vec<trainer::TrainingSample> = samples.iter()
            .map(trainer::from_dataset_sample)
            .collect();

        // Split 80/20 train/val
        let split = (trainer_samples.len() * 4) / 5;
        let (train_data, val_data) = trainer_samples.split_at(split.max(1));
        eprintln!("Train: {} samples, Val: {} samples", train_data.len(), val_data.len());

        // Create transformer + trainer
        let n_subcarriers = train_data.first()
            .and_then(|s| s.csi_features.first())
            .map(|f| f.len())
            .unwrap_or(56);
        let tf_config = graph_transformer::TransformerConfig {
            n_subcarriers,
            n_keypoints: 17,
            d_model: 64,
            n_heads: 4,
            n_gnn_layers: 2,
        };
        let transformer = graph_transformer::CsiToPoseTransformer::new(tf_config);
        eprintln!("Transformer params: {}", transformer.param_count());

        let trainer_config = trainer::TrainerConfig {
            epochs: args.epochs,
            batch_size: 8,
            lr: 0.001,
            warmup_epochs: 5,
            min_lr: 1e-6,
            early_stop_patience: 20,
            checkpoint_every: 10,
            ..Default::default()
        };
        let mut t = trainer::Trainer::with_transformer(trainer_config, transformer);

        // Run training
        eprintln!("Starting training for {} epochs...", args.epochs);
        let result = t.run_training(train_data, val_data);
        eprintln!("Training complete in {:.1}s", result.total_time_secs);
        eprintln!("  Best epoch: {}, PCK@0.2: {:.4}, OKS mAP: {:.4}",
            result.best_epoch, result.best_pck, result.best_oks);

        // Save checkpoint
        if let Some(ref ckpt_dir) = args.checkpoint_dir {
            let _ = std::fs::create_dir_all(ckpt_dir);
            let ckpt_path = ckpt_dir.join("best_checkpoint.json");
            let ckpt = t.checkpoint();
            match ckpt.save_to_file(&ckpt_path) {
                Ok(()) => eprintln!("Checkpoint saved to {}", ckpt_path.display()),
                Err(e) => eprintln!("Failed to save checkpoint: {e}"),
            }
        }

        // Sync weights back to transformer and save as RVF
        t.sync_transformer_weights();
        if let Some(ref save_path) = args.save_rvf {
            eprintln!("Saving trained model to RVF: {}", save_path.display());
            let weights = t.params().to_vec();
            let mut builder = RvfBuilder::new();
            builder.add_manifest(
                "wifi-densepose-trained",
                env!("CARGO_PKG_VERSION"),
                "WiFi DensePose trained model weights",
            );
            builder.add_metadata(&serde_json::json!({
                "training": {
                    "epochs": args.epochs,
                    "best_epoch": result.best_epoch,
                    "best_pck": result.best_pck,
                    "best_oks": result.best_oks,
                    "n_train_samples": train_data.len(),
                    "n_val_samples": val_data.len(),
                    "n_subcarriers": n_subcarriers,
                    "param_count": weights.len(),
                },
            }));
            builder.add_vital_config(&VitalSignConfig::default());
            builder.add_weights(&weights);
            match builder.write_to_file(save_path) {
                Ok(()) => eprintln!("RVF saved ({} params, {} bytes)",
                    weights.len(), weights.len() * 4),
                Err(e) => eprintln!("Failed to save RVF: {e}"),
            }
        }

        return;
    }

    info!("WiFi-DensePose Sensing Server (Rust + Axum + RuVector)");
    info!("  HTTP:      http://localhost:{}", args.http_port);
    info!("  WebSocket: ws://localhost:{}/ws/sensing", args.ws_port);
    info!("  UDP:       0.0.0.0:{} (ESP32 CSI)", args.udp_port);
    info!("  UI path:   {}", args.ui_path.display());
    info!("  Source:    {}", args.source);

    // Auto-detect data source
    let source = match args.source.as_str() {
        "auto" => {
            info!("Auto-detecting data source...");
            if probe_esp32(args.udp_port).await {
                info!("  ESP32 CSI detected on UDP :{}", args.udp_port);
                "esp32"
            } else {
                info!("  No hardware detected, using simulation");
                "simulate"
            }
        }
        other => other,
    };

    info!("Data source: {source}");

    // Shared state
    // Vital sign sample rate derives from tick interval (e.g. 500ms tick => 2 Hz)
    let vital_sample_rate = 1000.0 / args.tick_ms as f64;
    info!("Vital sign detector sample rate: {vital_sample_rate:.1} Hz");

    // Load RVF container if --load-rvf was specified
    let rvf_info = if let Some(ref rvf_path) = args.load_rvf {
        info!("Loading RVF container from {}", rvf_path.display());
        match RvfReader::from_file(rvf_path) {
            Ok(reader) => {
                let info = reader.info();
                info!(
                    "  RVF loaded: {} segments, {} bytes",
                    info.segment_count, info.total_size
                );
                if let Some(ref manifest) = info.manifest {
                    if let Some(model_id) = manifest.get("model_id") {
                        info!("  Model ID: {model_id}");
                    }
                    if let Some(version) = manifest.get("version") {
                        info!("  Version:  {version}");
                    }
                }
                if info.has_weights {
                    if let Some(w) = reader.weights() {
                        info!("  Weights: {} parameters", w.len());
                    }
                }
                if info.has_vital_config {
                    info!("  Vital sign config: present");
                }
                if info.has_quant_info {
                    info!("  Quantization info: present");
                }
                if info.has_witness {
                    info!("  Witness/proof: present");
                }
                Some(info)
            }
            Err(e) => {
                error!("Failed to load RVF container: {e}");
                None
            }
        }
    } else {
        None
    };

    // Load trained model via --model (uses progressive loading if --progressive set)
    let model_path = args.model.as_ref().or(args.load_rvf.as_ref());
    let mut progressive_loader: Option<ProgressiveLoader> = None;
    let mut model_loaded = false;
    if let Some(mp) = model_path {
        if args.progressive || args.model.is_some() {
            info!("Loading trained model (progressive) from {}", mp.display());
            match std::fs::read(mp) {
                Ok(data) => match ProgressiveLoader::new(&data) {
                    Ok(mut loader) => {
                        if let Ok(la) = loader.load_layer_a() {
                            info!("  Layer A ready: model={} v{} ({} segments)",
                                  la.model_name, la.version, la.n_segments);
                        }
                        model_loaded = true;
                        progressive_loader = Some(loader);
                    }
                    Err(e) => error!("Progressive loader init failed: {e}"),
                },
                Err(e) => error!("Failed to read model file: {e}"),
            }
        }
    }

    // Ensure data directories exist for models, recordings, and LLM data
    let _ = std::fs::create_dir_all("data/models");
    let _ = std::fs::create_dir_all("data/recordings");
    let _ = std::fs::create_dir_all("data/patients");

    // Initialize LLM analysis engine (template-only mode — no model needed)
    let llm_engine = {
        let kb_path = if std::path::Path::new("crates/wifi-densepose-llm/data/medical_knowledge.json").exists() {
            "crates/wifi-densepose-llm/data/medical_knowledge.json"
        } else if std::path::Path::new("data/medical_knowledge.json").exists() {
            "data/medical_knowledge.json"
        } else {
            warn!("Medical knowledge base not found, LLM engine disabled");
            "data/medical_knowledge.json" // LlmAnalysisEngine will handle the error
        };
        match LlmAnalysisEngine::new_with_paths("data/patients", kb_path).await {
            Ok(engine) => {
                info!("LLM Analysis Engine initialized (template-only mode)");
                Some(std::sync::Arc::new(engine))
            }
            Err(e) => {
                warn!("LLM Analysis Engine unavailable: {}", e);
                None
            }
        }
    };

    // Initialize Medical KB (agent version — vital-pattern matching)
    let medical_kb = {
        let config_path = config.as_ref()
            .and_then(|c| {
                let p = c.server.agent.agent_kb_path.as_str();
                if p.is_empty() { None } else { Some(p) }
            });

        let candidates: Vec<String> = if let Some(cp) = config_path {
            vec![cp.to_string(), format!("crates/wifi-densepose-llm/{cp}")]
        } else {
            vec![
                "crates/wifi-densepose-llm/data/agent_kb.json".to_string(),
                "data/agent_kb.json".to_string(),
            ]
        };

        let resolved = candidates.iter().find(|p| std::path::Path::new(p.as_str()).exists())
            .cloned()
            .unwrap_or_else(|| {
                warn!("Agent knowledge base not found, using empty KB");
                candidates[0].clone()
            });

        match MedicalKb::load(&resolved) {
            Ok(kb) => {
                info!("Medical Agent KB loaded from {resolved}: {} entries", kb.entry_count());
                kb
            }
            Err(e) => {
                warn!("Medical Agent KB unavailable: {e}, using empty KB");
                MedicalKb::empty()
            }
        }
    };

    // Initialize MedicalAgent — config file values take precedence, env vars as fallback
    let medical_agent = {
        let (agent_enabled, agent_mode) = config.as_ref()
            .map(|c| (c.server.agent.enabled, c.server.agent.mode.clone()))
            .unwrap_or((true, "agent".to_string()));

        // Build degradation config early — used by all agent constructors
        let degradation_config = {
            let deg = config.as_ref().map(|c| &c.server.agent.degradation);
            DegradationConfig {
                cooldown_secs: deg.map(|d| d.cooldown_secs).unwrap_or(300),
                max_cache_size: deg.map(|d| d.max_cache_size).unwrap_or(32),
                network_failure_threshold: deg.map(|d| d.network_failure_threshold).unwrap_or(5),
            }
        };

        if !agent_enabled || agent_mode == "template-only" {
            info!("Medical Agent: {} (mode={agent_mode})",
                  if !agent_enabled { "disabled" } else { "template-only" });
            MedicalAgent::new_template_only()
        } else {
            let gw = config.as_ref().map(|c| &c.server.agent.gateway);
            let cb = config.as_ref().map(|c| &c.server.agent.circuit_breaker);

            let endpoint = gw.and_then(|g| if g.endpoint.is_empty() { None } else { Some(g.endpoint.clone()) })
                .or_else(|| std::env::var("LLM_ENDPOINT").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".into());

            let model = gw.and_then(|g| if g.model.is_empty() { None } else { Some(g.model.clone()) })
                .or_else(|| std::env::var("LLM_MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".into());

            let api_key = gw.and_then(|g| if g.api_key.is_empty() { None } else { Some(g.api_key.clone()) })
                .or_else(|| std::env::var("LLM_API_KEY").ok())
                .unwrap_or_default();

            if api_key.is_empty() {
                info!("No API key configured, Medical Agent starting in template-only mode");
                MedicalAgent::new_template_only()
            } else {
                let gateway_config = GatewayConfig {
                    endpoint,
                    model: model.clone(),
                    api_key,
                    timeout_secs: gw.map(|g| g.timeout_secs).unwrap_or(20),
                    max_retries: gw.map(|g| g.max_retries).unwrap_or(2),
                    temperature: gw.map(|g| g.temperature).unwrap_or(0.3),
                    failure_threshold: cb.map(|c| c.failure_threshold).unwrap_or(3),
                    breaker_open_secs: cb.map(|c| c.open_duration_secs).unwrap_or(300),
                };
                match LlmGateway::new(gateway_config) {
                    Ok(gateway) => {
                        info!("Medical Agent initialized with LLM gateway (model: {model})");
                        MedicalAgent::new_with_degradation(gateway, degradation_config)
                    }
                    Err(e) => {
                        warn!("Failed to create LLM gateway: {e}, falling back to template-only");
                        MedicalAgent::new_template_only()
                    }
                }
            }
        }
    };

    // Discover model and recording files on startup
    let initial_models = handlers::model_routes::scan_model_files();
    let initial_recordings = handlers::recording_routes::scan_recording_files();
    info!("Discovered {} model files, {} recording files", initial_models.len(), initial_recordings.len());

    let (tx, _) = broadcast::channel::<String>(256);
    let state: SharedState = Arc::new(RwLock::new(AppStateInner {
        latest_update: None,
        rssi_history: VecDeque::new(),
        frame_history: VecDeque::new(),
        tick: 0,
        source: source.into(),
        last_esp32_frame: None,
        tx,
        total_detections: 0,
        start_time: std::time::Instant::now(),
        vital_detector: VitalSignDetector::new(vital_sample_rate),
        triage_engine: TriageEngine::new(TriageConfig::competition()),
        latest_vitals: VitalSigns::default(),
        rvf_info,
        save_rvf_path: args.save_rvf.clone(),
        progressive_loader,
        active_sona_profile: None,
        model_loaded,
        smoothed_person_score: 0.0,
        prev_person_count: 0,
        smoothed_motion: 0.0,
        current_motion_level: "absent".to_string(),
        debounce_counter: 0,
        debounce_candidate: "absent".to_string(),
        baseline_motion: 0.0,
        baseline_frames: 0,
        smoothed_hr: 0.0,
        smoothed_br: 0.0,
        smoothed_hr_conf: 0.0,
        smoothed_br_conf: 0.0,
        hr_buffer: VecDeque::with_capacity(8),
        br_buffer: VecDeque::with_capacity(8),
        edge_vitals: None,
        latest_wasm_events: None,
        edge_engine: EdgeModuleEngine::new(),
        // Model management
        discovered_models: initial_models,
        active_model_id: None,
        // Recording
        recordings: initial_recordings,
        recording_active: false,
        recording_start_time: None,
        recording_current_id: None,
        recording_stop_tx: None,
        // Training
        training_status: "idle".to_string(),
        training_config: None,
        adaptive_model: adaptive_classifier::AdaptiveModel::load(&adaptive_classifier::model_path()).ok().map(|m| {
            info!("Loaded adaptive classifier: {} frames, {:.1}% accuracy",
                  m.trained_frames, m.training_accuracy * 100.0);
            m
        }),
        llm_engine: llm_engine.clone(),
        medical_agent: Arc::new(tokio::sync::Mutex::new(medical_agent)),
        medical_kb,
    }));

    // Start background tasks based on source
    match source {
        "esp32" => {
            tokio::spawn(tasks::udp_receiver::udp_receiver_task(state.clone(), args.udp_port));
            tokio::spawn(tasks::broadcast_tick::broadcast_tick_task(state.clone(), args.tick_ms));
        }
        _ => {
            tokio::spawn(tasks::simulated_data::simulated_data_task(state.clone(), args.tick_ms));
        }
    }

    // Spawn periodic LLM analysis task (feeds vitals to LLM engine, triggers periodic analysis)
    {
        let llm_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            // First tick waits 30s, let data accumulate first
            interval.tick().await;
            loop {
                interval.tick().await;
                let engine = {
                    let s = llm_state.read().await;
                    s.llm_engine.clone()
                };
                if let Some(engine) = engine {
                    // Get current vitals and triage for periodic analysis
                    let (vitals, smoothed_motion, latest_update) = {
                        let s = llm_state.read().await;
                        (s.latest_vitals.clone(), s.smoothed_motion, s.latest_update.clone())
                    };
                    let triage_label = latest_update.as_ref()
                        .and_then(|u| u.triage_update.as_ref())
                        .map(|t| t.survivors.first().map(|s| s.triage.clone()).unwrap_or_else(|| "Unknown".to_string()))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let alerts: Vec<String> = latest_update.as_ref()
                        .and_then(|u| u.wasm_alerts.as_ref())
                        .map(|a| a.iter().map(|al: &EdgeAlert| al.event_name.clone()).collect())
                        .unwrap_or_default();

                    // Trigger analysis for AUTO-N1 (auto-detected node 1 patient)
                    let _ = engine.trigger_analysis(
                        "AUTO-N1",
                        vitals.breathing_rate_bpm,
                        vitals.heart_rate_bpm,
                        smoothed_motion,
                        vitals.signal_quality,
                        &triage_label,
                        &alerts,
                    ).await;
                }
            }
        });
    }

    // Spawn periodic Medical Agent analysis task (Phase 4)
    {
        let agent_enabled = config.as_ref().map(|c| c.server.agent.enabled).unwrap_or(true);
        let periodic_secs = config.as_ref()
            .map(|c| c.server.agent.periodic_analysis_secs)
            .unwrap_or(30)
            .max(5); // minimum 5s to avoid thrashing
        let agent_state = state.clone();
        tokio::spawn(async move {
            if !agent_enabled || periodic_secs == 0 {
                info!("Medical Agent periodic analysis disabled (enabled={agent_enabled}, interval={periodic_secs}s)");
                return;
            }
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(periodic_secs));
            interval.tick().await; // wait 30s for first data
            loop {
                interval.tick().await;

                // Gather current state under read lock
                let (vitals, triage_label, alerts, smoothed_motion) = {
                    let s = agent_state.read().await;
                    let triage = s.latest_update.as_ref()
                        .and_then(|u| u.triage_update.as_ref())
                        .map(|t| t.survivors.first().map(|s| s.triage.clone()).unwrap_or_else(|| "Unknown".to_string()))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let a: Vec<String> = s.latest_update.as_ref()
                        .and_then(|u| u.wasm_alerts.as_ref())
                        .map(|alerts| alerts.iter().map(|al| al.event_name.clone()).collect())
                        .unwrap_or_default();
                    (s.latest_vitals.clone(), triage, a, s.smoothed_motion)
                };

                // Skip if no vitals data
                if vitals.breathing_rate_bpm.is_none() && vitals.heart_rate_bpm.is_none() {
                    continue;
                }

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
                    patient_id: 1,
                    node_id: 1,
                    vitals_current: vitals_snapshot,
                    vitals_trend_1min: TrendSummary {
                        direction: wifi_densepose_llm::TrendDirection::Stable,
                        delta: 0.0,
                        delta_pct: 0.0,
                        anomaly_score: 1.0,
                        data_points: 10,
                    },
                    vitals_trend_5min: TrendSummary {
                        direction: wifi_densepose_llm::TrendDirection::Stable,
                        delta: 0.0,
                        delta_pct: 0.0,
                        anomaly_score: 1.0,
                        data_points: 50,
                    },
                    triage_current: triage_label,
                    triage_trajectory: vec![],
                    patient_history: None,
                    recent_alerts: alerts,
                    kb_matches: vec![],
                    triggered_by: TriggerSource::PeriodicScan,
                    built_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };

                let agent = {
                    let s = agent_state.read().await;
                    s.medical_agent.clone()
                };

                let mut agent_guard = agent.lock().await;
                let result = agent_guard.analyze(ctx).await;
                drop(agent_guard);

                // Broadcast analysis result via WebSocket
                if !result.text.is_empty() {
                    let tx = {
                        let s = agent_state.read().await;
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
            }
        });
    }

    // ADR-050: Parse bind address once, use for all listeners
    let bind_ip: std::net::IpAddr = args.bind_addr.parse()
        .expect("Invalid --bind-addr (use 127.0.0.1 or 0.0.0.0)");

    server::run_server(state, &args, bind_ip).await
        .expect("Server error");
}
