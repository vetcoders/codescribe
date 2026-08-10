//! Agent subsystem: the tool-using conversational agent behind Codescribe's
//! assistive surface.
//!
//! This module is a facade. Each submodule owns one concern of a turn, and the
//! `pub use` block below re-exports the types callers actually name, so
//! `codescribe_core::agent::X` stays stable when a type moves between files.
//!
//! A turn flows through them in order: [`session`] drives the loop,
//! [`provider`] streams the model's [`event`]s, [`registry`] resolves each
//! requested tool, [`permissions`] — with [`secret_paths`] and [`tool_grants`]
//! — decides allow / ask / deny, and [`thread_delivery`] persists the finished
//! turn into [`thread_store`].

/// On-disk store for image attachments referenced from conversation history.
pub mod assets;
/// Provider-neutral capability broker for canonical ops (`fs.read`, `repo.status`).
pub mod capabilities;
/// Streaming events: provider-level [`AgentEvent`] and UI-level [`AgentUiEvent`].
pub mod event;
/// Tool permission policy — the allow / ask / deny gateway.
pub mod permissions;
/// The [`AgentProvider`] trait and its per-request [`StreamOptions`].
pub mod provider;
/// Tool registry: definitions, execution policy, risk tiers, approval requests.
pub mod registry;
/// Durable run-monitor harness for external Vibecrafted runs.
pub mod run_monitor;
/// Secret-path escalation that stops a read-level `Allow` covering credentials.
pub mod secret_paths;
/// The turn loop: streaming, tool dispatch, approvals, iteration limits.
pub mod session;
/// Canonical persistence boundary for completed agent turns.
pub mod thread_delivery;
/// Render a persisted [`Thread`] to a Markdown transcript.
pub mod thread_export;
/// Searchable summary index over the thread store.
pub mod thread_index;
/// Durable thread storage: threads, messages, notes, token usage.
pub mod thread_store;
/// Persisted "always allow" grants for external (MCP) tools.
pub mod tool_grants;
/// Spill store keeping oversized tool output out of conversation history.
mod tool_output;
/// Core conversation types: [`Role`], [`Message`], [`ContentBlock`].
pub mod types;

pub use assets::AgentAssetStore;
pub use capabilities::{
    AgentCapabilityPreferences, CapabilityOp, CapabilityProvider, CapabilityResolution,
    CapabilityStatus, CapabilityTier, ConnectorHealth, capability_matrix,
    resolve as resolve_capability,
};
pub use event::{AgentEvent, AgentUiEvent};
pub use permissions::{AgentPermissions, PermissionLevel, ToolCapability, tool_identity};
pub use provider::{AgentProvider, StreamOptions};
pub use registry::{
    ToolApprovalRequest, ToolCallPreview, ToolDecision, ToolDefinition, ToolExecutionPolicy,
    ToolFuture, ToolHandler, ToolInputValidator, ToolOrigin, ToolRegistry, ToolResultContent,
    ToolRisk,
};
pub use run_monitor::{
    HeartbeatKind, RunMonitorRecord, RunMonitorRegistration, RunMonitorState, RunMonitorStore,
    RunObservation,
};
pub use session::{AgentSession, ImageAttachment, ToolApprovalFuture, ToolApprovalHandler};
pub use thread_delivery::{
    ThreadDeliveryGateway, ThreadDeliveryInput, ThreadDeliveryReceipt, ThreadDeliverySource,
};
pub use thread_index::{ThreadFilter, ThreadIndex, ThreadIndexData, ThreadSummary};
pub use thread_store::{Thread, ThreadMessage, ThreadNote, ThreadStore, TokenUsage};
pub use types::{ContentBlock, ImageAsset, Message, Role};
