//! Project ACP session updates onto the provider event vocabulary.

use serde_json::Value;

use crate::agent::AgentEvent;

/// Per-turn text accumulator; thought chunks never enter the answer document.
#[derive(Default)]
pub struct AcpEventMapper {
    text: String,
}

impl AcpEventMapper {
    /// Accept either a session/update params object or its update object.
    pub fn update(&mut self, params: &Value) -> Option<AgentEvent> {
        let update = params.get("update").unwrap_or(params);
        let content = update.get("content")?;
        if content.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let text = content.get("text")?.as_str()?.to_string();
        match update.get("sessionUpdate")?.as_str()? {
            "agent_message_chunk" => {
                self.text.push_str(&text);
                Some(AgentEvent::TextDelta(text))
            }
            "agent_thought_chunk" => Some(AgentEvent::ReasoningDelta(text)),
            _ => None,
        }
    }

    /// Only a successful prompt response may advance session continuity.
    pub fn finish(&mut self, session_id: &str) -> Vec<AgentEvent> {
        vec![
            AgentEvent::TextDone(std::mem::take(&mut self.text)),
            AgentEvent::ResponseDone {
                response_id: Some(session_id.to_string()),
                clean: true,
            },
        ]
    }

    /// An error never commits partial text or advances the session id.
    pub fn fail(&mut self, message: String) -> Vec<AgentEvent> {
        self.text.clear();
        vec![
            AgentEvent::ResponseDone {
                response_id: None,
                clean: false,
            },
            AgentEvent::Error(message),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acp_thoughts_never_enter_answer_text() {
        let mut mapper = AcpEventMapper::default();
        assert_eq!(mapper.update(&json!({"update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private thought"}}})), Some(AgentEvent::ReasoningDelta("private thought".into())));
        assert_eq!(mapper.update(&json!({"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"answer"}}})), Some(AgentEvent::TextDelta("answer".into())));
        assert_eq!(
            mapper.finish("s1"),
            vec![
                AgentEvent::TextDone("answer".into()),
                AgentEvent::ResponseDone {
                    response_id: Some("s1".into()),
                    clean: true
                }
            ]
        );
        assert_eq!(
            mapper.fail("cancelled".into()),
            vec![
                AgentEvent::ResponseDone {
                    response_id: None,
                    clean: false
                },
                AgentEvent::Error("cancelled".into())
            ]
        );
    }
}
