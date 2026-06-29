//! Medical Agent — Main orchestrator for the full analysis pipeline.
//!
//! Coordinates: ContextCollator → DegradationManager → AnalysisRouter →
//! PromptCompiler → LlmGateway/fallback → OutputValidator →
//! RiskAdjustmentExtractor → AnalysisResult.

use crate::context::ContextCollator;
use crate::degrade::{DegradationConfig, DegradationManager};
use crate::router::AnalysisRouter;
use crate::template::TemplateEngine;

use crate::types::{
    AnalysisResult, AnalysisSource, DegradationLevel, RouteDecision, StructuredContext,
    TriggerSource,
};

use crate::medical_knowledge::MedicalKnowledgeBase;

// ── Feature-gated imports ────────────────────────────────────────────────────

#[cfg(feature = "agent")]
use crate::gateway::LlmGateway;
#[cfg(feature = "agent")]
use crate::prompt::PromptCompiler;
#[cfg(feature = "agent")]
use crate::risk_adjust::RiskAdjustmentExtractor;
#[cfg(feature = "agent")]
use crate::validator::OutputValidator;

// ── Medical Agent ────────────────────────────────────────────────────────────

pub struct MedicalAgent {
    router: AnalysisRouter,
    degradation: DegradationManager,
    template_engine: TemplateEngine,
    /// Cached medical knowledge base to avoid repeated disk I/O on template fallback.
    cached_medical_kb: Option<MedicalKnowledgeBase>,

    #[cfg(feature = "agent")]
    prompt_compiler: PromptCompiler,
    #[cfg(feature = "agent")]
    gateway: Option<LlmGateway>,
    #[cfg(feature = "agent")]
    validator: OutputValidator,
    #[cfg(feature = "agent")]
    risk_extractor: RiskAdjustmentExtractor,
}

impl MedicalAgent {
    /// Create a medical agent using the cloud LLM path (requires `agent` feature).
    /// Uses default paths relative to CWD (for backwards compatibility).
    #[cfg(feature = "agent")]
    pub fn new(gateway: LlmGateway) -> Self {
        Self::new_with_degradation(gateway, DegradationConfig::default(), "data/prompts", "data/medical_knowledge.json")
    }

    #[cfg(feature = "agent")]
    pub fn new_with_degradation(
        gateway: LlmGateway,
        degradation_config: DegradationConfig,
        prompts_dir: &str,
        medical_kb_path: &str,
    ) -> Self {
        let prompt_compiler = PromptCompiler::from_dir(prompts_dir).unwrap_or_default();

        Self {
            router: AnalysisRouter,
            degradation: DegradationManager::with_config(degradation_config),
            template_engine: TemplateEngine::new(),
            cached_medical_kb: MedicalKnowledgeBase::load(medical_kb_path).ok(),
            prompt_compiler,
            gateway: Some(gateway),
            validator: OutputValidator::new(),
            risk_extractor: RiskAdjustmentExtractor::new(),
        }
    }

    /// Create a medical agent in template-only mode (when `agent` feature is disabled).
    #[cfg(not(feature = "agent"))]
    pub fn new() -> Self {
        Self {
            router: AnalysisRouter,
            degradation: DegradationManager::new(),
            template_engine: TemplateEngine::new(),
            cached_medical_kb: MedicalKnowledgeBase::load("data/medical_knowledge.json").ok(),
        }
    }

    /// Create a template-only agent. Works with any feature flag combination.
    /// Uses a CWD-relative path for the medical KB (backwards compatibility).
    pub fn new_template_only() -> Self {
        Self::new_template_only_with_path("data/medical_knowledge.json")
    }

    /// Create a template-only agent with explicit medical knowledge base path.
    /// Prefer this over `new_template_only()` when a resolved data directory is
    /// available — avoids silent KB load failure when CWD ≠ project root.
    pub fn new_template_only_with_path(medical_kb_path: &str) -> Self {
        #[cfg(feature = "agent")]
        {
            Self {
                router: AnalysisRouter,
                degradation: DegradationManager::new(),
                template_engine: TemplateEngine::new(),
                cached_medical_kb: MedicalKnowledgeBase::load(medical_kb_path).ok(),
                prompt_compiler: PromptCompiler::default(),
                gateway: None,
                validator: OutputValidator::new(),
                risk_extractor: RiskAdjustmentExtractor::new(),
            }
        }
        #[cfg(not(feature = "agent"))]
        {
            Self {
                router: AnalysisRouter,
                degradation: DegradationManager::new(),
                template_engine: TemplateEngine::new(),
                cached_medical_kb: MedicalKnowledgeBase::load(medical_kb_path).ok(),
            }
        }
    }

    /// Returns true if the circuit breaker is currently open (gateway unavailable).
    pub async fn is_breaker_open(&self) -> bool {
        #[cfg(feature = "agent")]
        {
            if let Some(ref gw) = self.gateway {
                return gw.is_breaker_open().await;
            }
        }
        false
    }

    // ── Main Entry Point ────────────────────────────────────────────────

    /// Analyze a patient. This is the main entry point called by sensing-server.
    ///
    /// Returns an AnalysisResult that the caller pushes to the UI via WebSocket.
    pub async fn analyze(
        &mut self,
        ctx: StructuredContext,
    ) -> AnalysisResult {
        let patient_id = ctx.patient_id;

        // Step 0: Sync circuit breaker state into degradation manager
        #[cfg(feature = "agent")]
        if let Some(ref gw) = self.gateway {
            let is_open = gw.is_breaker_open().await;
            self.degradation.on_circuit_breaker_change(is_open);
        }

        // Step 1: Degradation assessment
        let degrade_level = self.degradation.assess(patient_id);

        // Step 2: Handle cached replay (L4)
        if degrade_level == DegradationLevel::L4CachedReplay {
            if let Some(cached) = self.degradation.get_cached(patient_id) {
                return cached.clone();
            }
        }

        // Step 3: Route decision
        let is_deteriorating = matches!(ctx.triggered_by, TriggerSource::Deterioration { .. });

        #[cfg(feature = "agent")]
        let network_ok = self.gateway.is_some();
        #[cfg(not(feature = "agent"))]
        let network_ok = false;

        let in_cooldown = degrade_level == DegradationLevel::L4CachedReplay;

        let route = AnalysisRouter::decide(
            &ctx.triage_current,
            is_deteriorating,
            network_ok,
            in_cooldown,
        );

        // Step 4: Execute analysis based on route
        let result = match route.route {
            crate::types::AnalysisRoute::Skip => {
                AnalysisResult {
                    patient_id,
                    text: String::new(),
                    risk_adjustment: None,
                    source: AnalysisSource::Template,
                    degrade_level: DegradationLevel::L3TemplateOnly,
                    generated_at_ms: now_ms(),
                }
            }
            crate::types::AnalysisRoute::CachedReplay => {
                self.degradation
                    .get_cached(patient_id)
                    .cloned()
                    .unwrap_or_else(|| TemplateEngine::generate_basic(patient_id))
            }

            #[cfg(feature = "agent")]
            crate::types::AnalysisRoute::DeepLLM | crate::types::AnalysisRoute::BriefLLM => {
                if self.gateway.is_some() {
                    self.analyze_via_llm(&ctx, &route, degrade_level).await
                } else {
                    self.template_with_kb(&ctx)
                }
            }

            #[cfg(not(feature = "agent"))]
            crate::types::AnalysisRoute::DeepLLM | crate::types::AnalysisRoute::BriefLLM => {
                // In template-only mode, LLM routes fall back to template
                self.template_with_kb(&ctx)
            }

            crate::types::AnalysisRoute::TemplateWithKB => {
                self.template_with_kb(&ctx)
            }
            crate::types::AnalysisRoute::TemplateOnly => {
                TemplateEngine::generate_basic(patient_id)
            }
        };

        // Step 5: Record in degradation manager
        self.degradation.on_analysis_complete(patient_id, result.clone());

        result
    }

    // ── LLM Analysis Path ───────────────────────────────────────────────

    #[cfg(feature = "agent")]
    async fn analyze_via_llm(
        &mut self,
        ctx: &StructuredContext,
        route: &RouteDecision,
        degrade_level: DegradationLevel,
    ) -> AnalysisResult {
        let patient_id = ctx.patient_id;

        // 1. Compile prompt
        let prompt = self.prompt_compiler.compile(ctx, route);

        // 2. Call LLM via gateway (streaming → collect full text)
        let stream_result = self
            .gateway
            .as_ref()
            .unwrap()
            .stream(&prompt, route.max_output_tokens)
            .await;

        let full_text = match stream_result {
            Ok(mut stream) => {
                let mut text = String::new();
                use tokio_stream::StreamExt;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(s) => text.push_str(&s),
                        Err(e) => {
                            tracing::warn!("Stream error for patient {}: {}", patient_id, e);
                            break;
                        }
                    }
                }
                text
            }
            Err(e) => {
                tracing::warn!(
                    "LLM gateway failed for patient {}: {}, falling back to template",
                    patient_id, e
                );
                // Do NOT call on_network_change(false) here — a single transient
                // stream error should not permanently disable the network path.
                // The circuit breaker already handles persistent failures.
                return self.template_with_kb(ctx);
            }
        };

        // 3. Validate output
        let validated = match self.validator.validate(&full_text, &ctx.triage_current) {
            crate::validator::ValidationResult::Pass(text)
            | crate::validator::ValidationResult::PassWithWarning(text, _) => text,
            crate::validator::ValidationResult::FailAndFallback(reasons) => {
                tracing::warn!("Validator rejected LLM output: {:?}", reasons);
                return self.template_with_kb(ctx);
            }
        };

        // 4. Extract risk adjustment (second opinion)
        let risk_adjust = self.risk_extractor.extract(&validated);

        AnalysisResult {
            patient_id,
            text: validated,
            risk_adjustment: risk_adjust,
            source: AnalysisSource::LLM,
            degrade_level,
            generated_at_ms: now_ms(),
        }
    }

    // ── Local Analysis Fallback ─────────────────────────────────────────

    fn template_with_kb(&self, ctx: &StructuredContext) -> AnalysisResult {
        // Build FallbackContext from StructuredContext for the template engine
        use crate::fallback::FallbackContext;
        use crate::patient_record::PatientRecord;
        use crate::sliding_window::VitalTrendSummary;
        use crate::medical_knowledge::MatchInput;

        // Use cached KB (loaded once at construction time)
        let kb = self.cached_medical_kb.as_ref();

        let patient = PatientRecord {
            patient_id: ctx.patient_id.to_string(),
            name: None,
            age: ctx.patient_history.as_ref().and_then(|h| {
                h.age_estimate.as_ref().and_then(|a| match a.as_str() {
                    "Elderly" => Some(70u8),
                    "Adult" => Some(40u8),
                    "Child" => Some(10u8),
                    "Infant" => Some(1u8),
                    _ => None,
                })
            }),
            gender: None,
            pre_existing: ctx
                .patient_history
                .as_ref()
                .map(|h| h.prior_conditions.clone())
                .unwrap_or_default(),
            chief_complaint: None,
            allergies: vec![],
            medications: vec![],
            node_id: Some(ctx.node_id),
            admission_time: None,
            notes: None,
        };

        let matched = kb.map(|kb| {
            kb.match_conditions(
                &MatchInput {
                    breathing_rate: ctx.vitals_current.breathing_rate_bpm.map(|v| v as f64),
                    heart_rate: ctx.vitals_current.heart_rate_bpm.map(|v| v as f64),
                    motion_score: match ctx.vitals_current.motion_class.as_deref() {
                        Some("active") => 0.9,
                        Some("present_still") => 0.3,
                        _ => 0.1,
                    },
                    breathing_trend: match ctx.vitals_trend_1min.direction {
                        crate::sliding_window::TrendDirection::Rising => "Rising",
                        crate::sliding_window::TrendDirection::Falling => "Falling",
                        _ => "Stable",
                    },
                    heart_trend: match ctx.vitals_trend_1min.direction {
                        crate::sliding_window::TrendDirection::Rising => "Rising",
                        crate::sliding_window::TrendDirection::Falling => "Falling",
                        _ => "Stable",
                    },
                    motion_pattern: "ContinuousStill",
                    pre_existing: &patient.pre_existing,
                    age: patient.age,
                    active_edge_alerts: &ctx.recent_alerts,
                },
                3,
            )
        }).unwrap_or_default();

        // NOTE: StructuredContext carries a single vitals_trend_1min that aggregates
        // both BR and HR into one direction/delta. When heart_rate_bpm is absent we
        // cannot infer a meaningful HR-specific trend, so we fall back to Stable/0.
        let hr_present = ctx.vitals_current.heart_rate_bpm.is_some();
        let trend = VitalTrendSummary {
            rr_mean: ctx.vitals_current.breathing_rate_bpm.unwrap_or(16.0) as f64,
            rr_trend: ctx.vitals_trend_1min.direction,
            rr_change_pct: ctx.vitals_trend_1min.delta_pct as f64,
            hr_mean: ctx.vitals_current.heart_rate_bpm.unwrap_or(0.0) as f64,
            hr_trend: if hr_present {
                ctx.vitals_trend_1min.direction
            } else {
                crate::sliding_window::TrendDirection::Stable
            },
            hr_change_pct: if hr_present {
                ctx.vitals_trend_1min.delta_pct as f64
            } else {
                0.0
            },
            motion_pattern: crate::sliding_window::MotionPattern::ContinuousStill,
            ..Default::default()
        };

        let fb_ctx = FallbackContext {
            patient,
            current_rr: ctx.vitals_current.breathing_rate_bpm.map(|v| v as f64),
            current_hr: ctx.vitals_current.heart_rate_bpm.map(|v| v as f64),
            current_motion: match ctx.vitals_current.motion_class.as_deref() {
                Some("active") => 0.9,
                Some("present_still") => 0.3,
                _ => 0.1,
            },
            current_signal_quality: ctx.vitals_current.signal_quality as f64,
            current_triage: ctx.triage_current.clone(),
            trend_summary: trend,
            matched_conditions: matched,
            active_edge_alerts: ctx.recent_alerts.clone(),
        };

        TemplateEngine::generate(&fb_ctx)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

