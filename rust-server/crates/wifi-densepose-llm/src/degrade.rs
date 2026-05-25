//! Degradation Ladder — graceful fallback when network/LLM is unavailable.
//!
//! State machine: L0 (full LLM) → L1 (brief LLM) → L2 (template+KB) →
//! L3 (template only) → L4 (cached replay).

use crate::types::{AnalysisResult, DegradationLevel};
use std::collections::HashMap;
use std::time::Instant;

/// Degradation configuration (mirrors [server.agent.degradation] in wces.config.toml).
#[derive(Debug, Clone)]
pub struct DegradationConfig {
    pub cooldown_secs: u64,
    pub max_cache_size: usize,
    pub network_failure_threshold: u8,
}

impl Default for DegradationConfig {
    fn default() -> Self {
        Self { cooldown_secs: 300, max_cache_size: 32, network_failure_threshold: 5 }
    }
}

pub struct DegradationManager {
    config: DegradationConfig,
    pub(crate) network_reachable: bool,
    pub(crate) circuit_breaker_open: bool,
    pub(crate) consecutive_failures: u8,
    pub(crate) cooldowns: HashMap<u32, Instant>,
    pub(crate) analysis_cache: Vec<(u32, AnalysisResult)>,
}

impl DegradationManager {
    pub fn new() -> Self {
        Self::with_config(DegradationConfig::default())
    }

    pub fn with_config(config: DegradationConfig) -> Self {
        Self {
            config,
            network_reachable: true,
            circuit_breaker_open: false,
            consecutive_failures: 0,
            cooldowns: HashMap::new(),
            analysis_cache: Vec::new(),
        }
    }

    /// Assess degradation level before each analysis request.
    pub fn assess(&mut self, patient_id: u32) -> DegradationLevel {
        // 1. Check cooldown cache
        if let Some(last) = self.cooldowns.get(&patient_id) {
            if last.elapsed().as_secs() < self.config.cooldown_secs {
                if self.analysis_cache.iter().any(|(id, _)| *id == patient_id) {
                    return DegradationLevel::L4CachedReplay;
                }
            }
        }

        // 2. Check network
        if !self.network_reachable {
            return DegradationLevel::L2TemplateWithKB;
        }

        // 3. Check circuit breaker
        if self.circuit_breaker_open {
            return DegradationLevel::L2TemplateWithKB;
        }

        // 4. Normal path — let Router decide between L0/L1
        DegradationLevel::L0FullLLM
    }

    /// Record a completed analysis.
    pub fn on_analysis_complete(&mut self, patient_id: u32, result: AnalysisResult) {
        self.cooldowns.insert(patient_id, Instant::now());
        self.analysis_cache.retain(|(id, _)| *id != patient_id);
        if self.analysis_cache.len() >= self.config.max_cache_size {
            self.analysis_cache.remove(0);
        }
        self.analysis_cache.push((patient_id, result));
    }

    /// Update network status.
    pub fn on_network_change(&mut self, reachable: bool) {
        self.network_reachable = reachable;
        if reachable {
            self.consecutive_failures = 0;
        }
    }

    /// Update circuit breaker status.
    pub fn on_circuit_breaker_change(&mut self, open: bool) {
        self.circuit_breaker_open = open;
    }

    /// Record a failure, potentially escalating degradation.
    pub fn on_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.config.network_failure_threshold {
            self.network_reachable = false;
        }
    }

    /// Look up cached result for a patient.
    pub fn get_cached(&self, patient_id: u32) -> Option<&AnalysisResult> {
        self.analysis_cache
            .iter()
            .rev()
            .find(|(id, _)| *id == patient_id)
            .map(|(_, r)| r)
    }
}

impl Default for DegradationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnalysisSource, DegradationLevel};

    fn make_result(id: u32) -> AnalysisResult {
        AnalysisResult {
            patient_id: id,
            text: "test analysis".into(),
            risk_adjustment: None,
            source: AnalysisSource::Template,
            degrade_level: DegradationLevel::L0FullLLM,
            generated_at_ms: 0,
        }
    }

    #[test]
    fn test_normal_path_returns_l0() {
        let mut dm = DegradationManager::new();
        assert_eq!(dm.assess(1), DegradationLevel::L0FullLLM);
    }

    #[test]
    fn test_network_down_returns_l2() {
        let mut dm = DegradationManager::new();
        dm.on_network_change(false);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
    }

    #[test]
    fn test_circuit_breaker_open_returns_l2() {
        let mut dm = DegradationManager::new();
        dm.on_circuit_breaker_change(true);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
    }

    #[test]
    fn test_cooldown_with_cache_returns_l4() {
        let mut dm = DegradationManager::new();
        dm.on_analysis_complete(1, make_result(1));
        // Immediately re-assess — should find cached result
        assert_eq!(dm.assess(1), DegradationLevel::L4CachedReplay);
    }

    #[test]
    fn test_cooldown_without_cache_returns_l0() {
        let mut dm = DegradationManager::new();
        // Set cooldown manually without inserting cache
        dm.cooldowns.insert(1, Instant::now());
        // Cooldown active but no cache entry → still L0 (not L4)
        assert_eq!(dm.assess(1), DegradationLevel::L0FullLLM);
    }

    #[test]
    fn test_network_recovery_resets_failures() {
        let mut dm = DegradationManager::new();
        dm.on_network_change(false);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
        dm.on_network_change(true);
        assert_eq!(dm.assess(1), DegradationLevel::L0FullLLM);
        assert_eq!(dm.consecutive_failures, 0);
    }

    #[test]
    fn test_failure_accumulation_disables_network() {
        let mut dm = DegradationManager::new();
        for _ in 0..5 {
            dm.on_failure();
        }
        // After 5 consecutive failures, network is marked unreachable
        assert!(!dm.network_reachable);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
    }

    #[test]
    fn test_failure_below_threshold_keeps_network() {
        let mut dm = DegradationManager::new();
        for _ in 0..4 {
            dm.on_failure();
        }
        assert!(dm.network_reachable);
        assert_eq!(dm.assess(1), DegradationLevel::L0FullLLM);
    }

    #[test]
    fn test_cache_eviction_at_max_size() {
        let mut dm = DegradationManager::new();
        let cap = dm.config.max_cache_size;
        for i in 0..cap + 5 {
            dm.on_analysis_complete(i as u32, make_result(i as u32));
        }
        // Oldest entries should be evicted
        assert!(dm.get_cached(0).is_none());
        assert!(dm.get_cached(4).is_none());
        // Recent entries should exist
        assert!(dm.get_cached((cap + 4) as u32).is_some());
    }

    #[test]
    fn test_get_cached_returns_latest() {
        let mut dm = DegradationManager::new();
        dm.on_analysis_complete(1, make_result(1));

        // Update with newer result
        let mut result2 = make_result(1);
        result2.text = "updated analysis".into();
        dm.on_analysis_complete(1, result2);

        let cached = dm.get_cached(1).expect("should have cached");
        assert_eq!(cached.text, "updated analysis");
    }

    #[test]
    fn test_breaker_and_network_both_down() {
        let mut dm = DegradationManager::new();
        dm.on_network_change(false);
        dm.on_circuit_breaker_change(true);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
    }
}
