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
}
