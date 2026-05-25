//! Medical Analysis Engine — Coordinator
//!
//! Coordinates all components (PatientRecordDB, MedicalKnowledgeBase,
//! SlidingWindow, PromptBuilder, FallbackAnalyzer) into a unified
//! analysis pipeline. LLM inference is offloaded to cloud API via the Agent.

use crate::config::LlmConfig;
use crate::fallback::{FallbackAnalyzer, FallbackContext, LlmAnalysisResult};
use crate::medical_knowledge::{MatchInput, MedicalKnowledgeBase};
use crate::patient_record::{PatientRecord, PatientRecordDB};
use crate::prompt_builder::{PromptBuilder, PromptContext};
use crate::sliding_window::{VitalSnapshot, WindowManager};

use crate::types::StreamToken;

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, Mutex};

/// Inner state shared across tasks.
struct EngineInner {
    config: LlmConfig,
    patient_db: PatientRecordDB,
    knowledge_base: MedicalKnowledgeBase,
    windows: WindowManager,
    /// Last analysis time per patient (for cooldown)
    last_analysis: HashMap<String, Instant>,
    /// Node ID → patient ID mapping
    node_patient_map: HashMap<u8, String>,
}

/// The main LLM analysis engine.
pub struct LlmAnalysisEngine {
    inner: Arc<Mutex<EngineInner>>,
}

impl LlmAnalysisEngine {
    /// Create a new engine with the given configuration.
    pub async fn new(config: LlmConfig) -> Result<Self> {
        let patient_db = PatientRecordDB::open(&config.patient_db_path)?;
        let knowledge_base = MedicalKnowledgeBase::load(&config.medical_kb_path)?;

        let windows = WindowManager::new(
            config.short_window_secs,
            config.medium_window_secs,
            config.long_window_secs,
        );

        tracing::info!(
            "Medical Analysis Engine: {} patients, {} knowledge entries",
            patient_db.count(),
            knowledge_base.condition_count()
        );

        Ok(Self {
            inner: Arc::new(Mutex::new(EngineInner {
                config,
                patient_db,
                knowledge_base,
                windows,
                last_analysis: HashMap::new(),
                node_patient_map: HashMap::new(),
            })),
        })
    }

    /// Create a new engine with default config and custom data paths.
    pub async fn new_with_paths(
        patient_db_path: impl AsRef<Path>,
        medical_kb_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let mut config = LlmConfig::default();
        config.patient_db_path = patient_db_path.as_ref().to_string_lossy().into();
        config.medical_kb_path = medical_kb_path.as_ref().to_string_lossy().into();
        Self::new(config).await
    }

    // ── Patient Management ───────────────────────────────────────────────

    /// Register a new patient (or update existing).
    pub async fn register_patient(&self, record: PatientRecord) -> Result<()> {
        let mut inner = self.inner.lock().await;

        if let Some(node_id) = record.node_id {
            inner
                .node_patient_map
                .insert(node_id, record.patient_id.clone());
        }

        inner.patient_db.put(&record)?;
        Ok(())
    }

    /// Get a patient record by ID.
    pub async fn get_patient(&self, patient_id: &str) -> Result<Option<PatientRecord>> {
        let inner = self.inner.lock().await;
        inner.patient_db.get(patient_id)
    }

    /// List all registered patients.
    pub async fn list_patients(&self) -> Result<Vec<PatientRecord>> {
        let inner = self.inner.lock().await;
        inner.patient_db.list_all()
    }

    /// Find patient by node ID.
    pub async fn get_patient_by_node(&self, node_id: u8) -> Result<Option<PatientRecord>> {
        let inner = self.inner.lock().await;
        inner.patient_db.get_by_node_id(node_id)
    }

    // ── Vital Data Ingestion ─────────────────────────────────────────────

    /// Push a new vital signs snapshot into sliding windows.
    pub async fn push_vitals(
        &self,
        node_id: u8,
        breathing_rate: f64,
        heart_rate: f64,
        motion_score: f64,
        signal_quality: f64,
    ) {
        let mut inner = self.inner.lock().await;

        let patient_id = inner
            .node_patient_map
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| format!("AUTO-N{}", node_id));

        inner.windows.push(
            &patient_id,
            VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate,
                heart_rate,
                motion_score,
                signal_quality,
            },
        );
    }

    // ── Analysis Trigger ────────────────────────────────────────────────

    /// Trigger a synchronous analysis (uses fallback, always returns immediately).
    pub async fn trigger_analysis(
        &self,
        patient_id: &str,
        current_rr: Option<f64>,
        current_hr: Option<f64>,
        current_motion: f64,
        current_signal_quality: f64,
        current_triage: &str,
        active_edge_alerts: &[String],
    ) -> Option<LlmAnalysisResult> {
        // Use shared context builder logic
        let (ctx, _prompt, now) =
            self.build_analysis_context(patient_id, current_rr, current_hr,
                current_motion, current_signal_quality, current_triage, active_edge_alerts).await?;

        let mut result = FallbackAnalyzer::analyze(&ctx);
        result.analysis_time_ms = now.elapsed().as_millis() as u64;

        // Update cooldown
        {
            let mut inner = self.inner.lock().await;
            inner
                .last_analysis
                .insert(patient_id.to_string(), Instant::now());
        }

        Some(result)
    }

    /// Trigger a streaming analysis.
    ///
    /// When the `llm` feature is enabled and the model is loaded, this spawns
    /// a blocking token generation task and returns a broadcast receiver.
    /// The caller should consume the receiver and render tokens in the UI.
    ///
    /// When the model isn't available, returns the fallback result immediately
    /// and pushes it as a single "complete" token through the receiver.
    pub async fn trigger_analysis_streaming(
        &self,
        patient_id: &str,
        current_rr: Option<f64>,
        current_hr: Option<f64>,
        current_motion: f64,
        current_signal_quality: f64,
        current_triage: &str,
        active_edge_alerts: &[String],
    ) -> Option<broadcast::Receiver<StreamToken>> {
        let (ctx, _prompt, _now) =
            self.build_analysis_context(patient_id, current_rr, current_hr,
                current_motion, current_signal_quality, current_triage, active_edge_alerts).await?;

        // Update cooldown immediately before spawning
        {
            let mut inner = self.inner.lock().await;
            inner
                .last_analysis
                .insert(patient_id.to_string(), Instant::now());
        }

        // Fallback: generate template analysis and send as synthetic stream
        let result = FallbackAnalyzer::analyze(&ctx);
        let json = serde_json::to_string(&result).unwrap_or_default();

        let (tx, rx) = broadcast::channel(4);
        // Split the fallback JSON into "chunks" to simulate streaming
        let chars: Vec<char> = json.chars().collect();
        let chunk_size = 16usize;

        let patient_id = patient_id.to_string();
        tokio::spawn(async move {
            let total = chars.len();
            for (i, chunk) in chars.chunks(chunk_size).enumerate() {
                let text: String = chunk.iter().collect();
                let _ = tx.send(StreamToken {
                    survivor_id: patient_id.clone(),
                    token_index: i as u32,
                    text,
                    is_complete: i * chunk_size >= total - chunk_size,
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
            let _ = tx.send(StreamToken {
                survivor_id: patient_id,
                token_index: u32::MAX,
                text: String::new(),
                is_complete: true,
            });
        });

        Some(rx)
    }

    // ── Internal Helpers ────────────────────────────────────────────────

    /// Build the shared analysis context (used by both sync and streaming paths).
    async fn build_analysis_context(
        &self,
        patient_id: &str,
        current_rr: Option<f64>,
        current_hr: Option<f64>,
        current_motion: f64,
        current_signal_quality: f64,
        current_triage: &str,
        active_edge_alerts: &[String],
    ) -> Option<(FallbackContext, Option<String>, Instant)> {
        let inner = self.inner.lock().await;
        let now = Instant::now();

        // Cooldown check
        if let Some(last) = inner.last_analysis.get(patient_id) {
            let elapsed = now.duration_since(*last);
            if elapsed.as_secs() < inner.config.per_patient_cooldown_secs {
                return None;
            }
        }

        // Get or create patient record
        let patient = match inner.patient_db.get(patient_id) {
            Ok(Some(p)) => p,
            Ok(None) => {
                let record = PatientRecord::new(patient_id);
                if let Err(e) = inner.patient_db.put(&record) {
                    tracing::warn!("Failed to auto-register patient {}: {}", patient_id, e);
                }
                record
            }
            Err(e) => {
                tracing::error!("Failed to get patient record: {}", e);
                return None;
            }
        };

        // Get window data
        let window = inner.windows.get(patient_id)?;
        if !window.has_enough_data() {
            return None;
        }

        let trend_summary = window.medium_summary();

        // Match medical conditions
        let match_input = MatchInput {
            breathing_rate: current_rr,
            heart_rate: current_hr,
            motion_score: current_motion,
            breathing_trend: trend_summary.rr_trend.as_str(),
            heart_trend: trend_summary.hr_trend.as_str(),
            motion_pattern: trend_summary.motion_pattern.as_str(),
            pre_existing: &patient.pre_existing,
            age: patient.age,
            active_edge_alerts,
        };

        let matched_conditions = inner.knowledge_base.match_conditions(&match_input, 3);

        // Build fallback context
        let ctx = FallbackContext {
            patient: patient.clone(),
            current_rr,
            current_hr,
            current_motion,
            current_signal_quality,
            current_triage: current_triage.to_string(),
            trend_summary: trend_summary.clone(),
            matched_conditions: matched_conditions.clone(),
            active_edge_alerts: active_edge_alerts.to_vec(),
        };

        // Build LLM prompt
        let prompt = {
            let prompt_ctx = PromptContext {
                patient,
                current_rr,
                current_hr,
                current_motion,
                current_signal_quality,
                current_triage: current_triage.to_string(),
                trend_summary,
                matched_conditions,
                active_edge_alerts: active_edge_alerts.to_vec(),
            };
            let built = PromptBuilder::build(&prompt_ctx);
            Some(built.prompt)
        };

        drop(inner);
        Some((ctx, prompt, Instant::now()))
    }

    /// Build an LLM prompt for a patient (used when full LLM is enabled).
    pub async fn build_prompt(
        &self,
        patient_id: &str,
        current_rr: Option<f64>,
        current_hr: Option<f64>,
        current_motion: f64,
        current_signal_quality: f64,
        current_triage: &str,
        active_edge_alerts: &[String],
    ) -> Option<String> {
        let (_, prompt, _) = self
            .build_analysis_context(
                patient_id,
                current_rr,
                current_hr,
                current_motion,
                current_signal_quality,
                current_triage,
                active_edge_alerts,
            )
            .await?;
        prompt
    }

    // ── Health / Status ──────────────────────────────────────────────────

    /// Get engine status information.
    pub async fn status(&self) -> EngineStatus {
        let inner = self.inner.lock().await;

        let llm_loaded = false; // LLM now via cloud API, not local Candle

        EngineStatus {
            patients_registered: inner.patient_db.count(),
            knowledge_entries: inner.knowledge_base.condition_count(),
            tracked_patients: inner.windows.patient_count(),
            periodic_interval_secs: inner.config.periodic_interval_secs,
            analysis_cooldown_secs: inner.config.per_patient_cooldown_secs,
            llm_loaded,
        }
    }
}

/// Engine status snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineStatus {
    pub patients_registered: usize,
    pub knowledge_entries: usize,
    pub tracked_patients: usize,
    pub periodic_interval_secs: u64,
    pub analysis_cooldown_secs: u64,
    pub llm_loaded: bool,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_initialization() {
        let engine = LlmAnalysisEngine::new_with_paths(
            "data/test_engine_patients",
            "data/medical_knowledge.json",
        )
        .await
        .expect("Engine should initialize");

        let status = engine.status().await;
        assert!(status.patients_registered <= 1);
        assert!(status.knowledge_entries >= 5);
        assert_eq!(status.tracked_patients, 0);
    }

    #[tokio::test]
    async fn test_register_and_analyze() {
        let engine = LlmAnalysisEngine::new_with_paths(
            "data/test_engine2_patients",
            "data/medical_knowledge.json",
        )
        .await
        .expect("Engine should initialize");

        let mut patient = PatientRecord::new("PAT-TEST-ENGINE");
        patient.age = Some(68);
        patient.pre_existing = vec!["COPD".into()];
        patient.node_id = Some(2);
        engine.register_patient(patient).await.unwrap();

        for i in 0..10 {
            engine
                .push_vitals(
                    2,
                    28.0 + i as f64 * 0.5,
                    100.0 + i as f64 * 2.0,
                    0.3,
                    0.85,
                )
                .await;
        }

        let result = engine
            .trigger_analysis(
                "PAT-TEST-ENGINE",
                Some(32.0),
                Some(118.0),
                0.4,
                0.8,
                "Immediate",
                &["med_respiratory_distress".into()],
            )
            .await;

        assert!(result.is_some());
        let analysis = result.unwrap();
        assert_eq!(analysis.patient_id, "PAT-TEST-ENGINE");
        assert!(analysis.risk_assessment.deteriorating);
        assert!(!analysis.recommendations.is_empty());

        drop(engine);
        let _ = std::fs::remove_dir_all("data/test_engine_patients");
        let _ = std::fs::remove_dir_all("data/test_engine2_patients");
    }

    #[tokio::test]
    async fn test_streaming_analysis_fallback() {
        let engine = LlmAnalysisEngine::new_with_paths(
            "data/test_engine_streaming",
            "data/medical_knowledge.json",
        )
        .await
        .expect("Engine should initialize");

        let mut patient = PatientRecord::new("PAT-STREAM");
        patient.node_id = Some(1);
        engine.register_patient(patient).await.unwrap();

        // Feed vitals
        for i in 0..10 {
            engine
                .push_vitals(1, 32.0 + i as f64 * 0.5, 115.0 + i as f64 * 2.0, 0.4, 0.8)
                .await;
        }

        // Trigger streaming analysis
        let rx = engine
            .trigger_analysis_streaming(
                "PAT-STREAM",
                Some(36.0),
                Some(130.0),
                0.4,
                0.8,
                "Immediate",
                &[],
            )
            .await;

        assert!(rx.is_some());

        // Collect tokens
        let mut rx = rx.unwrap();
        let mut received_tokens = 0usize;
        let mut saw_complete = false;

        for _ in 0..100 {
            match rx.recv().await {
                Ok(token) => {
                    received_tokens += 1;
                    if token.is_complete {
                        saw_complete = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        assert!(received_tokens > 0, "Should receive tokens");
        assert!(saw_complete, "Should see completion signal");

        drop(engine);
        let _ = std::fs::remove_dir_all("data/test_engine_streaming");
    }
}
