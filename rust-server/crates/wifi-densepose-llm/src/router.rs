//! Analysis Router — decides which analysis path to take.
//!
//! Pure function: inputs → RouteDecision. No side effects, <0.1ms.

use crate::types::{AnalysisRoute, RouteDecision};

pub struct AnalysisRouter;

impl AnalysisRouter {
    /// Decide the analysis route based on triage level, deterioration status,
    /// network availability, and cooldown state.
    pub fn decide(
        triage: &str,
        is_deteriorating: bool,
        network_reachable: bool,
        in_cooldown: bool,
    ) -> RouteDecision {
        use AnalysisRoute::*;

        if in_cooldown {
            return RouteDecision {
                route: CachedReplay,
                reason: "cooldown active".into(),
                max_output_tokens: 0,
                priority: 0,
            };
        }

        let triage_lower = triage.to_lowercase();

        let route = if triage_lower.contains("deceased") || triage_lower.contains("unknown") {
            Skip
        } else if !network_reachable {
            // No network → fall back to local KB-enhanced template
            TemplateWithKB
        } else if triage_lower.contains("immediate") || triage_lower.contains("red") {
            if is_deteriorating {
                DeepLLM
            } else {
                BriefLLM
            }
        } else if triage_lower.contains("delayed") || triage_lower.contains("yellow") {
            if is_deteriorating {
                DeepLLM
            } else {
                BriefLLM
            }
        } else if triage_lower.contains("minor") || triage_lower.contains("green") {
            TemplateWithKB
        } else {
            TemplateWithKB
        };

        let (max_output_tokens, priority) = match route {
            DeepLLM => (300, 3),
            BriefLLM => (150, 2),
            TemplateWithKB => (0, 1),
            TemplateOnly => (0, 1),
            CachedReplay => (0, 0),
            Skip => (0, 0),
        };

        RouteDecision {
            route,
            reason: format!(
                "triage={} deteriorating={} network={}",
                triage, is_deteriorating, network_reachable
            ),
            max_output_tokens,
            priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AnalysisRoute;

    #[test]
    fn test_immediate_deteriorating_deep_llm() {
        let d = AnalysisRouter::decide("Immediate", true, true, false);
        assert_eq!(d.route, AnalysisRoute::DeepLLM);
        assert_eq!(d.max_output_tokens, 300);
    }

    #[test]
    fn test_immediate_stable_brief_llm() {
        let d = AnalysisRouter::decide("Immediate", false, true, false);
        assert_eq!(d.route, AnalysisRoute::BriefLLM);
        assert_eq!(d.max_output_tokens, 150);
    }

    #[test]
    fn test_minor_template_with_kb() {
        let d = AnalysisRouter::decide("Minor", false, true, false);
        assert_eq!(d.route, AnalysisRoute::TemplateWithKB);
    }

    #[test]
    fn test_deceased_skip() {
        let d = AnalysisRouter::decide("Deceased", false, true, false);
        assert_eq!(d.route, AnalysisRoute::Skip);
    }

    #[test]
    fn test_cooldown_cached_replay() {
        let d = AnalysisRouter::decide("Immediate", true, true, true);
        assert_eq!(d.route, AnalysisRoute::CachedReplay);
    }

    #[test]
    fn test_no_network_fallback() {
        let d = AnalysisRouter::decide("Immediate", true, false, false);
        assert_eq!(d.route, AnalysisRoute::TemplateWithKB);
    }
}
