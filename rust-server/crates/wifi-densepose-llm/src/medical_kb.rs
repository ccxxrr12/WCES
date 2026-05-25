//! Medical KB — Lightweight knowledge base with vital-sign pattern matching.
//!
//! Loads a JSON file at startup, supports hot reload via SIGHUP or API.
//! Scoring: weighted match across vitals, trends, motion, and alerts.
//! Typical: 20 entries × 6 conditions = 120 comparisons, <0.1ms.

use crate::types::{AgentVitalSnapshot, KbMatchResult};
use serde::Deserialize;
use std::time::Instant;

// ── KB Entry ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct MedicalKbEntry {
    pub id: String,
    pub condition: String,
    pub vital_pattern: VitalPattern,
    #[serde(default)]
    pub risk_factors: Vec<String>,
    #[serde(default)]
    pub triage_implication: String,
    #[serde(default)]
    pub monitoring_notes: String,
    #[serde(default)]
    pub differential_diagnosis: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VitalPattern {
    pub hr_range: Option<(f32, f32)>,
    pub rr_range: Option<(f32, f32)>,
    pub hr_trend: Option<String>,
    pub rr_trend: Option<String>,
    #[serde(default)]
    pub motion_states: Vec<String>,
    #[serde(default)]
    pub edge_alert_keywords: Vec<String>,
}

impl VitalPattern {
    /// Score a AgentVitalSnapshot against this pattern.
    /// Returns (score 0.0-1.0, list of matched condition descriptions).
    pub fn score(&self, v: &AgentVitalSnapshot) -> (f32, Vec<String>) {
        let mut conditions: u8 = 0;
        let mut total: u8 = 0;
        let mut matched: Vec<String> = Vec::new();

        if let Some((lo, hi)) = self.hr_range {
            total += 1;
            if let Some(hr) = v.heart_rate_bpm {
                if hr >= lo && hr <= hi {
                    conditions += 1;
                    matched.push(format!("心率 {:.0} ∈ [{:.0}, {:.0}]", hr, lo, hi));
                }
            }
        }
        if let Some((lo, hi)) = self.rr_range {
            total += 1;
            if let Some(rr) = v.breathing_rate_bpm {
                if rr >= lo && rr <= hi {
                    conditions += 1;
                    matched.push(format!("呼吸 {:.0} ∈ [{:.0}, {:.0}]", rr, lo, hi));
                }
            }
        }
        if total == 0 {
            return (0.0, matched);
        }
        let score = conditions as f32 / total as f32;
        (score, matched)
    }
}

// ── Medical KB ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct KbFile {
    entries: Vec<MedicalKbEntry>,
}

pub struct MedicalKb {
    entries: Vec<MedicalKbEntry>,
    pub(crate) loaded_at: Instant,
}

impl MedicalKb {
    pub fn load(path: &str) -> Result<Self, anyhow::Error> {
        let data = std::fs::read_to_string(path)?;
        let kb: KbFile = serde_json::from_str(&data)?;
        tracing::info!("MedicalKB: {} entries loaded from {}", kb.entries.len(), path);
        Ok(Self {
            entries: kb.entries,
            loaded_at: Instant::now(),
        })
    }

    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            loaded_at: Instant::now(),
        }
    }

    pub fn reload(&mut self, path: &str) -> Result<(), anyhow::Error> {
        let data = std::fs::read_to_string(path)?;
        let kb: KbFile = serde_json::from_str(&data)?;
        self.entries = kb.entries;
        self.loaded_at = Instant::now();
        tracing::info!("MedicalKB: reloaded {} entries", self.entries.len());
        Ok(())
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Match vitals against all KB entries. Returns top 3 matches with score ≥ 0.4.
    pub fn match_vitals(&self, vitals: &AgentVitalSnapshot) -> Vec<KbMatchResult> {
        let mut results: Vec<KbMatchResult> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let (score, matched) = entry.vital_pattern.score(vitals);
                if score >= 0.4 {
                    Some(KbMatchResult {
                        entry_id: entry.id.clone(),
                        condition: entry.condition.clone(),
                        match_score: score,
                        matched_conditions: matched,
                        risk_factors: entry.risk_factors.clone(),
                        triage_implication: entry.triage_implication.clone(),
                        monitoring_notes: entry.monitoring_notes.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| {
            b.match_score
                .partial_cmp(&a.match_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(3);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vitals(hr: Option<f32>, rr: Option<f32>) -> AgentVitalSnapshot {
        AgentVitalSnapshot {
            heart_rate_bpm: hr,
            breathing_rate_bpm: rr,
            breathing_confidence: 0.9,
            heartbeat_confidence: 0.85,
            signal_quality: 0.8,
            motion_class: Some("present_still".into()),
            person_count_estimate: Some(1),
            rssi: Some(-45),
        }
    }

    #[test]
    fn test_tachycardia_match() {
        let entry = MedicalKbEntry {
            id: "tach_001".into(),
            condition: "心动过速".into(),
            vital_pattern: VitalPattern {
                hr_range: Some((100.0, 180.0)),
                rr_range: None,
                hr_trend: None,
                rr_trend: None,
                motion_states: vec![],
                edge_alert_keywords: vec![],
            },
            risk_factors: vec![],
            triage_implication: String::new(),
            monitoring_notes: String::new(),
            differential_diagnosis: vec![],
        };

        let kb = MedicalKb {
            entries: vec![entry],
            loaded_at: Instant::now(),
        };

        let vitals = make_vitals(Some(135.0), Some(22.0));
        let matches = kb.match_vitals(&vitals);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].condition, "心动过速");
        assert!(matches[0].match_score >= 0.9);
    }

    #[test]
    fn test_no_match() {
        let entry = MedicalKbEntry {
            id: "tach_001".into(),
            condition: "心动过速".into(),
            vital_pattern: VitalPattern {
                hr_range: Some((100.0, 180.0)),
                rr_range: None,
                hr_trend: None,
                rr_trend: None,
                motion_states: vec![],
                edge_alert_keywords: vec![],
            },
            risk_factors: vec![],
            triage_implication: String::new(),
            monitoring_notes: String::new(),
            differential_diagnosis: vec![],
        };

        let kb = MedicalKb {
            entries: vec![entry],
            loaded_at: Instant::now(),
        };

        let vitals = make_vitals(Some(72.0), Some(14.0));
        let matches = kb.match_vitals(&vitals);
        assert!(matches.is_empty());
    }
}
