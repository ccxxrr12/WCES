//! Shared data types for the sensing server.
//!
//! Extracted from `main.rs` to keep the entry point manageable.

use serde::{Deserialize, Serialize};

use crate::edge_module_engine::EdgeAlert;
use crate::mat_pipeline::TriageUpdate;
use crate::vital_signs::VitalSigns;

// ── Constants ──────────────────────────────────────────────────────────────────

/// If no ESP32 frame arrives within this duration, source reverts to offline.
pub const ESP32_OFFLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Number of frames retained in `frame_history` for temporal analysis.
pub const FRAME_HISTORY_CAPACITY: usize = 100;

// Signal processing constants
pub const DEBOUNCE_FRAMES: u32 = 4;
pub const MOTION_EMA_ALPHA: f64 = 0.15;
pub const BASELINE_EMA_ALPHA: f64 = 0.003;
pub const BASELINE_WARMUP: u64 = 50;

// Vital smoothing constants
pub const VITAL_MEDIAN_WINDOW: usize = 21;
pub const VITAL_EMA_ALPHA: f64 = 0.02;
pub const HR_MAX_JUMP: f64 = 8.0;
pub const BR_MAX_JUMP: f64 = 2.0;
pub const HR_DEAD_BAND: f64 = 2.0;
pub const BR_DEAD_BAND: f64 = 0.5;

// ── CSI Frame Types ────────────────────────────────────────────────────────────

/// ADR-018 ESP32 CSI binary frame header (20 bytes)
#[derive(Debug, Clone)]
pub struct Esp32Frame {
    pub magic: u32,
    pub node_id: u8,
    pub n_antennas: u8,
    pub n_subcarriers: u16,
    pub freq_mhz: u32,
    pub sequence: u32,
    pub rssi: i8,
    pub noise_floor: i8,
    pub amplitudes: Vec<f64>,
    pub phases: Vec<f64>,
}

// ── WebSocket Sensing Update ───────────────────────────────────────────────────

/// Sensing update broadcast to WebSocket clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensingUpdate {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: f64,
    pub source: String,
    pub tick: u64,
    pub nodes: Vec<NodeInfo>,
    pub features: FeatureInfo,
    pub classification: ClassificationInfo,
    pub signal_field: SignalField,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vital_signs: Option<VitalSigns>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triage_update: Option<TriageUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_alerts: Option<Vec<EdgeAlert>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose_keypoints: Option<Vec<[f64; 4]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_status: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persons: Option<Vec<PersonDetection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_persons: Option<usize>,
    /// Kalman-smoothed survivor tracking data from TrackingBridge (dead data flow fix #3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked_survivors: Option<Vec<TrackedSurvivor>>,
    /// Pending alerts from AlertingBridge (dead data flow fix #4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: u8,
    pub rssi_dbm: f64,
    pub position: [f64; 3],
    pub amplitude: Vec<f64>,
    pub subcarrier_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breathing_rate_bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heart_rate_bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_level: Option<String>,
    pub presence: bool,
    pub active: bool,
    pub channel: u8,
    pub band: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureInfo {
    pub mean_rssi: f64,
    pub variance: f64,
    pub motion_band_power: f64,
    pub breathing_band_power: f64,
    pub dominant_freq_hz: f64,
    pub change_points: usize,
    pub spectral_power: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationInfo {
    pub motion_level: String,
    pub presence: bool,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalField {
    pub grid_size: [usize; 3],
    pub values: Vec<f64>,
}

// ── Tracking & Alerting ────────────────────────────────────────────────────────

/// Survivor tracking data from Kalman filter (for UI map rendering).
/// Populated from TrackingBridge::active_track_snapshots().
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedSurvivor {
    pub survivor_id: String,
    /// Kalman-smoothed 3D position [x, y, z].
    pub position: Option<[f64; 3]>,
    /// Velocity vector [vx, vy, vz] in m/s.
    pub velocity: Option<[f64; 3]>,
    /// Whether this survivor was re-identified in the last update.
    pub reidentified: bool,
    /// Tracking confidence [0-1].
    pub tracking_confidence: f64,
}

// ── Pose / Person Detection ────────────────────────────────────────────────────

/// WiFi-derived pose keypoint (17 COCO keypoints)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoseKeypoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub confidence: f64,
}

/// Person detection from WiFi sensing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonDetection {
    pub id: u32,
    pub confidence: f64,
    pub keypoints: Vec<PoseKeypoint>,
    pub bbox: BoundingBox,
    pub zone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// ── ESP32 Edge Packet Types ────────────────────────────────────────────────────

/// Decoded vitals packet from ESP32 edge processing pipeline (ADR-039, magic 0xC511_0002).
#[derive(Debug, Clone, Serialize)]
pub struct Esp32VitalsPacket {
    pub node_id: u8,
    pub presence: bool,
    pub fall_detected: bool,
    pub motion: bool,
    pub breathing_rate_bpm: f64,
    pub heartrate_bpm: f64,
    pub rssi: i8,
    pub n_persons: u8,
    pub motion_energy: f32,
    pub presence_score: f32,
    pub timestamp_ms: u32,
}

/// Single WASM event (type + value) — ADR-040.
#[derive(Debug, Clone, Serialize)]
pub struct WasmEvent {
    pub event_type: u8,
    pub value: f32,
}

/// Decoded WASM output packet from ESP32 Tier 3 runtime (ADR-040, magic 0xC511_0005).
#[derive(Debug, Clone, Serialize)]
pub struct WasmOutputPacket {
    pub node_id: u8,
    pub module_id: u8,
    pub events: Vec<WasmEvent>,
}

// ── Per-node state (multi-node support) ──────────────────────────────────────────

use std::collections::VecDeque;

/// Independent state tracked for each ESP32-C5 sensing node.
/// When multiple nodes stream CSI data, each gets its own signal processing pipeline.
/// Triage decisions are made by the shared `AppStateInner.triage_engine`.
pub(crate) struct PerNodeState {
    pub frame_history: VecDeque<Vec<f64>>,
    pub rssi_history: VecDeque<f64>,
    pub latest_vitals: VitalSigns,
    pub tick: u64,
    pub last_frame_time: Option<std::time::Instant>,

    // Motion smoothing
    pub smoothed_motion: f64,
    pub current_motion_level: String,
    pub debounce_counter: u32,
    pub debounce_candidate: String,
    pub baseline_motion: f64,
    pub baseline_frames: u64,

    // Vital smoothing
    pub smoothed_hr: f64,
    pub smoothed_br: f64,
    pub smoothed_hr_conf: f64,
    pub smoothed_br_conf: f64,
    pub hr_buffer: VecDeque<f64>,
    pub br_buffer: VecDeque<f64>,

    // Person count
    pub smoothed_person_score: f64,
    pub prev_person_count: usize,

    // Dynamic sample rate (measured from frame arrival intervals)
    pub measured_sample_rate: f64,

    // Edge/wasm
    pub edge_vitals: Option<crate::types::Esp32VitalsPacket>,
    pub latest_wasm_events: Option<crate::types::WasmOutputPacket>,

    // Signal processing pipeline (phase sanitize + normalize + hampel + motion + coherence)
    pub signal_pipeline: wifi_densepose_sensing_server::signal_pipeline::SignalPipeline,
}

impl PerNodeState {
    pub fn new(vital_sample_rate: f64) -> Self {
        Self {
            frame_history: VecDeque::with_capacity(FRAME_HISTORY_CAPACITY),
            rssi_history: VecDeque::with_capacity(60),
            latest_vitals: VitalSigns::default(),
            tick: 0,
            last_frame_time: None,
            smoothed_motion: 0.0,
            current_motion_level: "absent".into(),
            debounce_counter: 0,
            debounce_candidate: "absent".into(),
            baseline_motion: 0.0,
            baseline_frames: 0,
            smoothed_hr: 0.0,
            smoothed_br: 0.0,
            smoothed_hr_conf: 0.0,
            smoothed_br_conf: 0.0,
            hr_buffer: VecDeque::with_capacity(32),
            br_buffer: VecDeque::with_capacity(32),
            smoothed_person_score: 0.0,
            prev_person_count: 0,
            measured_sample_rate: vital_sample_rate,
            edge_vitals: None,
            latest_wasm_events: None,
            signal_pipeline: wifi_densepose_sensing_server::signal_pipeline::SignalPipeline::new(),
        }
    }
}
