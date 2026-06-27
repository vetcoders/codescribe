//! Grouped Tool Activity model for the Assistive conversation timeline.
//!
//! Product rule: the primary timeline is for **conversation**. Tool calls are
//! **evidence/activity**, raw tool output is **debug**. An assistant answer must
//! never be interrupted by individual tool-call log cards.
//!
//! Before this module, each completed tool emitted its own `System` chat bubble
//! (`tool_evidence_line`). With several tools per turn those single cards
//! interleaved with the streaming assistant answer, forcing the reader to
//! mentally reconstruct execution order. Here we accumulate every tool event of
//! one assistant turn into a single [`ToolActivityGroup`] and render it as one
//! compact block, separate from the assistant answer.
//!
//! This module is intentionally pure (no AppKit, no global state): the grouping
//! and summary rendering are unit-testable without a running UI. The voice-chat
//! state layer owns one group per assistant turn and feeds it raw events; the
//! rendered string is what the timeline shows.

/// Lifecycle of a single tool call within a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Dispatched, result not yet received.
    Running,
    /// Finished with a usable result.
    Completed,
    /// Finished in error.
    Failed,
}

impl ToolStatus {
    fn word(self) -> &'static str {
        match self {
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Failed => "failed",
        }
    }
}

/// One tool call's evidence within a turn. `raw_name` is the transport wire name
/// (`mcp__server__tool`) kept only for debug/telemetry; `display_name` is the
/// human-readable label shown in the timeline.
#[derive(Debug, Clone)]
pub struct ToolActivityEntry {
    pub id: String,
    pub display_name: String,
    pub raw_name: String,
    pub status: ToolStatus,
    /// Short result summary for a completed call (already truncated upstream).
    pub summary: String,
    /// Error reason for a failed call (compact; full payload stays in the log).
    pub error: String,
    /// Optional structured result count when the upstream summary exposes one.
    pub result_count: Option<usize>,
}

/// Maximum characters for a per-entry suffix so the block stays compact and no
/// raw payload / stack dump leaks into the conversation timeline.
const ENTRY_SUFFIX_MAX_CHARS: usize = 80;

fn clamp_suffix(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= ENTRY_SUFFIX_MAX_CHARS {
        return trimmed.to_string();
    }
    let clipped: String = trimmed
        .chars()
        .take(ENTRY_SUFFIX_MAX_CHARS.saturating_sub(1))
        .collect();
    format!("{}…", clipped.trim_end())
}

impl ToolActivityEntry {
    /// One compact line, e.g. `Web search · completed · 10 results` or
    /// `AICX intents · failed · empty index`. Never contains the raw wire name.
    pub fn line(&self) -> String {
        let mut line = format!("{} · {}", self.display_name, self.status.word());
        let suffix = match self.status {
            ToolStatus::Completed => {
                if let Some(count) = self.result_count {
                    let noun = if count == 1 { "result" } else { "results" };
                    format!("{count} {noun}")
                } else {
                    clamp_suffix(&self.summary)
                }
            }
            ToolStatus::Failed => clamp_suffix(&self.error),
            ToolStatus::Running => String::new(),
        };
        if !suffix.is_empty() {
            line.push_str(" · ");
            line.push_str(&suffix);
        }
        line
    }
}

/// All tool evidence for one assistant turn. Owned per-turn by the voice-chat
/// state; rendered once as a single timeline block.
#[derive(Debug, Clone, Default)]
pub struct ToolActivityGroup {
    pub entries: Vec<ToolActivityEntry>,
}

impl ToolActivityGroup {
    /// Record a tool that just started. Idempotent on `id`: a repeated start for
    /// the same call updates its labels but keeps its place.
    pub fn mark_running(&mut self, id: &str, raw_name: &str, display_name: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.raw_name = raw_name.to_string();
            entry.display_name = display_name.to_string();
            return;
        }
        self.entries.push(ToolActivityEntry {
            id: id.to_string(),
            display_name: display_name.to_string(),
            raw_name: raw_name.to_string(),
            status: ToolStatus::Running,
            summary: String::new(),
            error: String::new(),
            result_count: None,
        });
    }

    /// Record a tool result. Matches the running entry by `id`; if no start was
    /// seen (events can be dropped/coalesced) it inserts a finished entry so the
    /// evidence is never lost.
    pub fn mark_result(
        &mut self,
        id: &str,
        raw_name: &str,
        display_name: &str,
        summary: &str,
        is_error: bool,
    ) {
        let status = if is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        };
        let summary_owned = summary.trim().to_string();
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.raw_name = raw_name.to_string();
            entry.display_name = display_name.to_string();
            entry.status = status;
            if is_error {
                entry.error = summary_owned;
            } else {
                entry.summary = summary_owned;
            }
            return;
        }
        self.entries.push(ToolActivityEntry {
            id: id.to_string(),
            display_name: display_name.to_string(),
            raw_name: raw_name.to_string(),
            status,
            summary: if is_error {
                String::new()
            } else {
                summary_owned.clone()
            },
            error: if is_error {
                summary_owned
            } else {
                String::new()
            },
            result_count: None,
        });
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut completed = 0;
        let mut failed = 0;
        let mut running = 0;
        for entry in &self.entries {
            match entry.status {
                ToolStatus::Completed => completed += 1,
                ToolStatus::Failed => failed += 1,
                ToolStatus::Running => running += 1,
            }
        }
        (completed, failed, running)
    }

    /// One-line header summarizing the whole turn's tool activity, e.g.
    /// `Tool activity · 3 calls completed`, `Tool activity · 3 calls · 1 failed`,
    /// or `Tool activity · 2 calls · 1 running`. Failures are always visible here
    /// even when the block is collapsed.
    pub fn compact_header(&self) -> String {
        let total = self.entries.len();
        if total == 0 {
            return "Tool activity".to_string();
        }
        let noun = if total == 1 { "call" } else { "calls" };
        let (_, failed, running) = self.counts();

        if running == total {
            return format!("Tool activity · {total} {noun} running");
        }

        let mut header = format!("Tool activity · {total} {noun}");
        if running > 0 {
            header.push_str(&format!(" · {running} running"));
        }
        if failed > 0 {
            header.push_str(&format!(" · {failed} failed"));
        }
        if running == 0 && failed == 0 {
            // Clean turn: read as "3 calls completed".
            header.push_str(" completed");
        }
        header
    }

    /// Render the block for the timeline. Collapsed → header only (failures still
    /// counted in the header). Expanded (default) → header + one compact line per
    /// tool. Raw payloads are never included — they stay in the debug log.
    pub fn render(&self, collapsed: bool) -> String {
        let header = self.compact_header();
        if collapsed || self.entries.is_empty() {
            return header;
        }
        let mut out = header;
        for entry in &self.entries {
            out.push('\n');
            out.push_str("- ");
            out.push_str(&entry.line());
        }
        out
    }

    /// Roll the turn's entries up by **source** (the system behind the wire
    /// name), preserving first-seen order. Two Loctree calls become one
    /// `Loctree` row; counts and the richest result text are folded in.
    fn source_summaries(&self) -> Vec<SourceSummary> {
        let mut out: Vec<SourceSummary> = Vec::new();
        for entry in &self.entries {
            let source = source_label(&entry.raw_name, &entry.display_name);
            let slot = match out.iter_mut().find(|s| s.source == source) {
                Some(slot) => slot,
                None => {
                    out.push(SourceSummary {
                        source,
                        completed: 0,
                        failed: 0,
                        running: 0,
                        best_count: None,
                        best_summary: String::new(),
                        first_error: String::new(),
                    });
                    out.last_mut().expect("just pushed a source slot")
                }
            };
            match entry.status {
                ToolStatus::Completed => {
                    slot.completed += 1;
                    if let Some(count) = entry.result_count {
                        slot.best_count = Some(slot.best_count.map_or(count, |c| c.max(count)));
                    }
                    let summary = entry.summary.trim();
                    if summary.chars().count() > slot.best_summary.chars().count() {
                        slot.best_summary = summary.to_string();
                    }
                }
                ToolStatus::Failed => {
                    slot.failed += 1;
                    let error = entry.error.trim();
                    if slot.first_error.is_empty() && !error.is_empty() {
                        slot.first_error = error.to_string();
                    }
                }
                ToolStatus::Running => slot.running += 1,
            }
        }
        out
    }

    /// The default operator-facing evidence block for one assistant turn.
    ///
    /// `What I checked · N tools[ · M warning(s)]`, then one compact line per
    /// source (`- Source: detail.`), then a `Key finding:` verdict when one can
    /// be extracted. Deterministic and self-contained — it echoes the results it
    /// already has, never fabricates prose, and never lets a raw `mcp__` wire
    /// name reach the rendered text. This is layer 3 of the product layering
    /// (raw payload = debug, technical list = [`render`], summary = default).
    pub fn evidence_summary(&self) -> String {
        let summaries = self.source_summaries();
        if summaries.is_empty() {
            return "What I checked".to_string();
        }
        let tools = summaries.len();
        let warnings: usize = summaries.iter().map(|s| s.failed).sum();
        let tool_noun = if tools == 1 { "tool" } else { "tools" };
        let mut header = format!("What I checked · {tools} {tool_noun}");
        if warnings > 0 {
            let warn_noun = if warnings == 1 { "warning" } else { "warnings" };
            header.push_str(&format!(" · {warnings} {warn_noun}"));
        }
        let mut out = header;
        for summary in &summaries {
            out.push('\n');
            out.push_str(&format!(
                "- {}: {}",
                summary.source,
                finish_clause(&source_detail(summary))
            ));
        }
        if let Some(key) = evidence_key_finding(&summaries) {
            out.push('\n');
            out.push_str(&format!("Key finding: {key}"));
        }
        out
    }
}

// ---- Evidence Summary helpers (deterministic, no LLM) -------------------------
//
// The summary groups calls by SOURCE — the system behind the wire name — so it
// answers "which sources did I consult", not "how many wire calls fired". These
// are pure, module-local, and unit-testable. Kept separate from
// `friendly_tool_name` (controller/helpers.rs): that table yields the *specific*
// per-call label ("Loctree context"); this yields the *coarse* system heading
// ("Loctree"). Different granularity, not a duplicate path.

/// One source's rolled-up evidence within a turn.
#[derive(Debug, Clone)]
struct SourceSummary {
    source: String,
    completed: usize,
    failed: usize,
    running: usize,
    best_count: Option<usize>,
    best_summary: String,
    first_error: String,
}

/// Coarse source/system a tool call belongs to. Keyed on the wire `raw_name`
/// (the stable system identity); `display_name` is the fallback for native
/// tools that have no `mcp__server__tool` shape. Never emits a raw `mcp__` name.
fn source_label(raw_name: &str, display_name: &str) -> String {
    if let Some(rest) = raw_name.strip_prefix("mcp__") {
        let server = rest.split("__").next().unwrap_or("");
        return match server {
            "brave-search" => "Web search".to_string(),
            "loctree-mcp" => "Loctree".to_string(),
            "aicx-mcp" => "AICX".to_string(),
            "vibecrafted-mcp" => "Vibecrafted".to_string(),
            "" => fallback_source(display_name),
            other => prettify_source(other),
        };
    }
    match raw_name {
        "read_clipboard" | "write_clipboard" => "Clipboard".to_string(),
        "take_screenshot" => "Screenshot".to_string(),
        "transcribe_audio" => "Audio".to_string(),
        _ => fallback_source(display_name),
    }
}

/// Use the already-friendly display name when no source mapping applies, so the
/// line still reads cleanly instead of falling back to a wire token.
fn fallback_source(display_name: &str) -> String {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        "Tool".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Prettify an unknown MCP server segment (`some-server` → `Some server`) so the
/// summary stays readable without leaking the `mcp__` addressing scheme.
fn prettify_source(server: &str) -> String {
    let cleaned: String = server
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect();
    let mut chars = cleaned.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Tool".to_string(),
    }
}

/// Default action verb for a source when a completed call left no result text,
/// so the line reads as an action ("Loctree: scanned code surfaces") rather than
/// a bare "completed".
fn source_action(source: &str) -> &'static str {
    match source {
        "Web search" => "searched the web",
        "Loctree" => "scanned code surfaces",
        "AICX" => "checked intent history",
        "Vibecrafted" => "verified run status",
        "Clipboard" => "read the clipboard",
        "Screenshot" => "captured the screen",
        "Audio" => "transcribed audio",
        _ => "checked",
    }
}

/// Compact result phrase for one source line. Leads with a failure when the
/// source only failed; otherwise leads with the real result (count or richest
/// summary, or the action verb) and appends running/warning counts.
fn source_detail(summary: &SourceSummary) -> String {
    if summary.failed > 0 && summary.completed == 0 && summary.running == 0 {
        return if summary.first_error.is_empty() {
            "failed".to_string()
        } else {
            format!("failed — {}", clamp_suffix(&summary.first_error))
        };
    }

    let mut detail = if let Some(count) = summary.best_count {
        count_phrase(count)
    } else if !summary.best_summary.is_empty() {
        clamp_suffix(&summary.best_summary)
    } else {
        source_action(&summary.source).to_string()
    };
    if summary.running > 0 {
        detail.push_str(&format!(" · {} running", summary.running));
    }
    if summary.failed > 0 {
        let noun = if summary.failed == 1 {
            "warning"
        } else {
            "warnings"
        };
        detail.push_str(&format!(" · {} {noun}", summary.failed));
    }
    detail
}

/// `N result` / `N results`.
fn count_phrase(count: usize) -> String {
    let noun = if count == 1 { "result" } else { "results" };
    format!("{count} {noun}")
}

/// Deterministic single-line verdict: failures dominate; otherwise echo the most
/// informative completed result. `None` when there is nothing notable to report.
fn evidence_key_finding(summaries: &[SourceSummary]) -> Option<String> {
    if let Some(failed) = summaries.iter().find(|s| s.failed > 0) {
        return Some(if failed.first_error.is_empty() {
            format!("{} check failed.", failed.source)
        } else {
            format!(
                "{} check failed: {}",
                failed.source,
                finish_clause(&clamp_suffix(&failed.first_error))
            )
        });
    }

    let richest = summaries
        .iter()
        .filter(|s| !s.best_summary.is_empty() || s.best_count.is_some())
        .max_by_key(|s| s.best_summary.chars().count())?;
    let detail = if richest.best_summary.is_empty() {
        match richest.best_count {
            Some(count) => count_phrase(count),
            None => return None,
        }
    } else {
        clamp_suffix(&richest.best_summary)
    };
    Some(format!("{} — {}", richest.source, finish_clause(&detail)))
}

/// Append a sentence period unless the clause already ends in terminal
/// punctuation (including the ellipsis a clamp may add).
fn finish_clause(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.ends_with(['.', '!', '?', '…']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_from_regression() -> ToolActivityGroup {
        // Mirrors the operator's regression event sequence (interleaved with
        // assistant text chunks, which this module never sees):
        //   ToolExecuting brave -> ToolResult brave (10 results)
        //   ToolExecuting loctree -> ToolResult loctree
        //   ToolExecuting aicx -> ToolResult failed (empty index)
        let mut group = ToolActivityGroup::default();
        group.mark_running("c1", "mcp__brave-search__brave_web_search", "Web search");
        group.mark_result(
            "c1",
            "mcp__brave-search__brave_web_search",
            "Web search",
            "10 results",
            false,
        );
        group.mark_running("c2", "mcp__loctree-mcp__context", "Loctree context");
        group.mark_running("c3", "mcp__aicx-mcp__aicx_intents", "AICX intents");
        group.mark_result(
            "c2",
            "mcp__loctree-mcp__context",
            "Loctree context",
            "",
            false,
        );
        group.mark_result(
            "c3",
            "mcp__aicx-mcp__aicx_intents",
            "AICX intents",
            "empty index",
            true,
        );
        group
    }

    #[test]
    fn groups_all_turn_tools_into_one_block() {
        let group = group_from_regression();
        assert_eq!(
            group.entries.len(),
            3,
            "three calls collapse into one group"
        );
    }

    #[test]
    fn header_surfaces_failures_compactly() {
        let group = group_from_regression();
        // 2 completed + 1 failed → failure visible in the header.
        assert_eq!(group.compact_header(), "Tool activity · 3 calls · 1 failed");
    }

    #[test]
    fn header_reads_completed_when_clean() {
        let mut group = ToolActivityGroup::default();
        group.mark_result(
            "a",
            "mcp__loctree-mcp__context",
            "Loctree context",
            "",
            false,
        );
        group.mark_result("b", "read_clipboard", "Clipboard read", "ok", false);
        assert_eq!(group.compact_header(), "Tool activity · 2 calls completed");
    }

    #[test]
    fn header_reports_running_progress() {
        let mut group = ToolActivityGroup::default();
        group.mark_running("a", "mcp__brave-search__brave_web_search", "Web search");
        assert_eq!(group.compact_header(), "Tool activity · 1 call running");
        group.mark_running("b", "mcp__loctree-mcp__context", "Loctree context");
        group.mark_result(
            "a",
            "mcp__brave-search__brave_web_search",
            "Web search",
            "10 results",
            false,
        );
        assert_eq!(
            group.compact_header(),
            "Tool activity · 2 calls · 1 running"
        );
    }

    #[test]
    fn expanded_render_lists_each_tool_readably() {
        let group = group_from_regression();
        let expected = "Tool activity · 3 calls · 1 failed\n\
             - Web search · completed · 10 results\n\
             - Loctree context · completed\n\
             - AICX intents · failed · empty index";
        assert_eq!(group.render(false), expected);
    }

    #[test]
    fn collapsed_render_is_header_only() {
        let group = group_from_regression();
        assert_eq!(group.render(true), "Tool activity · 3 calls · 1 failed");
    }

    #[test]
    fn raw_wire_names_never_leak_into_rendered_block() {
        let group = group_from_regression();
        let rendered = group.render(false);
        assert!(
            !rendered.contains("mcp__"),
            "raw MCP names must not reach the timeline"
        );
    }

    #[test]
    fn result_for_unseen_start_is_still_recorded() {
        // A ToolResult with no prior ToolExecuting (dropped/coalesced start)
        // must still produce evidence rather than vanish.
        let mut group = ToolActivityGroup::default();
        group.mark_result("z", "take_screenshot", "Screenshot", "saved", false);
        assert_eq!(group.entries.len(), 1);
        assert_eq!(group.entries[0].status, ToolStatus::Completed);
    }

    #[test]
    fn duplicate_start_does_not_duplicate_entry() {
        let mut group = ToolActivityGroup::default();
        group.mark_running("c1", "mcp__loctree-mcp__find", "Loctree occurrences/find");
        group.mark_running("c1", "mcp__loctree-mcp__find", "Loctree occurrences/find");
        assert_eq!(group.entries.len(), 1);
    }

    #[test]
    fn long_failure_suffix_is_clamped_not_a_stack_dump() {
        let mut group = ToolActivityGroup::default();
        let huge = "panic at line 42: ".repeat(40);
        group.mark_result("c1", "mcp__x__y", "Tool", &huge, true);
        let line = group.entries[0].line();
        assert!(
            line.chars().count() < 120,
            "failed line must stay compact: {line}"
        );
        assert!(line.ends_with('…'), "clamped suffix is elided");
    }

    // ---- Evidence Summary (default operator-facing block) ----------------

    #[test]
    fn evidence_summary_has_semantic_heading_and_one_line_per_source() {
        let group = group_from_regression();
        let expected = "What I checked · 3 tools · 1 warning\n\
             - Web search: 10 results.\n\
             - Loctree: scanned code surfaces.\n\
             - AICX: failed — empty index.\n\
             Key finding: AICX check failed: empty index.";
        assert_eq!(group.evidence_summary(), expected);
    }

    #[test]
    fn evidence_summary_collapses_repeat_calls_to_one_source() {
        // Two Loctree calls in a turn must read as ONE "Loctree" source, not two
        // rows — the summary answers "which sources", not "how many wire calls".
        let mut group = ToolActivityGroup::default();
        group.mark_result(
            "c1",
            "mcp__loctree-mcp__context",
            "Loctree context",
            "scanned voice_chat",
            false,
        );
        group.mark_result(
            "c2",
            "mcp__loctree-mcp__find",
            "Loctree occurrences/find",
            "clipboard 230 occurrences, schowek 4 occurrences",
            false,
        );
        let summary = group.evidence_summary();
        assert_eq!(
            summary,
            "What I checked · 1 tool\n\
             - Loctree: clipboard 230 occurrences, schowek 4 occurrences.\n\
             Key finding: Loctree — clipboard 230 occurrences, schowek 4 occurrences."
        );
        assert_eq!(
            summary.matches("- Loctree:").count(),
            1,
            "two Loctree calls fold into one source line"
        );
    }

    #[test]
    fn evidence_summary_regression_scenario_three_distinct_sources() {
        // The operator's suggested regression event sequence.
        let mut group = ToolActivityGroup::default();
        group.mark_running("c1", "mcp__brave-search__brave_web_search", "Web search");
        group.mark_result(
            "c1",
            "mcp__brave-search__brave_web_search",
            "Web search",
            "10 results",
            false,
        );
        group.mark_running("c2", "mcp__loctree-mcp__find", "Loctree occurrences/find");
        group.mark_result(
            "c2",
            "mcp__loctree-mcp__find",
            "Loctree occurrences/find",
            "clipboard 230 occurrences, schowek 4 occurrences",
            false,
        );
        group.mark_running(
            "c3",
            "mcp__vibecrafted-mcp__vc_run_observe",
            "Vibecrafted observe",
        );
        group.mark_result(
            "c3",
            "mcp__vibecrafted-mcp__vc_run_observe",
            "Vibecrafted observe",
            "run completed",
            false,
        );
        assert_eq!(
            group.evidence_summary(),
            "What I checked · 3 tools\n\
             - Web search: 10 results.\n\
             - Loctree: clipboard 230 occurrences, schowek 4 occurrences.\n\
             - Vibecrafted: run completed.\n\
             Key finding: Loctree — clipboard 230 occurrences, schowek 4 occurrences."
        );
    }

    #[test]
    fn evidence_summary_singular_nouns_for_one_tool_one_warning() {
        let mut group = ToolActivityGroup::default();
        group.mark_result(
            "c1",
            "mcp__aicx-mcp__aicx_intents",
            "AICX intents",
            "boom",
            true,
        );
        assert_eq!(
            group.evidence_summary(),
            "What I checked · 1 tool · 1 warning\n\
             - AICX: failed — boom.\n\
             Key finding: AICX check failed: boom."
        );
    }

    #[test]
    fn evidence_summary_never_leaks_raw_wire_names() {
        let group = group_from_regression();
        assert!(
            !group.evidence_summary().contains("mcp__"),
            "raw MCP names must never reach the evidence summary"
        );
    }

    #[test]
    fn evidence_summary_failed_line_is_clamped_not_a_stack_dump() {
        let mut group = ToolActivityGroup::default();
        let huge = "panic at line 42: ".repeat(40);
        group.mark_result("c1", "mcp__x__y", "Tool", &huge, true);
        let summary = group.evidence_summary();
        for line in summary.lines() {
            assert!(
                line.chars().count() < 160,
                "no line may become a stack dump: {line}"
            );
        }
    }

    #[test]
    fn evidence_summary_unknown_server_prettifies_without_wire_scheme() {
        let mut group = ToolActivityGroup::default();
        group.mark_result(
            "c1",
            "mcp__some-other-server__do_thing",
            "Do thing · Some other server",
            "ok",
            false,
        );
        let summary = group.evidence_summary();
        assert!(summary.contains("- Some other server: ok."));
        assert!(!summary.contains("mcp__"));
    }
}
