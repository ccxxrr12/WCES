//! Analysis Router — decides which analysis path to take.
//!
//! Pure function: inputs → RouteDecision. No side effects, <0.1ms.

use crate::types::{AnalysisRoute, RouteDecision};

pub struct AnalysisRouter;

/// Normalize a triage label into a canonical category string.
///
/// Uses **exact** matching (after trim + lowercase) rather than `contains`
/// to avoid false positives such as `"Immediate (reclassified from Deceased)"`
/// matching `deceased` first and routing to `Skip`.
///
/// Returns one of: `"immediate"`, `"delayed"`, `"minor"`, `"deceased"`,
/// `"unknown"`, or `"other"` (for unrecognized labels).
fn normalize_triage(s: &str) -> &'static str {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "immediate" | "red" | "红" | "红色" => "immediate",
        "delayed" | "yellow" | "黄" | "黄色" => "delayed",
        "minor" | "green" | "绿" | "绿色" => "minor",
        "deceased" | "black" | "黑" | "黑色" => "deceased",
        "unknown" => "unknown",
        _ => "other",
    }
}

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

        let triage_norm = normalize_triage(triage);

        let route = if triage_norm == "deceased" || triage_norm == "unknown" {
            Skip
        } else if !network_reachable {
            // No network → fall back to local KB-enhanced template
            TemplateWithKB
        } else if triage_norm == "immediate" {
            if is_deteriorating {
                DeepLLM
            } else {
                BriefLLM
            }
        } else if triage_norm == "delayed" {
            if is_deteriorating {
                DeepLLM
            } else {
                BriefLLM
            }
        } else if triage_norm == "minor" {
            TemplateWithKB
        } else {
            // "other" — unrecognized triage label, safest local fallback
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

    /// L7: a reclassified triage label must not be misrouted by substring
    /// matching. `"Immediate (reclassified from Deceased)"` previously hit
    /// the `contains("deceased")` branch and was routed to `Skip`; with
    /// exact matching it now routes as `Immediate`.
    #[test]
    fn test_reclassified_label_not_misrouted() {
        let d = AnalysisRouter::decide(
            "Immediate (reclassified from Deceased)",
            true,
            true,
            false,
        );
        assert_ne!(d.route, AnalysisRoute::Skip,
            "reclassified Immediate must not be skipped via deceased substring");
    }

    #[test]
    fn test_normalized_labels() {
        assert_eq!(normalize_triage("Immediate"), "immediate");
        assert_eq!(normalize_triage(" red "), "immediate");
        assert_eq!(normalize_triage("红"), "immediate");
        assert_eq!(normalize_triage("Delayed"), "delayed");
        assert_eq!(normalize_triage("yellow"), "delayed");
        assert_eq!(normalize_triage("Minor"), "minor");
        assert_eq!(normalize_triage("green"), "minor");
        assert_eq!(normalize_triage("Deceased"), "deceased");
        assert_eq!(normalize_triage("black"), "deceased");
        assert_eq!(normalize_triage("unknown"), "unknown");
        assert_eq!(normalize_triage("nonsense"), "other");
    }
}
