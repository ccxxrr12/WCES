//! MAT Pipeline — START 分诊 + 伤员追踪 + 告警 (竞赛核心模块)
//!
//! 本模块不重复实现信号处理 — 服务器已有完整的 FFT 呼吸/心率检测。
//! 本模块专注于将 VitalSigns 转换为 START 分诊、伤员追踪和告警。
//!
//! # 使用方式
//! ```rust,ignore
//! use mat_pipeline::{TriageEngine, VitalSignsInput};
//!
//! let mut engine = TriageEngine::new(TriageConfig::competition());
//! let input = VitalSignsInput {
//!     breathing_rate_bpm: Some(15.0),
//!     heart_rate_bpm: Some(72.0),
//!     motion_score: 0.3,
//!     ..Default::default()
//! };
//! let update = engine.process(&input);
//! // 序列化为 JSON 推送到 /ws/triage
//! ```
//!
//! # START 协议参考
//! - Immediate (红): RR>30 或 RR<10, 或 HR>120 或 HR<40
//! - Delayed (黄): RR 10-12 或 25-30, 或 HR 40-50 或 100-120, 或高运动
//! - Minor (绿): 生命体征正常 + 可自主移动
//! - Deceased (黑): 无生命体征

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── 输入类型 (来自服务器的 VitalSigns) ──────────────────────────────────────

/// 从服务器 VitalSignDetector 的输出转换而来的输入
#[derive(Debug, Clone, Default)]
pub struct VitalSignsInput {
    pub breathing_rate_bpm: Option<f64>,
    pub breathing_confidence: f64,
    pub heart_rate_bpm: Option<f64>,
    pub heartbeat_confidence: f64,
    pub signal_quality: f64,
    /// 0-1 运动强度 (来自服务器 motion_level)
    pub motion_score: f64,
    /// 检测到的人员 ID
    pub person_id: Option<u32>,
    /// 来源节点 ID
    pub node_id: u8,
    /// RSSI (dBm)
    pub rssi: f64,
}

// ── 输出类型 (推送到 triage UI) ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageUpdate {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub survivors: Vec<SurvivorSnapshot>,
    pub assessment: MassCasualtySnapshot,
    pub alerts: Vec<AlertSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurvivorSnapshot {
    pub id: String,
    pub triage: String,
    pub triage_color: String,
    pub triage_priority: u8,
    pub breathing_rate: Option<f64>,
    pub heart_rate: Option<f64>,
    pub motion_score: f64,
    pub position: Option<[f64; 3]>,
    pub position_confidence: f64,
    pub is_deteriorating: bool,
    pub tracked_seconds: f64,
    pub node_id: u8,
    pub estimated_age: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MassCasualtySnapshot {
    pub total: u32,
    pub immediate: u32,
    pub delayed: u32,
    pub minor: u32,
    pub deceased: u32,
    pub unknown: u32,
    pub severity: String,
    pub rescuer_estimate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertSnapshot {
    pub time: String,
    pub survivor_id: String,
    pub alert_type: String,
    pub message: String,
    pub priority: u8,
}

// ── 分诊配置 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TriageConfig {
    pub node_positions: HashMap<u8, (f64, f64, f64)>,
    pub survivor_timeout_secs: f64,
    pub deterioration_window: u32,
    pub min_signal_quality: f64,
}

impl Default for TriageConfig {
    fn default() -> Self {
        Self {
            node_positions: HashMap::new(),
            survivor_timeout_secs: 30.0,
            deterioration_window: 5,
            min_signal_quality: 0.1,
        }
    }
}

impl TriageConfig {
    pub fn competition() -> Self {
        let mut pos = HashMap::new();
        pos.insert(1, (0.0, 1.15, 1.0));
        pos.insert(2, (-1.0, -0.58, 1.0));
        pos.insert(3, (1.0, -0.58, 1.0));
        Self { node_positions: pos, ..Default::default() }
    }
}

// ── START 分诊规则 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TriageLevel {
    Deceased = 4,   // 黑色 — 无生命体征
    Unknown = 5,    // 灰色 — 数据不足
    Minor = 3,      // 绿色 — 轻伤
    Delayed = 2,    // 黄色 — 延迟救治
    Immediate = 1,  // 红色 — 立即救治
}

impl TriageLevel {
    pub fn name(&self) -> &'static str {
        match self {
            TriageLevel::Immediate => "Immediate",
            TriageLevel::Delayed => "Delayed",
            TriageLevel::Minor => "Minor",
            TriageLevel::Deceased => "Deceased",
            TriageLevel::Unknown => "Unknown",
        }
    }
    pub fn color(&self) -> &'static str {
        match self {
            TriageLevel::Immediate => "red",
            TriageLevel::Delayed => "yellow",
            TriageLevel::Minor => "green",
            TriageLevel::Deceased => "black",
            TriageLevel::Unknown => "gray",
        }
    }
    pub fn priority(&self) -> u8 { *self as u8 }
}

/// START 分诊计算 (遵循标准 START 协议)
pub fn calculate_triage(input: &VitalSignsInput) -> TriageLevel {
    // 信号质量不足 → Unknown
    if input.signal_quality < 0.05 {
        return TriageLevel::Unknown;
    }

    let br = input.breathing_rate_bpm;
    let hr = input.heart_rate_bpm;

    // 无有效生命体征 → Deceased
    if br.is_none() && hr.is_none() {
        return TriageLevel::Deceased;
    }

    let br = br.unwrap_or(0.0);
    let hr = hr.unwrap_or(0.0);

    // Immediate (红): START 协议 — 呼吸 >30/min 或 <10/min
    if br > 30.0 || (br > 0.0 && br < 10.0) {
        return TriageLevel::Immediate;
    }

    // Immediate (红): 心率 >120 或 <40 BPM
    if hr > 120.0 || (hr > 0.0 && hr < 40.0) {
        return TriageLevel::Immediate;
    }

    // Minor (绿): 能自主移动 (motion_score 高) + 生命体征正常
    if br >= 12.0 && br <= 24.0 && hr >= 50.0 && hr <= 100.0 && input.motion_score > 0.3 {
        return TriageLevel::Minor;
    }

    // Minor (绿): 生命体征正常 (即使不动)
    if br >= 12.0 && br <= 20.0 && hr >= 60.0 && hr <= 100.0 {
        return TriageLevel::Minor;
    }

    // Delayed (黄): 其余情况 — 稳定但需观察
    TriageLevel::Delayed
}

// ── 简易距离估计 (RSSI → 米) ──────────────────────────────────────────────

fn rssi_to_distance(rssi: f64) -> f64 {
    let ref_rssi = -30.0;  // 1米参考 RSSI
    let n = 3.0;           // 室内穿墙路径损耗指数
    10.0_f64.powf((ref_rssi - rssi.max(-90.0)) / (10.0 * n))
}

// ── 伤员追踪引擎 ────────────────────────────────────────────────────────────

struct TrackedSurvivor {
    id: String,
    triage: TriageLevel,
    prev_triage: TriageLevel,
    position: (f64, f64, f64),
    position_confidence: f64,
    breathing_history: Vec<f64>,
    heart_rate_history: Vec<f64>,
    motion_history: Vec<f64>,
    first_seen: f64,
    last_updated: f64,
    node_id: u8,
    deterioration_count: u32,
    status: &'static str,  // "active" | "rescued" | "lost" | "deceased"
}

impl TrackedSurvivor {
    fn new(id: String, now: f64, node_id: u8) -> Self {
        Self {
            id, triage: TriageLevel::Unknown, prev_triage: TriageLevel::Unknown,
            position: (0.0, 0.0, 0.0), position_confidence: 0.0,
            breathing_history: Vec::new(), heart_rate_history: Vec::new(),
            motion_history: Vec::new(),
            first_seen: now, last_updated: now, node_id,
            deterioration_count: 0, status: "active",
        }
    }
}

pub struct TriageEngine {
    config: TriageConfig,
    survivors: HashMap<String, TrackedSurvivor>,
    alerts: Vec<AlertSnapshot>,
    counter: u32,
    start_time: f64,
    /// Per-survivor recent RSSI readings from each node, for multi-node triangulation
    node_observations: HashMap<String, HashMap<u8, (f64, f64)>>,  // survivor_id -> {node_id -> (rssi, timestamp)}
}

impl TriageEngine {
    pub fn new(config: TriageConfig) -> Self {
        Self {
            config, survivors: HashMap::new(), alerts: Vec::new(),
            counter: 0, start_time: now_secs(),
            node_observations: HashMap::new(),
        }
    }

    pub fn process(&mut self, input: &VitalSignsInput) -> TriageUpdate {
        let now = now_secs();

        // 信号质量过滤
        if input.signal_quality < self.config.min_signal_quality {
            return self.build_update();
        }

        // 伤员匹配/创建
        let sid = self.match_or_create(input, now);

        // 更新伤员
        if let Some(s) = self.survivors.get_mut(&sid) {
            s.last_updated = now;
            s.node_id = input.node_id;
            s.prev_triage = s.triage;

            // 平滑生命体征 (最近5个值的均值)
            if let Some(br) = input.breathing_rate_bpm {
                s.breathing_history.push(br);
                if s.breathing_history.len() > 30 { s.breathing_history.remove(0); }
            }
            if let Some(hr) = input.heart_rate_bpm {
                s.heart_rate_history.push(hr);
                if s.heart_rate_history.len() > 30 { s.heart_rate_history.remove(0); }
            }
            s.motion_history.push(input.motion_score);
            if s.motion_history.len() > 30 { s.motion_history.remove(0); }

            // 位置估计: 多节点三角定位 (替代简易RSSI→距离)
            // 记录当前节点的RSSI观测
            let obs = self.node_observations.entry(sid.clone()).or_default();
            obs.insert(input.node_id, (input.rssi, now));
            // 清理超过5秒的旧观测
            obs.retain(|_, (_, t)| now - *t < 5.0);
            
            // 多节点三角定位
            if obs.len() >= 2 {
                // 使用2+个节点的RSSI距离进行加权位置估计
                let mut wx = 0.0f64; let mut wy = 0.0f64; let mut wz = 0.0f64;
                let mut total_w = 0.0f64;
                for (nid, (rssi, _)) in obs.iter() {
                    if let Some((nx, ny, nz)) = self.config.node_positions.get(nid) {
                        let d = rssi_to_distance(*rssi);
                        let w = 1.0 / (d.max(0.3));  // 距离越近权重越高
                        wx += nx * w; wy += ny * w; wz += nz * w;
                        total_w += w;
                    }
                }
                if total_w > 0.0 {
                    s.position = (wx / total_w, wy / total_w, wz / total_w);
                    s.position_confidence = (obs.len() as f64 / 3.0).min(1.0) * input.signal_quality;
                }
            } else if let Some((nx, ny, nz)) = self.config.node_positions.get(&input.node_id) {
                // 单节点: 基于RSSI的距离估计 (保留旧逻辑作为回退)
                let d = rssi_to_distance(input.rssi);
                s.position = (nx + d * 0.5, ny + d * 0.3, nz * 0.5);
                s.position_confidence = input.signal_quality * 0.5;
            }

            // 分诊判定
            let smooth_input = VitalSignsInput {
                breathing_rate_bpm: average_last(&s.breathing_history, 5),
                heart_rate_bpm: average_last(&s.heart_rate_history, 5),
                motion_score: average_last64(&s.motion_history, 5) as f64,
                ..*input
            };
            s.triage = calculate_triage(&smooth_input);

            // 恶化检测 (分诊等级提升 ≥2 级)
            if s.triage.priority() + 2 <= s.prev_triage.priority() {
                s.deterioration_count += 1;
                if s.deterioration_count >= self.config.deterioration_window {
                    s.deterioration_count = 0;
                    self.alerts.push(AlertSnapshot {
                        time: chrono_now(),
                        survivor_id: sid.clone(),
                        alert_type: "DETERIORATION".to_string(),
                        message: format!("{} → {}", s.prev_triage.name(), s.triage.name()),
                        priority: s.triage.priority(),
                    });
                }
            } else {
                s.deterioration_count = 0;
            }
        }

        // 清理过期
        self.survivors.retain(|_, s| (now - s.last_updated) < self.config.survivor_timeout_secs);

        self.build_update()
    }

    fn match_or_create(&mut self, input: &VitalSignsInput, now: f64) -> String {
        // 已有伤员: person_id 匹配
        if let Some(_pid) = input.person_id {
            for (id, s) in &self.survivors {
                if s.node_id == input.node_id && s.last_updated > now - 5.0 {
                    return id.clone();
                }
            }
        }
        // 新建伤员
        self.counter += 1;
        let id = format!("SURV-{:04x}", self.counter);
        self.survivors.insert(id.clone(), TrackedSurvivor::new(id.clone(), now, input.node_id));
        id
    }

    fn build_update(&self) -> TriageUpdate {
        let mut immediate = 0u32; let mut delayed = 0u32; let mut minor = 0u32;
        let mut deceased = 0u32; let mut unknown = 0u32;
        let now = now_secs();

        let survivors: Vec<SurvivorSnapshot> = self.survivors.iter().map(|(id, s)| {
            match s.triage {
                TriageLevel::Immediate => immediate += 1,
                TriageLevel::Delayed => delayed += 1,
                TriageLevel::Minor => minor += 1,
                TriageLevel::Deceased => deceased += 1,
                TriageLevel::Unknown => unknown += 1,
            }
            SurvivorSnapshot {
                id: id.clone(),
                triage: s.triage.name().to_string(),
                triage_color: s.triage.color().to_string(),
                triage_priority: s.triage.priority(),
                breathing_rate: average_last(&s.breathing_history, 3),
                heart_rate: average_last(&s.heart_rate_history, 3),
                motion_score: average_last64(&s.motion_history, 3) as f64,
                position: Some([s.position.0, s.position.1, s.position.2]),
                position_confidence: s.position_confidence,
                is_deteriorating: s.deterioration_count > 0,
                tracked_seconds: now - s.first_seen,
                node_id: s.node_id,
                estimated_age: estimate_age(average_last(&s.breathing_history, 3), average_last(&s.heart_rate_history, 3)),
                status: s.status.to_string(),
            }
        }).collect();

        let total = immediate + delayed + minor + deceased + unknown;
        let rescuer = immediate * 4 + delayed * 2 + minor / 2;
        let severity = if total == 0 { "Minimal" }
            else if immediate >= 3 { "Critical" }
            else if immediate >= 1 { "Major" }
            else if delayed >= 1 { "Moderate" }
            else { "Minimal" };

        TriageUpdate {
            msg_type: "triage_update".to_string(),
            survivors,
            assessment: MassCasualtySnapshot {
                total, immediate, delayed, minor, deceased, unknown,
                severity: severity.to_string(),
                rescuer_estimate: rescuer,
            },
            alerts: self.alerts.clone(),
        }
    }
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

fn average_last(data: &[f64], n: usize) -> Option<f64> {
    if data.is_empty() { return None; }
    let window: Vec<&f64> = data.iter().rev().take(n).collect();
    if window.is_empty() { return None; }
    Some(window.iter().copied().sum::<f64>() / window.len() as f64)
}

fn average_last64(data: &[f64], n: usize) -> f64 {
    average_last(data, n).unwrap_or(0.0)
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

fn chrono_now() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = t.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}", secs/3600, (secs%3600)/60, secs%60)
}

fn estimate_age(br: Option<f64>, hr: Option<f64>) -> String {
    match (br, hr) {
        (Some(b), Some(h)) if b > 25.0 && h > 100.0 => "Infant (<2y)".into(),
        (Some(b), Some(h)) if b > 18.0 && h > 80.0 => "Child (2-12y)".into(),
        (Some(b), Some(h)) if b < 16.0 && h < 65.0 => "Elderly (60y+)".into(),
        _ => "Adult".into(),
    }
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_immediate_high_rr() {
        let v = VitalSignsInput { breathing_rate_bpm: Some(35.0), ..Default::default() };
        assert_eq!(calculate_triage(&v), TriageLevel::Immediate);
    }

    #[test]
    fn test_start_immediate_low_hr() {
        let v = VitalSignsInput { breathing_rate_bpm: Some(15.0), heart_rate_bpm: Some(35.0), ..Default::default() };
        assert_eq!(calculate_triage(&v), TriageLevel::Immediate);
    }

    #[test]
    fn test_start_minor_ambulatory() {
        let v = VitalSignsInput {
            breathing_rate_bpm: Some(15.0), heart_rate_bpm: Some(72.0),
            motion_score: 0.5, signal_quality: 0.8,
            ..Default::default()
        };
        assert_eq!(calculate_triage(&v), TriageLevel::Minor);
    }

    #[test]
    fn test_start_deceased_no_vitals() {
        let v = VitalSignsInput { signal_quality: 0.5, ..Default::default() };
        assert_eq!(calculate_triage(&v), TriageLevel::Deceased);
    }

    #[test]
    fn test_engine_creates_survivor() {
        let mut engine = TriageEngine::new(TriageConfig::competition());
        let input = VitalSignsInput {
            breathing_rate_bpm: Some(15.0), heart_rate_bpm: Some(70.0),
            motion_score: 0.3, signal_quality: 0.8, node_id: 1,
            breathing_confidence: 0.9, heartbeat_confidence: 0.8,
            rssi: -50.0, person_id: None,
        };
        let update = engine.process(&input);
        assert_eq!(update.survivors.len(), 1);
        assert_eq!(update.assessment.total, 1);
    }
}
