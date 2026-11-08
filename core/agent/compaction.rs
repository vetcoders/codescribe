//! Context compaction for one agent session: budget, estimate and pruning.
//!
//! An agent turn may run hundreds of tool rounds, and every round replays the
//! whole history. Without a budget the session grows until the provider
//! refuses the request and the turn dies. This module keeps the request inside
//! the model's window in two stages:
//!
//! 1. **Overflow detection.** Before a request the session estimates what it
//!    will carry and compares it with the usable window (the model window minus
//!    an output reserve). It also recognises the provider's own refusal of an
//!    overlong prompt, so a wrong estimate still ends in recovery.
//! 2. **Tool-result pruning.** Older tool results move out of the history into
//!    the content-addressed tool-output store, and the history keeps the same
//!    one-line reference that oversized outputs already use. Nothing is
//!    destroyed: the agent can read the stored output back.
//!
//! Pruning never touches what the model has not yet read (results after the
//! last assistant message), and never touches anything before a signed
//! response of the current turn: Google signs each response over a digest of
//! the whole preceding history, and Gemini rejects a current-turn call whose
//! signature no longer matches. Signed responses of earlier turns whose
//! history changed lose their signature and replay as ordinary content, which
//! Gemini accepts for turns before the current one.
//!
//! Lineage (both MIT): opencode `session/overflow.ts` (usable window, 20k
//! reserve), opencode `session/compaction.ts` prune (40k protect window, newest
//! first), and the overflow phrases of opencode `llm/src/provider-error.ts`.
//! oh-my-pi `compaction/pruning.ts` contributed the cache rule: prune rarely
//! and in one sweep, because every rewritten result invalidates the provider's
//! prompt cache from that point on.

use anyhow::Result;

use super::tool_output::ToolOutputStore;
use super::{ContentBlock, Message, Role, ToolDefinition};

/// Overrides the context window for every model, in tokens.
pub const CONTEXT_WINDOW_ENV: &str = "CODESCRIBE_AGENT_CONTEXT_WINDOW";

/// Output reserve kept free of history (opencode `COMPACTION_BUFFER`).
pub(crate) const COMPACTION_BUFFER_TOKENS: u64 = 20_000;

/// Newest tool output, in tokens, that a normal prune keeps inline.
pub(crate) const PRUNE_PROTECT_TOKENS: u64 = 40_000;

/// A result smaller than this saves less than it costs in prompt-cache churn.
pub(crate) const PRUNE_MIN_RESULT_BYTES: usize = 1_024;

/// Window for a model this table does not recognise.
pub(crate) const FALLBACK_CONTEXT_WINDOW: u64 = 128_000;

/// Fixed charge for one image block; a full-size image costs about this much.
const IMAGE_TOKENS: u64 = 1_600;

/// Role and framing tokens charged per message.
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;

/// Bytes per token. Deliberately low: code and Polish text tokenize denser
/// than English prose, and an overestimate only prunes a little early.
const BYTES_PER_TOKEN: u64 = 3;

/// Usable request size for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    /// Model context window in tokens.
    pub window: u64,
    /// Tokens kept free for the model's answer.
    pub reserved: u64,
}

impl ContextBudget {
    /// Budget for `window`, reserving the smaller of the requested output
    /// ceiling and [`COMPACTION_BUFFER_TOKENS`].
    pub fn new(window: u64, max_output_tokens: Option<u32>) -> Self {
        let reserved = max_output_tokens
            .map(u64::from)
            .unwrap_or(COMPACTION_BUFFER_TOKENS)
            .min(COMPACTION_BUFFER_TOKENS);
        Self { window, reserved }
    }

    /// Tokens the request itself may carry.
    pub fn usable(&self) -> u64 {
        self.window.saturating_sub(self.reserved)
    }

    /// Whether a request of `tokens` leaves no room for the answer.
    pub fn overflows(&self, tokens: u64) -> bool {
        tokens >= self.usable()
    }
}

/// Context window of a model family, in input tokens.
///
/// Conservative defaults for families whose windows are published; an unknown
/// model gets [`FALLBACK_CONTEXT_WINDOW`]. The provider's own refusal
/// calibrates the session when a default is too generous, and
/// [`CONTEXT_WINDOW_ENV`] overrides the table.
pub fn default_context_window(model: &str) -> u64 {
    let model = model.trim().to_ascii_lowercase();
    let family = model.rsplit('/').next().unwrap_or(&model);
    if family.contains("claude") {
        200_000
    } else if family.contains("gemini") || family.starts_with("gpt-4.1") {
        1_000_000
    } else if family.starts_with("gpt-5") || family.starts_with("gpt-6") {
        // 400k total with a 128k output ceiling leaves 272k of input.
        272_000
    } else if family.starts_with("o1") || family.starts_with("o3") || family.starts_with("o4") {
        200_000
    } else if family.starts_with("gpt-4") {
        128_000
    } else {
        FALLBACK_CONTEXT_WINDOW
    }
}

/// [`CONTEXT_WINDOW_ENV`] when it holds a positive integer.
pub fn context_window_from_env() -> Option<u64> {
    std::env::var(CONTEXT_WINDOW_ENV)
        .ok()
        .and_then(|raw| raw.trim().replace('_', "").parse::<u64>().ok())
        .filter(|tokens| *tokens > 0)
}

/// Estimated tokens of a text.
pub fn estimate_text_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(BYTES_PER_TOKEN)
}

/// Estimated tokens of one content block, nested tool results included.
pub fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(text) => estimate_text_tokens(text),
        ContentBlock::Image { .. } | ContentBlock::ImageAsset(_) => IMAGE_TOKENS,
        ContentBlock::ToolUse { id, name, input } => {
            estimate_text_tokens(id) + estimate_text_tokens(name) + json_tokens(input)
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            estimate_text_tokens(tool_use_id)
                + content.iter().map(estimate_block_tokens).sum::<u64>()
        }
    }
}

/// Estimated tokens of one message.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    MESSAGE_OVERHEAD_TOKENS
        + message
            .content
            .iter()
            .map(estimate_block_tokens)
            .sum::<u64>()
}

/// Estimated tokens of a whole request: system prompt, tool declarations and
/// history.
pub fn estimate_request_tokens(
    messages: &[Message],
    system_prompt: Option<&str>,
    tools: &[ToolDefinition],
) -> u64 {
    let system = system_prompt.map_or(0, estimate_text_tokens);
    let tools = tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(&tool.name)
                + estimate_text_tokens(&tool.description)
                + json_tokens(&tool.input_schema)
        })
        .sum::<u64>();
    system + tools + messages.iter().map(estimate_message_tokens).sum::<u64>()
}

fn json_tokens(value: &serde_json::Value) -> u64 {
    serde_json::to_string(value).map_or(0, |text| estimate_text_tokens(&text))
}

/// Phrases providers use to refuse a prompt longer than the model window.
///
/// Matched on the lowercased error chain. Taken from opencode
/// `llm/src/provider-error.ts`, reduced to phrases that cannot describe a rate
/// or quota limit, plus the on-device formatter's own wording.
const OVERFLOW_PHRASES: &[&str] = &[
    "prompt is too long",
    "prompt too long",
    "request_too_large",
    "input is too long for requested model",
    "exceeds the context window",
    "maximum context length",
    "input token count",
    "tokens in request more than max tokens allowed",
    "maximum prompt length is",
    "reduce the length of the messages",
    "maximum allowed input length",
    "longer than the model's context length",
    "longer than the models context length",
    "exceeds the available context size",
    "greater than the context length",
    "context window exceeds limit",
    "exceeded model token limit",
    "context_length_exceeded",
    "context length exceeded",
    "context window exceeded",
    "request entity too large",
    "context length is only",
    "too large for model with",
    "but the configured context size is",
    "model_context_window_exceeded",
    "too many tokens",
    "token limit exceeded",
];

/// Whether a provider error says the prompt did not fit the model window.
pub fn is_context_overflow_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    OVERFLOW_PHRASES.iter().any(|phrase| error.contains(phrase))
}

/// What one prune pass changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PruneOutcome {
    /// Tool results moved into the store.
    pub results: usize,
    /// Estimated tokens the history no longer carries.
    pub tokens_saved: u64,
    /// Earlier-turn responses that now replay without their signature.
    pub signatures_dropped: usize,
}

/// A user message that opens a turn, as opposed to one carrying tool results.
fn opens_turn(message: &Message) -> bool {
    message.role == Role::User
        && !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// Whether a message carries a provider's signed wire state.
fn is_signed(message: &Message) -> bool {
    message
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.as_stored().get("provider_wire").is_some())
}

/// Text bytes of a tool result that are still inline.
fn inline_text_bytes(content: &[ContentBlock]) -> usize {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) if !ToolOutputStore::is_reference(text) => Some(text.len()),
            _ => None,
        })
        .sum()
}

/// Move older tool results into `store`, newest `protect_tokens` excepted.
///
/// Walks tool results from newest to oldest. Results the model has not read
/// yet stay, and so does everything at or before the latest signed response of
/// the current turn. The newest `protect_tokens` of tool output stay inline;
/// every older result above [`PRUNE_MIN_RESULT_BYTES`] becomes a stored
/// reference. When a result of an earlier turn changes, the signed responses
/// that follow it in earlier turns drop their `provider_wire` metadata.
pub(crate) fn prune_tool_results(
    messages: &mut [Message],
    store: &ToolOutputStore,
    protect_tokens: u64,
) -> Result<PruneOutcome> {
    let mut outcome = PruneOutcome::default();
    let Some(last_assistant) = messages.iter().rposition(|m| m.role == Role::Assistant) else {
        return Ok(outcome);
    };
    let turn_start = messages.iter().rposition(opens_turn).unwrap_or(0);
    // A signed response of the current turn hashes everything before it.
    let fence = messages
        .iter()
        .rposition(is_signed)
        .filter(|signed| *signed >= turn_start);

    let mut protected = 0u64;
    let mut victims = Vec::new();
    for (index, message) in messages.iter().enumerate().rev() {
        for (position, block) in message.content.iter().enumerate().rev() {
            let ContentBlock::ToolResult { content, .. } = block else {
                continue;
            };
            let tokens = content.iter().map(estimate_block_tokens).sum::<u64>();
            protected = protected.saturating_add(tokens);
            if index > last_assistant
                || fence.is_some_and(|fence| index <= fence)
                || protected <= protect_tokens
                || inline_text_bytes(content) < PRUNE_MIN_RESULT_BYTES
            {
                continue;
            }
            victims.push((index, position));
        }
    }

    let mut first_changed = None;
    let mut failure = None;
    for (index, position) in victims {
        let ContentBlock::ToolResult { content, .. } = &mut messages[index].content[position]
        else {
            continue;
        };
        let before = content.iter().map(estimate_block_tokens).sum::<u64>();
        match store.store_content(content) {
            Ok(true) => {}
            Ok(false) => continue,
            // The result stays inline; what already moved is settled below.
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
        let after = content.iter().map(estimate_block_tokens).sum::<u64>();
        outcome.results += 1;
        outcome.tokens_saved += before.saturating_sub(after);
        first_changed = Some(first_changed.map_or(index, |first: usize| first.min(index)));
    }

    if let Some(first) = first_changed {
        for message in &mut messages[first..turn_start.max(first)] {
            if let Some(metadata) = &message.metadata
                && metadata.as_stored().get("provider_wire").is_some()
            {
                message.metadata = metadata.without_provider_wire();
                outcome.signatures_dropped += 1;
            }
        }
    }
    match failure {
        Some(error) if outcome.results == 0 => Err(error),
        Some(error) => {
            tracing::warn!(%error, results = outcome.results, "Tool-result pruning stopped early");
            Ok(outcome)
        }
        None => Ok(outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{MessageMetadata, ProviderResponseMetadata, ProviderWireMessage};

    fn store() -> (tempfile::TempDir, ToolOutputStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ToolOutputStore::new_in(dir.path().join("tool_outputs"), 64 * 1024);
        (dir, store)
    }

    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.into())])
    }

    fn call(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": format!("{id}.rs")}),
            }],
        )
    }

    fn result(id: &str, bytes: usize) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: vec![ContentBlock::Text(format!("{id}:{}", "x".repeat(bytes)))],
                is_error: false,
            }],
        )
    }

    fn answer(text: &str) -> Message {
        Message::new(Role::Assistant, vec![ContentBlock::Text(text.into())])
    }

    fn signed(mut message: Message) -> Message {
        let wire = ProviderWireMessage {
            version: 1,
            occurrence_id: uuid::Uuid::new_v4().to_string(),
            segment_index: 0,
            segment_count: 1,
            response: Some(ProviderResponseMetadata {
                wire: "google-generate-content".into(),
                provider_id: "a".repeat(64),
                model: "gemini-test".into(),
                context_digest: "b".repeat(64),
                payload: serde_json::json!({}),
            }),
        };
        message.metadata = Some(MessageMetadata::for_provider_wire(wire).expect("valid wire"));
        message
    }

    fn inline(message: &Message) -> bool {
        message.content.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { content, .. }
                if inline_text_bytes(content) > 0)
        })
    }

    fn stored_text(message: &Message) -> String {
        match &message.content[0] {
            ContentBlock::ToolResult { content, .. } => match &content[0] {
                ContentBlock::Text(text) => text.clone(),
                other => panic!("expected text, got {other:?}"),
            },
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    #[test]
    fn budget_reserves_the_smaller_of_output_ceiling_and_buffer() {
        assert_eq!(ContextBudget::new(200_000, None).usable(), 180_000);
        assert_eq!(ContextBudget::new(200_000, Some(4_096)).usable(), 195_904);
        assert_eq!(ContextBudget::new(200_000, Some(64_000)).usable(), 180_000);
        assert!(ContextBudget::new(200_000, None).overflows(180_000));
        assert!(!ContextBudget::new(200_000, None).overflows(179_999));
        assert_eq!(ContextBudget::new(10_000, None).usable(), 0);
    }

    #[test]
    fn model_families_map_to_their_windows() {
        assert_eq!(default_context_window("claude-opus-4-8"), 200_000);
        assert_eq!(default_context_window("anthropic/claude-sonnet-5"), 200_000);
        assert_eq!(default_context_window("gpt-5.5"), 272_000);
        assert_eq!(default_context_window("gpt-6-sol"), 272_000);
        assert_eq!(default_context_window("gpt-4.1"), 1_000_000);
        assert_eq!(default_context_window("gpt-4o-mini"), 128_000);
        assert_eq!(default_context_window("o4-mini"), 200_000);
        assert_eq!(default_context_window("gemini-3-pro"), 1_000_000);
        assert_eq!(default_context_window("mtplx-9b"), FALLBACK_CONTEXT_WINDOW);
        assert_eq!(default_context_window(""), FALLBACK_CONTEXT_WINDOW);
    }

    #[test]
    fn estimate_counts_text_images_tool_calls_and_declarations() {
        let message = Message::new(
            Role::User,
            vec![
                ContentBlock::Text("x".repeat(300)),
                ContentBlock::Image {
                    data: vec![0; 10],
                    media_type: "image/png".into(),
                },
            ],
        );
        assert_eq!(estimate_message_tokens(&message), 4 + 100 + IMAGE_TOKENS);
        let tool = ToolDefinition {
            name: "read_file".into(),
            description: "d".repeat(30),
            input_schema: serde_json::json!({}),
        };
        let total = estimate_request_tokens(
            std::slice::from_ref(&message),
            Some(&"s".repeat(30)),
            &[tool],
        );
        assert_eq!(total, 10 + 3 + 10 + 1 + estimate_message_tokens(&message));
    }

    #[test]
    fn overflow_refusals_are_recognised_and_rate_limits_are_not() {
        for refusal in [
            "Anthropic 400: prompt is too long: 215000 tokens > 200000 maximum",
            "OpenAI error code=context_length_exceeded: Your input exceeds the context window of this model",
            "This model's maximum context length is 128000 tokens",
            "llama.cpp: the request exceeds the available context size, try increasing it",
            "Google 400: The input token count (1048577) exceeds the maximum number of tokens allowed",
            "HTTP 413 Request Entity Too Large",
            "on-device format failed: context window exceeded: 4096",
        ] {
            assert!(is_context_overflow_error(refusal), "{refusal}");
        }
        for other in [
            "429 Too Many Requests: rate limit exceeded",
            "401 invalid x-api-key",
            "Rate limit reached for gpt-5.5 on tokens per min (TPM): Limit 30000",
            "connection reset by peer",
        ] {
            assert!(!is_context_overflow_error(other), "{other}");
        }
    }

    #[test]
    fn pruning_keeps_the_newest_window_and_stores_older_results_by_reference() {
        let (_dir, store) = store();
        // Four seen results of ~10k tokens each, newest last, then an answer.
        let mut messages = vec![user("read four files")];
        for id in ["a", "b", "c", "d"] {
            messages.push(call(id));
            messages.push(result(id, 30_000));
        }
        messages.push(answer("done"));
        let before = estimate_request_tokens(&messages, None, &[]);

        let outcome = prune_tool_results(&mut messages, &store, 25_000).expect("prune");

        assert_eq!(outcome.results, 2, "a and b fall outside the 25k window");
        assert!(!inline(&messages[2]) && !inline(&messages[4]));
        assert!(inline(&messages[6]) && inline(&messages[8]));
        assert_eq!(
            before - estimate_request_tokens(&messages, None, &[]),
            outcome.tokens_saved
        );
        let reference = stored_text(&messages[2]);
        assert!(ToolOutputStore::is_reference(&reference), "{reference}");
        let path = reference
            .trim_start_matches("[tool output stored: ")
            .rsplit_once(" (")
            .expect("reference names a path")
            .0;
        assert_eq!(
            std::fs::read_to_string(path).expect("stored output"),
            format!("a:{}", "x".repeat(30_000)),
            "nothing is destroyed: the full output can be read back"
        );
    }

    #[test]
    fn results_the_model_has_not_read_are_never_pruned() {
        let (_dir, store) = store();
        let mut messages = vec![user("go"), call("a"), result("a", 30_000), call("b")];
        messages.push(result("b", 30_000));

        let outcome = prune_tool_results(&mut messages, &store, 0).expect("prune");

        assert_eq!(outcome.results, 1, "only the result read by the next call");
        assert!(!inline(&messages[2]));
        assert!(
            inline(&messages[4]),
            "b is unread: no assistant message after it"
        );
    }

    #[test]
    fn small_and_already_stored_results_stay_and_a_second_pass_is_a_no_op() {
        let (_dir, store) = store();
        let mut messages = vec![
            user("go"),
            call("small"),
            result("small", 100),
            call("big"),
            result("big", 30_000),
            answer("done"),
        ];
        let first = prune_tool_results(&mut messages, &store, 0).expect("first pass");
        assert_eq!(first.results, 1);
        assert!(inline(&messages[2]), "below the minimum size");
        let snapshot = messages.clone();
        let second = prune_tool_results(&mut messages, &store, 0).expect("second pass");
        assert_eq!(second, PruneOutcome::default());
        assert_eq!(
            messages, snapshot,
            "a pruned history is stable for the cache"
        );
    }

    #[test]
    fn nothing_before_a_signed_response_of_the_current_turn_is_pruned() {
        let (_dir, store) = store();
        let mut messages = vec![
            user("earlier turn"),
            call("old"),
            result("old", 30_000),
            answer("earlier answer"),
            user("current turn"),
            call("a"),
            result("a", 30_000),
            signed(call("b")),
            result("b", 30_000),
            call("c"),
            result("c", 30_000),
            answer("current answer"),
        ];
        let outcome = prune_tool_results(&mut messages, &store, 0).expect("prune");
        assert_eq!(outcome.results, 2, "only b and c lie after the signed call");
        assert!(
            inline(&messages[2]),
            "earlier turn sits under the signature"
        );
        assert!(inline(&messages[6]), "a sits under the signature");
        assert!(!inline(&messages[8]) && !inline(&messages[10]));
        assert!(
            is_signed(&messages[7]),
            "the current-turn signature is kept"
        );
        assert_eq!(outcome.signatures_dropped, 0);
    }

    #[test]
    fn earlier_turn_signatures_after_a_pruned_result_replay_unsigned() {
        let (_dir, store) = store();
        let mut messages = vec![
            signed(answer("first answer, signed before anything changed")),
            user("earlier turn"),
            call("old"),
            result("old", 30_000),
            signed(answer("earlier answer")),
            user("current turn"),
            call("a"),
            result("a", 100),
            answer("current answer"),
        ];
        let outcome = prune_tool_results(&mut messages, &store, 0).expect("prune");
        assert_eq!(outcome.results, 1);
        assert!(!inline(&messages[3]));
        assert_eq!(outcome.signatures_dropped, 1);
        assert!(
            is_signed(&messages[0]),
            "a signature before the change still matches"
        );
        assert!(
            !is_signed(&messages[4]),
            "its digest covered the pruned result"
        );
    }

    #[test]
    fn a_store_that_cannot_write_leaves_every_result_inline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"file").expect("blocker file");
        let store = ToolOutputStore::new_in(blocker.join("tool_outputs"), 64 * 1024);
        let mut messages = vec![user("go"), call("a"), result("a", 30_000), answer("done")];
        let snapshot = messages.clone();

        let error = prune_tool_results(&mut messages, &store, 0).expect_err("store fails");

        assert!(format!("{error:#}").contains("tool-output"), "{error:#}");
        assert_eq!(
            messages, snapshot,
            "pruning never trades output for a notice"
        );
    }

    #[test]
    fn dropping_a_signature_keeps_other_metadata_namespaces() {
        let message = signed(answer("x"));
        let mut stored = message.metadata.unwrap().as_stored().clone();
        stored["delivery"] = serde_json::json!({"receipt": "r-1"});
        let metadata = MessageMetadata::from_stored(stored).without_provider_wire();
        assert_eq!(
            metadata.map(|m| m.as_stored().clone()),
            Some(serde_json::json!({"delivery": {"receipt": "r-1"}}))
        );
        assert!(
            signed(answer("y"))
                .metadata
                .unwrap()
                .without_provider_wire()
                .is_none()
        );
    }
}
