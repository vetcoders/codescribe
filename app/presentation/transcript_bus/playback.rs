//! External agent rows on the private journal: typed user messages, delivery
//! controls, agent replies, reply playback receipts (`waiting`, `playing`,
//! `spoken`, `failed`, `refused`, `stopped`) and panel focus. Each schema is
//! admitted only with its owner lease; none carries transcript authority.

use std::io;
use std::path::Path;

use super::attachments::*;
use super::envelope::*;

/// External agent messages and delivery controls use the same private journal.
/// PCM transcript authority remains reducer-only.
pub fn append_agent_event(path: &Path, event: &serde_json::Value) -> io::Result<serde_json::Value> {
    if event["schema"] == crate::presentation::dual_host_projection::HISTORY_PROJECTION_SCHEMA {
        return crate::presentation::dual_host_projection::append_projection(path, event);
    }
    if event["schema"] == "codescribe.capture-attachment.v1" {
        return append_capture_attachment(path, event);
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid agent event");
    if !path.is_absolute()
        || !event.is_object()
        || ["projection", "pair_id", "source_receipt"]
            .iter()
            .any(|key| event.get(*key).is_some())
    {
        return Err(invalid());
    }
    let text_field = |key: &str| event[key].as_str().filter(|value| !value.trim().is_empty());
    let identity_field = |key: &str| {
        text_field(key).is_some_and(|value| {
            value.len() == 24
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    };
    let identity_key = match event["schema"].as_str() {
        Some("codescribe.agent-user-message.v1") => "message_id",
        Some("codescribe.delivery-control.v1") => "control_id",
        Some("codescribe.agent-panel-focus.v1" | "codescribe.agent-panel-focus-result.v1") => {
            "request_id"
        }
        _ => "reply_id",
    };
    // Panel focus rows only point at a vc-frame pane or a terminal tab; they
    // must never look like transcript evidence or carry words.
    let carries_transcript_claims = || {
        [
            "text",
            "wav",
            "capture_epoch",
            "sample_start",
            "sample_end",
            "occurrence_session_id",
            "hold_until",
        ]
        .iter()
        .any(|key| event.get(*key).is_some())
    };
    if !identity_field(identity_key)
        || ["provider", "provider_session_id", "lease_id", "emitted_at"]
            .iter()
            .any(|key| text_field(key).is_none())
    {
        return Err(invalid());
    }
    {
        use sha2::{Digest, Sha256};
        let provider = text_field("provider").ok_or_else(invalid)?;
        let session = text_field("provider_session_id").ok_or_else(invalid)?;
        let owner = format!("{}\0{session}", provider.to_lowercase());
        let lease = hex::encode(Sha256::digest(owner.as_bytes()));
        if provider != provider.to_lowercase() || event["lease_id"] != lease[..32] {
            return Err(invalid());
        }
    }
    match (event["schema"].as_str(), event["kind"].as_str()) {
        (Some("codescribe.delivery-control.v1"), Some("revise" | "withdraw" | "release")) => {
            if ["bus", "channel", "audience"]
                .iter()
                .any(|key| text_field(key).is_none())
                || event["bus"].as_str() != path.to_str()
                || !matches!(
                    event["channel"].as_str(),
                    Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                )
                || chrono::DateTime::parse_from_rfc3339(
                    text_field("emitted_at").ok_or_else(invalid)?,
                )
                .is_err()
                || event
                    .get("message_id")
                    .is_some_and(|value| !value.is_null() && !identity_field("message_id"))
                || event.get("utterance_id").is_some_and(|value| {
                    !value.is_null()
                        && (value.as_u64().is_none() || text_field("session_id").is_none())
                })
                || event
                    .get("session_id")
                    .is_some_and(|value| !value.is_null() && text_field("session_id").is_none())
                || (event["kind"] == "revise"
                    && (text_field("text").is_none()
                        || event["text"]
                            .as_str()
                            .is_some_and(|text| text.len() > 64 * 1024)))
                || [
                    "status",
                    "wav",
                    "capture_epoch",
                    "sample_start",
                    "sample_end",
                    "occurrence_session_id",
                    "hold_until",
                ]
                .iter()
                .any(|key| event.get(*key).is_some())
            {
                return Err(invalid());
            }
        }
        (Some("codescribe.agent-user-message.v1"), Some("agent_user_message")) => {
            let recipients = event["recipients"].as_array().ok_or_else(invalid)?;
            let owner = recipients.first().ok_or_else(invalid)?;
            if text_field("text").is_none()
                || event["text"]
                    .as_str()
                    .is_some_and(|text| text.len() > 64 * 1024)
                || event["source"] != "typed"
                || event["source_event_id"] != event["message_id"]
                || event
                    .get("final_revision")
                    .is_some_and(|revision| revision.as_u64().is_none_or(|n| n == 0))
                || event.get("history_reply_to").is_some_and(|reference| {
                    !reference.is_null()
                        && !serde_json::from_value::<
                            crate::presentation::dual_host_projection::MessageReference,
                        >(reference.clone())
                        .is_ok_and(|reference| reference.valid())
                })
                || recipients.len() != 1
                || [
                    "provider",
                    "provider_session_id",
                    "lease_id",
                    "channel",
                    "audience",
                ]
                .iter()
                .any(|key| owner[*key] != event[*key] || text_field(key).is_none())
                || owner["bus"].as_str() != path.to_str()
                || !matches!(
                    event["channel"].as_str(),
                    Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                )
                || [
                    "status",
                    "wav",
                    "capture_epoch",
                    "sample_start",
                    "sample_end",
                    "occurrence_session_id",
                ]
                .iter()
                .any(|key| event.get(*key).is_some())
            {
                return Err(invalid());
            }
        }

        (Some("codescribe.agent-reply.v1"), Some("agent_reply")) => {
            if text_field("text").is_none()
                || event["spoken"].as_bool() != Some(false)
                || !matches!(
                    event["association"].as_str(),
                    Some("addressed" | "unsolicited")
                )
                || (event["association"] == "addressed" && !identity_field("delivery_id"))
                || (event["association"] == "unsolicited" && !event["delivery_id"].is_null())
            {
                return Err(invalid());
            }
        }
        (Some("codescribe.agent-reply-playback.v1"), Some("agent_reply_playback")) => {
            if !identity_field("playback_ticket")
                || !matches!(
                    event["state"].as_str(),
                    Some("waiting" | "playing" | "spoken" | "failed" | "refused" | "stopped")
                )
                || event["spoken"].as_bool() != Some(event["state"] == "spoken")
            {
                return Err(invalid());
            }
        }
        (Some("codescribe.agent-panel-focus.v1"), Some("focus_agent_panel")) => {
            if event["bus"].as_str() != path.to_str()
                || !matches!(
                    event["channel"].as_str(),
                    Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                )
                || chrono::DateTime::parse_from_rfc3339(
                    text_field("emitted_at").ok_or_else(invalid)?,
                )
                .is_err()
                || event.get("status").is_some()
                || carries_transcript_claims()
            {
                return Err(invalid());
            }
        }
        (Some("codescribe.agent-panel-focus-result.v1"), Some("agent_panel_focus_result")) => {
            if !matches!(
                event["status"].as_str(),
                Some(
                    "focused"
                        | "already_focused"
                        | "pane_missing"
                        | "terminal_missing"
                        | "automation_denied"
                        | "no_frame"
                        | "stale"
                        | "error"
                )
            ) || !(event["exit_code"].is_null() || event["exit_code"].is_i64())
                || event["elapsed_ms"].as_u64().is_none()
                || !(event["frame"].is_null() || event["frame"].is_object())
                || !(event["terminal"].is_null() || event["terminal"].is_object())
                || !(event["target"].is_null()
                    || matches!(
                        event["target"].as_str(),
                        Some("frame" | "iterm2" | "terminal")
                    ))
                || chrono::DateTime::parse_from_rfc3339(
                    text_field("emitted_at").ok_or_else(invalid)?,
                )
                .is_err()
                || carries_transcript_claims()
            {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    if event["schema"] == "codescribe.agent-user-message.v1" {
        if chrono::DateTime::parse_from_rfc3339(text_field("emitted_at").ok_or_else(invalid)?)
            .is_err()
            || serde_json::to_vec(event).map_err(io::Error::other)?.len() > 16 << 20
        {
            return Err(invalid());
        }
        return crate::presentation::dual_host_projection::append_original(path, event);
    }
    let encoded = crate::presentation::transcript_bus_maintenance::generation::encode(event)?;
    if encoded.len() > 24 << 20 {
        return Err(invalid());
    }
    let file = shared_bus_file(path)?;
    let receipt = crate::presentation::transcript_bus_maintenance::generation::append_durable(
        path, &file, &encoded,
    )?;
    serde_json::to_value(receipt).map_err(io::Error::other)
}
