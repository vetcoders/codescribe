//! Host admission for Max, using the same concrete provider clients and the
//! existing Agent tool registry. This owns settings selection, not a second
//! conversation history or model/tool loop.

use std::sync::Arc;
use anyhow::{Context, Result, ensure};
use tokio::sync::{mpsc, oneshot};
use codescribe_core::agent::{AgentSession, ImageAttachment, StreamOptions,
    ThreadDeliveryGateway, ToolApprovalHandler, ToolRegistry};
use codescribe_core::agent::consultation::{ConsultationAnswer, ConsultationEvents,
    ConsultationRuntime, ConsultationTurn};
use codescribe_core::config::{FormattingPolicy, RuntimeLlmLaneKind,
    RuntimeSettingsSnapshot};

/// Selected consultation with request-scoped settings admission.
/// The controller owns this across captures and replaces it only on an
/// explicit new-consultation action, not on focus or recording end.
pub struct MaxConsultation {
    runtime: ConsultationRuntime,
}

impl MaxConsultation {
    /// The host supplies its existing permission-configured registry and
    /// approval broker. No approval broker means the Agent gate refuses tools
    /// that require a decision; absence never turns into implicit permission.
    pub fn start(
        id: String,
        settings: &RuntimeSettingsSnapshot,
        tools: Arc<ToolRegistry>,
        approval: Option<ToolApprovalHandler>,
        gateway: ThreadDeliveryGateway,
        events: ConsultationEvents,
        install_lease_path: std::path::PathBuf,
    ) -> Result<Self> {
        let provider = super::create_provider_for_lane(settings, RuntimeLlmLaneKind::Formatting)?;
        let (tx, rx) = mpsc::channel(64);
        let mut session = AgentSession::new(provider, tools, tx);
        if let Some(approval) = approval {
            session = session.with_tool_approval(id.clone(), approval);
        }
        let runtime = ConsultationRuntime::start(id, session, rx, gateway, events, install_lease_path)?;
        Ok(Self { runtime })
    }

    pub fn id(&self) -> &str { self.runtime.id() }

    /// All request knobs and provenance come from one immutable formatting
    /// snapshot. A chat provider selection cannot leak into this request.
    pub fn enqueue(
        &self,
        turn_id: String,
        text: String,
        attachments: Vec<ImageAttachment>,
        settings: &RuntimeSettingsSnapshot,
    ) -> Result<oneshot::Receiver<Result<ConsultationAnswer>>> {
        ensure!(settings.formatting_policy() == FormattingPolicy::Max, "Max consultation is not selected");
        let options = stream_options(settings)?;
        // Construct from this turn's seal. A cached "last enqueued generation"
        // would be wrong if that entry were later rejected as a duplicate.
        // Local history preserves continuity; provider response chains are
        // deliberately reset when this request reaches the owner.
        let replacement_provider = Some(super::create_provider_for_lane(settings, RuntimeLlmLaneKind::Formatting)?);
        let receipt = self.runtime.enqueue(ConsultationTurn {
            id: turn_id,
            policy: settings.formatting_policy(),
            text,
            attachments,
            options,
            provider_name: settings.llm_lanes().formatting().provider().as_str().to_string(),
            replacement_provider,
        })?;
        Ok(receipt)
    }
}

#[async_trait::async_trait]
impl codescribe_core::ai_formatting::FormattingAgent for MaxConsultation {
    async fn execute(&self, turn_id: &str, text: &str, settings: &RuntimeSettingsSnapshot) -> Result<String> {
        let answer = self.enqueue(turn_id.to_string(), text.to_string(), Vec::new(), settings)?
            .await.context("Max consultation owner stopped before replying")??;
        Ok(answer.text)
    }
}

fn stream_options(settings: &RuntimeSettingsSnapshot) -> Result<StreamOptions> {
    ensure!(settings.formatting_policy() == FormattingPolicy::Max, "Max consultation is not selected");
    let lane = settings.llm_lanes().formatting();
    ensure!(lane.request_available(), "{}", lane.unavailable_reason().unwrap_or("formatting provider unavailable"));
    let prompt = settings.ai_execution().formatter().formatting_prompt()
        .context("Max consultation prompt unavailable")?;
    Ok(StreamOptions {
        model: lane.model().to_string(),
        system_prompt: Some(prompt.composed_content().to_string()),
        max_tokens: None,
        temperature: None,
        reset_chain: false,
    })
}
