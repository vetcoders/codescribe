//! Agent-facing operating doctrine appended to the system prompt: how to drive
//! long-running review-tool MCP servers (health/state/run, the
//! started->completed state machine, run_id/base_used reporting) and how to fall
//! back from the GitHub connector to the local checkout. Prompt-layer only — no
//! executor changes; this pins agent behaviour without touching tool internals.

/// A concise review-tool + connector-fallback doctrine for the agent system
/// prompt. Kept tight on purpose: prompt space is a scarce resource, so this
/// section states the operating rules and nothing else.
pub fn review_doctrine_prompt_section() -> String {
    "REVIEW TOOLS & CONNECTORS\n\
     Treat review-tool MCP servers (prview and any similar) as agent-facing \
     review systems, not CLI parsers. Sequence: check `health`, then inspect \
     `state`, and start a review only when no completed run exists for the \
     current HEAD. `running` is an intermediate state, never a verdict — do not \
     report it as a result. Always report run_id, base_used, and whether the \
     result is final. For multi-branch work, consider passing an explicit base \
     instead of relying on develop/main/master fallback.\n\
     For long-running calls, report a mini state machine — started / running / \
     stale / completed / failed / needs-rerun — and say \"review still running, \
     poll the verdict later\" rather than inventing an outcome. Poll with a \
     small retry cap and backoff; do not spam tool calls.\n\
     If a GitHub fetch (`fetch_github_file`) fails once or twice, stop retrying: \
     switch to the local checkout as the source of truth (`list_projects` + \
     git) and report the connector failure as a separate problem, not as \
     missing repository data."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctrine_section_carries_the_load_bearing_anchors() {
        let section = review_doctrine_prompt_section();
        // Header + the operator's follow-up-prompt invariants must survive edits.
        assert!(section.starts_with("REVIEW TOOLS & CONNECTORS"));
        for anchor in [
            "health",
            "state",
            "run_id",
            "base_used",
            "running",
            "needs-rerun",
            "poll the verdict later",
            "fetch_github_file",
            "local checkout",
            "list_projects",
            "separate problem",
        ] {
            assert!(
                section.contains(anchor),
                "doctrine section missing anchor: {anchor}"
            );
        }
    }
}
