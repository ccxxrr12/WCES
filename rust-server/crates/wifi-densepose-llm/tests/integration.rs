//! Integration tests for the LLM analysis engine.
//!
//! These tests verify end-to-end functionality of the analysis pipeline.

#[cfg(test)]
mod tests {
    use wifi_densepose_llm::{
        FallbackAnalyzer, FallbackContext, LlmConfig, MatchInput, MedicalKnowledgeBase,
        PatientRecord, PromptBuilder, PromptContext, SlidingWindow, TrendDirection,
        VitalSnapshot, VitalTrendSummary,
    };
    use std::time::Instant;

    // ── Medical Knowledge Base ───────────────────────────────────────────────

    #[test]
    fn test_knowledge_base_loads_all_conditions() {
        let kb = MedicalKnowledgeBase::load("data/medical_knowledge.json")
            .expect("Should load knowledge base");
        assert!(
            kb.condition_count() >= 8,
            "Should have 8+ conditions, got {}",
            kb.condition_count()
        );
    }

    #[test]
    fn test_knowledge_base_matches_copd_case() {
        let kb = MedicalKnowledgeBase::load("data/medical_knowledge.json").unwrap();

        let input = MatchInput {
            breathing_rate: Some(28.0),
            heart_rate: Some(105.0),
            motion_score: 0.4,
            breathing_trend: "Rising",
            heart_trend: "Rising",
            motion_pattern: "IntermittentMotion",
            pre_existing: &["COPD".into(), "高血压".into()],
            age: Some(68),
            active_edge_alerts: &["med_respiratory_distress".into()],
        };

        let matches = kb.match_conditions(&input, 3);
        assert!(!matches.is_empty());

        // COPD exacerbation should be in top results
        let has_copd = matches.iter().any(|(c, _)| c.id == "copd_exacerbation");
        assert!(has_copd, "COPD exacerbation should be in match results");
    }

    // ── Patient Record ──────────────────────────────────────────────────────

    #[test]
    fn test_patient_record_summary() {
        let mut record = PatientRecord::new("PAT-TEST");
        record.pre_existing = vec!["COPD".into(), "2型糖尿病".into()];
        record.chief_complaint = Some("胸痛".into());

        assert_eq!(record.pre_existing_summary(), "COPD、2型糖尿病");
        assert!(record.has_condition("COPD"));
        assert!(!record.has_condition("心脏病"));
    }

    // ── Sliding Window ──────────────────────────────────────────────────────

    #[test]
    fn test_trend_detection_rising() {
        let mut sw = SlidingWindow::new(60, 300, 1800);

        // Simulate rapidly rising RR
        for i in 0..20 {
            sw.push(VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate: 16.0 + i as f64 * 1.5, // 16 → 44.5
                heart_rate: 72.0 + i as f64 * 2.0,     // 72 → 110
                motion_score: 0.3,
                signal_quality: 0.9,
            });
        }

        let summary = sw.medium_summary();
        assert_eq!(summary.rr_trend, TrendDirection::Rising);
        assert_eq!(summary.hr_trend, TrendDirection::Rising);
        assert!(summary.rr_change_pct > 50.0);
    }

    #[test]
    fn test_trend_detection_stable() {
        let mut sw = SlidingWindow::new(60, 300, 1800);

        for _ in 0..20 {
            sw.push(VitalSnapshot {
                timestamp: Instant::now(),
                breathing_rate: 16.0,
                heart_rate: 72.0,
                motion_score: 0.1,
                signal_quality: 0.95,
            });
        }

        let summary = sw.medium_summary();
        assert_eq!(summary.rr_trend, TrendDirection::Stable);
        assert_eq!(summary.hr_trend, TrendDirection::Stable);
    }

    // ── Prompt Builder ──────────────────────────────────────────────────────

    #[test]
    fn test_prompt_builder_includes_all_sections() {
        let patient = PatientRecord {
            patient_id: "PAT-001".into(),
            name: Some("张三".into()),
            age: Some(65),
            gender: Some(wifi_densepose_llm::Gender::Male),
            pre_existing: vec!["COPD".into()],
            chief_complaint: Some("呼吸困难".into()),
            allergies: vec!["青霉素".into()],
            medications: vec!["沙丁胺醇".into()],
            node_id: Some(1),
            admission_time: None,
            notes: None,
        };

        let trend = VitalTrendSummary::default();

        let kb = MedicalKnowledgeBase::load("data/medical_knowledge.json").unwrap();
        let matched = kb.match_conditions(
            &MatchInput {
                breathing_rate: Some(16.0),
                heart_rate: Some(72.0),
                motion_score: 0.1,
                breathing_trend: "Stable",
                heart_trend: "Stable",
                motion_pattern: "ContinuousStill",
                pre_existing: &["COPD".into()],
                age: Some(65),
                active_edge_alerts: &[],
            },
            2,
        );

        let ctx = PromptContext {
            patient,
            current_rr: Some(16.0),
            current_hr: Some(72.0),
            current_motion: 0.1,
            current_signal_quality: 0.95,
            current_triage: "Minor".into(),
            trend_summary: trend,
            matched_conditions: matched,
            active_edge_alerts: vec![],
        };

        let built = PromptBuilder::build(&ctx);
        assert!(built.estimated_tokens > 100);
        assert!(built.prompt.contains("PAT-001"));
        assert!(built.prompt.contains("COPD"));
        assert!(built.prompt.contains("Minor"));
    }

    // ── Fallback Analyzer ──────────────────────────────────────────────────

    #[test]
    fn test_fallback_seizure_detection() {
        let patient = PatientRecord::new("PAT-SEIZURE");
        let mut trend = VitalTrendSummary::default();
        trend.motion_pattern = wifi_densepose_llm::MotionPattern::SpikeAndDrop;
        trend.rr_mean = 25.0;
        trend.hr_mean = 130.0;

        let kb = MedicalKnowledgeBase::load("data/medical_knowledge.json").unwrap();
        let matched = kb.match_conditions(
            &MatchInput {
                breathing_rate: Some(25.0),
                heart_rate: Some(130.0),
                motion_score: 0.9,
                breathing_trend: "Rising",
                heart_trend: "Rising",
                motion_pattern: "SpikeAndDrop",
                pre_existing: &[],
                age: Some(30),
                active_edge_alerts: &["med_seizure_detect".into()],
            },
            3,
        );

        let ctx = FallbackContext {
            patient,
            current_rr: Some(25.0),
            current_hr: Some(130.0),
            current_motion: 0.9,
            current_signal_quality: 0.5,
            current_triage: "Delayed".into(),
            trend_summary: trend,
            matched_conditions: matched,
            active_edge_alerts: vec!["med_seizure_detect".into()],
        };

        let result = FallbackAnalyzer::analyze(&ctx);
        // Seizure with spike-and-drop should suggest upgrade
        assert!(!result.triage_opinion.agrees_with_start);
        assert_eq!(result.triage_opinion.suggested_level, "Immediate");
        assert!(result.recommendations.iter().any(|r| r.contains("保护")));
    }

    #[test]
    fn test_fallback_normal_patient() {
        let patient = PatientRecord::new("PAT-NORMAL");
        let trend = VitalTrendSummary {
            rr_mean: 16.0,
            rr_trend: TrendDirection::Stable,
            rr_change_pct: 0.0,
            hr_mean: 72.0,
            hr_trend: TrendDirection::Stable,
            hr_change_pct: 0.0,
            motion_pattern: wifi_densepose_llm::MotionPattern::ContinuousStill,
            ..Default::default()
        };

        let ctx = FallbackContext {
            patient,
            current_rr: Some(16.0),
            current_hr: Some(72.0),
            current_motion: 0.1,
            current_signal_quality: 0.95,
            current_triage: "Minor".into(),
            trend_summary: trend,
            matched_conditions: vec![],
            active_edge_alerts: vec![],
        };

        let result = FallbackAnalyzer::analyze(&ctx);
        assert!(result.triage_opinion.agrees_with_start);
        assert_eq!(result.risk_assessment.overall_level, "low");
        assert!(!result.risk_assessment.deteriorating);
    }

    // ── Config ──────────────────────────────────────────────────────────────

    #[test]
    fn test_config_defaults() {
        let config = LlmConfig::default();
        assert_eq!(config.short_window_secs, 60);
        assert_eq!(config.medium_window_secs, 300);
        assert_eq!(config.long_window_secs, 1800);
        assert_eq!(config.periodic_interval_secs, 30);
        assert_eq!(config.analysis_timeout_secs, 120);
    }

    #[test]
    fn test_config_competition() {
        let config = LlmConfig::competition();
        assert_eq!(config.short_window_secs, 60);
        assert_eq!(config.medium_window_secs, 300);
        assert_eq!(config.periodic_interval_secs, 30);
    }

    // ── Agent Integration ────────────────────────────────────────────────────

    use wifi_densepose_llm::{
        AgentVitalSnapshot, AnalysisSource, DegradationLevel, MedicalAgent, MedicalKb,
        StructuredContext, TriggerSource, TrendSummary,
    };

    #[tokio::test]
    async fn test_agent_analyze_skip_deceased() {
        let mut agent = MedicalAgent::new_template_only();
        let ctx = make_context(1, "Deceased", TriggerSource::PeriodicScan);
        let result = agent.analyze(ctx).await;
        assert!(result.text.is_empty() || result.degrade_level >= DegradationLevel::L3TemplateOnly);
    }

    #[tokio::test]
    async fn test_agent_analyze_minor_returns_template() {
        let mut agent = MedicalAgent::new_template_only();
        let ctx = make_context(2, "Minor", TriggerSource::PeriodicScan);
        let result = agent.analyze(ctx).await;
        assert_eq!(result.source, AnalysisSource::Template);
        assert!(result.degrade_level >= DegradationLevel::L2TemplateWithKB);
    }

    #[tokio::test]
    async fn test_agent_analyze_immediate_deteriorating() {
        let mut agent = MedicalAgent::new_template_only();
        let ctx = make_context(3, "Immediate", TriggerSource::Deterioration {
            patient_id: 3,
            from: "Delayed".into(),
            to: "Immediate".into(),
        });
        let result = agent.analyze(ctx).await;
        assert_eq!(result.patient_id, 3);
        assert!(!result.text.is_empty() || result.source == AnalysisSource::Template);
    }

    #[tokio::test]
    async fn test_agent_cooldown_caches_result() {
        let mut agent = MedicalAgent::new_template_only();
        let ctx = make_context(4, "Minor", TriggerSource::PeriodicScan);

        let r1 = agent.analyze(ctx.clone()).await;
        let r2 = agent.analyze(ctx).await;

        assert_eq!(r1.text, r2.text);
        assert_eq!(r1.patient_id, r2.patient_id);
    }

    #[test]
    fn test_medical_kb_loads_agent_json() {
        let kb = MedicalKb::load("data/agent_kb.json")
            .expect("Should load agent KB");
        assert!(kb.entry_count() >= 15, "Should have 15+ entries, got {}", kb.entry_count());

        let vitals = AgentVitalSnapshot {
            breathing_rate_bpm: Some(32.0),
            heart_rate_bpm: Some(135.0),
            breathing_confidence: 0.9,
            heartbeat_confidence: 0.85,
            signal_quality: 0.7,
            motion_class: Some("present_still".into()),
            person_count_estimate: Some(1),
            rssi: Some(-45),
        };

        let matches = kb.match_vitals(&vitals);
        assert!(!matches.is_empty(), "should match tachycardia or respiratory_distress");
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_context(patient_id: u32, triage: &str, trigger: TriggerSource) -> StructuredContext {
        StructuredContext {
            patient_id,
            node_id: 1,
            vitals_current: AgentVitalSnapshot {
                breathing_rate_bpm: Some(18.0),
                heart_rate_bpm: Some(80.0),
                breathing_confidence: 0.9,
                heartbeat_confidence: 0.85,
                signal_quality: 0.8,
                motion_class: Some("present_still".into()),
                person_count_estimate: Some(1),
                rssi: Some(-45),
            },
            vitals_trend_1min: TrendSummary {
                direction: TrendDirection::Stable,
                delta: 0.0,
                delta_pct: 0.0,
                anomaly_score: 1.0,
                data_points: 10,
            },
            vitals_trend_5min: TrendSummary {
                direction: TrendDirection::Stable,
                delta: 0.0,
                delta_pct: 0.0,
                anomaly_score: 1.0,
                data_points: 50,
            },
            triage_current: triage.to_string(),
            triage_trajectory: vec![],
            patient_history: None,
            recent_alerts: vec![],
            kb_matches: vec![],
            triggered_by: trigger,
            built_at_ms: 0,
        }
    }
}
