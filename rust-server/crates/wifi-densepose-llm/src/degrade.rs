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
    /// Maximum age of a cached analysis result before it is considered stale.
    /// Must be <= cooldown_secs to avoid the L4CachedReplay mismatch window.
    pub cache_ttl_secs: u64,
}

impl Default for DegradationConfig {
    fn default() -> Self {
        Self {
            cooldown_secs: 300,
            max_cache_size: 32,
            network_failure_threshold: 5,
            cache_ttl_secs: 120,
        }
    }
}

pub struct DegradationManager {
    config: DegradationConfig,
    pub(crate) network_reachable: bool,
    pub(crate) circuit_breaker_open: bool,
    pub(crate) consecutive_failures: u8,
    pub(crate) cooldowns: HashMap<u32, Instant>,
    pub(crate) analysis_cache: Vec<(u32, AnalysisResult, Instant)>,
}

impl DegradationManager {
    pub fn new() -> Self {
        Self::with_config(DegradationConfig::default())
    }

    pub fn with_config(mut config: DegradationConfig) -> Self {
        // Enforce the documented invariant: cache TTL must not exceed cooldown,
        // otherwise the L4CachedReplay logic is silently broken.
        // Clamp + warn in release builds (not just debug_assert! which evaporates).
        if config.cache_ttl_secs > config.cooldown_secs {
            tracing::warn!(
                "cache_ttl_secs ({}) exceeds cooldown_secs ({}) — clamping TTL to cooldown. \
                 Fix [server.agent.degradation] in wces.config.toml",
                config.cache_ttl_secs, config.cooldown_secs
            );
            config.cache_ttl_secs = config.cooldown_secs;
        }
        Self {
            config,
            network_reachable: true,
            circuit_breaker_open: false,
            consecutive_failures: 0,
            cooldowns: HashMap::new(),
            analysis_cache: Vec::new(),
        }
    }

    /// Whether a non-expired cache entry exists for the given patient.
    fn has_valid_cache(&self, patient_id: u32) -> bool {
        let ttl = self.config.cache_ttl_secs;
        self.analysis_cache.iter().any(|(id, _, cached_at)| {
            *id == patient_id && cached_at.elapsed().as_secs() <= ttl
        })
    }

    /// Assess degradation level before each analysis request.
    pub fn assess(&mut self, patient_id: u32) -> DegradationLevel {
        // 0. Evict expired cooldown entries to prevent unbounded memory growth.
        //    Runs inline on assess() calls (periodic, not per-frame) — a
        //    dedicated reaper timer would be overkill for the expected scale.
        let cooldown = self.config.cooldown_secs;
        self.cooldowns.retain(|_, t| t.elapsed().as_secs() < cooldown);

        // 1. Check cooldown cache — must be in cooldown AND have a
        //    non-expired cache entry to return L4CachedReplay.
        if let Some(last) = self.cooldowns.get(&patient_id) {
            if last.elapsed().as_secs() < self.config.cooldown_secs {
                if self.has_valid_cache(patient_id) {
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
        self.analysis_cache.retain(|(id, _, _)| *id != patient_id);
        if self.analysis_cache.len() >= self.config.max_cache_size {
            self.analysis_cache.remove(0);
        }
        self.analysis_cache.push((patient_id, result, Instant::now()));
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
    /// Returns None if the cache entry is older than cache_ttl_secs.
    pub fn get_cached(&self, patient_id: u32) -> Option<&AnalysisResult> {
        let ttl = self.config.cache_ttl_secs;
        self.analysis_cache
            .iter()
            .rev()
            .find(|(id, _, _)| *id == patient_id)
            .and_then(|(_, result, cached_at)| {
                if cached_at.elapsed().as_secs() <= ttl {
                    Some(result)
                } else {
                    None
                }
            })
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
        // Recent entries should exist (within TTL)
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
    fn test_cache_expires_after_ttl() {
        let mut dm = DegradationManager::new();
        dm.on_analysis_complete(1, make_result(1));
        // Manually backdate the cache entry timestamp
        dm.analysis_cache[0].2 = Instant::now() - std::time::Duration::from_secs(121);
        // Cache should be expired
        assert!(dm.get_cached(1).is_none());
    }

    #[test]
    fn test_breaker_and_network_both_down() {
        let mut dm = DegradationManager::new();
        dm.on_network_change(false);
        dm.on_circuit_breaker_change(true);
        assert_eq!(dm.assess(1), DegradationLevel::L2TemplateWithKB);
    }
}
