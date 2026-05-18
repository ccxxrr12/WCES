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
#[allow(dead_code)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: u8,
    pub rssi_dbm: f64,
    pub position: [f64; 3],
    pub amplitude: Vec<f64>,
    pub subcarrier_count: usize,
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

/// Decoded WASM output packet from ESP32 Tier 3 runtime (ADR-040, magic 0xC511_0004).
#[derive(Debug, Clone, Serialize)]
pub struct WasmOutputPacket {
    pub node_id: u8,
    pub module_id: u8,
    pub events: Vec<WasmEvent>,
}
