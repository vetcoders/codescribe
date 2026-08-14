//! Agent-facing ground truth about Responses-style and streaming AI APIs,
//! appended to the system prompt. Every claim here was measured in the field
//! (2026-08-12..14: the pair-400, the promptless-chain leak on build 661, the
//! key-swap `previous_response_not_found`, and the full hours-later recall of
//! a stored chain). Prompt-layer only — the agent must answer questions about
//! these mechanics from facts, not with generic clarification menus.

/// A concise Responses/streaming primer for the agent system prompt. Kept
/// tight on purpose: prompt space is a scarce resource, so this section
/// states the contract facts, the app's transcription shape, and the
/// answer-first rule — nothing else.
pub fn responses_api_prompt_section() -> String {
    "RESPONSES & STREAMING AI APIS — GROUND TRUTH\n\
     Codescribe speaks the OpenAI Responses API (`/v1/responses`), never \
     legacy chat/completions. Requests carry `input` items shaped \
     `{\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":...}]}` \
     (assistant history rides as `output_text`).\n\
     Conversation state: `previous_response_id` chains turns server-side; a \
     stored response id IS durable conversation memory and can be resumed \
     hours later. Three measured sharp edges: (1) sending `instructions` \
     together with `previous_response_id` is HTTP 400 on OpenAI; (2) \
     instructions are NOT carried across chained turns — a chained turn must \
     re-carry the system prompt as a leading `developer` input item; (3) \
     response ids are key/org-scoped — after a key rotation the old id \
     answers `previous_response_not_found`, so drop it and continue \
     unchained. Some Responses backends (LibraxisAI) also mint a \
     `response_id` for STT transcriptions (`resp_stt_*`) that LLM turns can \
     chain from — voice joining the conversation as a chain link.\n\
     Streaming is SSE: `response.created` -> `response.in_progress` -> \
     `response.output_item.added` -> `response.output_text.delta`... -> \
     `response.completed` (the completed event carries the full output and \
     the response id). Non-streaming is one JSON body of the same shape.\n\
     This app's transcription is layered: Apple SFSpeech live partials/finals \
     form the canvas, Whisper re-transcribes windowed tails, the lexicon is a \
     post-pass, LLM formatting is a separate lane — and RAW is append-only, \
     never full-replaced.\n\
     ANSWER-FIRST RULE: when a spoken request is rough, partial, or \
     frustrated, extract the actionable intent and act or answer \
     substantively from these facts and the codebase. Ask at most ONE \
     clarifying question, and only when genuinely blocked — never reply with \
     numbered option menus or requirement questionnaires."
        .to_string()
}

/// Pins the load-bearing anchors this section must keep through future edits.
#[cfg(test)]
mod tests {
    use super::*;

    /// The primer must keep the measured contract facts and the answer-first rule.
    #[test]
    fn api_truth_section_carries_the_load_bearing_anchors() {
        let section = responses_api_prompt_section();
        assert!(section.starts_with("RESPONSES & STREAMING AI APIS"));
        for anchor in [
            "/v1/responses",
            "input_text",
            "previous_response_id",
            "HTTP 400",
            "NOT carried across chained turns",
            "developer",
            "previous_response_not_found",
            "resp_stt_",
            "response.output_text.delta",
            "response.completed",
            "append-only",
            "ANSWER-FIRST RULE",
            "numbered option menus",
        ] {
            assert!(
                section.contains(anchor),
                "api-truth section missing anchor: {anchor}"
            );
        }
    }
}
