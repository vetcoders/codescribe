//! Station-qualified conversation history. These rows are never delivery input.
//! Encoding, locking, append and durability stay in the existing generation owner.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::transcript_bus::shared_bus_file;
use super::transcript_bus_maintenance::generation;

pub const HISTORY_PROJECTION_SCHEMA: &str = "codescribe.history-projection.v1";
pub const STATION_PAIR_SCHEMA: &str = "codescribe.station-pair.v1";
pub const SEND_RESULT_SCHEMA: &str = "codescribe.bus-send-result.v1";
pub const HISTORY_LIMIT: usize = 256 * 1024;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "station history contract refused",
    )
}

fn hex_id(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn label(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn generation_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalOwner {
    pub provider: String,
    pub provider_session_id: String,
    pub lease_id: String,
}

impl LogicalOwner {
    fn validate(&self) -> io::Result<()> {
        let digest = hex::encode(Sha256::digest(format!(
            "{}\0{}",
            self.provider, self.provider_session_id
        )));
        if !label(&self.provider, 64)
            || self.provider != self.provider.to_lowercase()
            || !label(&self.provider_session_id, 512)
            || self.lease_id != digest[..32]
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn matches(&self, value: &Value) -> bool {
        value["provider"] == self.provider
            && value["provider_session_id"] == self.provider_session_id
            && value["lease_id"] == self.lease_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Station {
    pub host_id: String,
    pub generation_id: String,
    pub channel: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationTransition {
    pub host_id: String,
    pub from: String,
    pub to: String,
    pub receipt_id: String,
}

/// Trusted local admission receipt, never accepted from an import envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationPairing {
    pub schema: String,
    pub pair_id: String,
    pub home_host_id: String,
    pub owner: LogicalOwner,
    pub bridge_root: PathBuf,
    pub local_bus: PathBuf,
    pub local: Station,
    pub peer: Station,
    pub transitions: Vec<GenerationTransition>,
    pub guest_broadcast_off_receipt_id: String,
}

fn private_json(path: &Path) -> io::Result<Value> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(invalid());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
        || meta.len() > HISTORY_LIMIT as u64
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(HISTORY_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > HISTORY_LIMIT {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

impl StationPairing {
    fn load(path: &Path) -> io::Result<Self> {
        let receipt = PathBuf::from(format!("{}.station-pair.json", path.display()));
        let pair: Self = serde_json::from_value(private_json(&receipt)?).map_err(|_| invalid())?;
        pair.owner.validate()?;
        if pair.schema != STATION_PAIR_SCHEMA || !hex_id(&pair.pair_id, 24)
            || pair.local_bus != path || path.canonicalize()? != path
            || !pair.bridge_root.is_absolute() || pair.bridge_root.canonicalize()? != pair.bridge_root
            || pair.local.host_id == pair.peer.host_id || pair.transitions.len() > 64
            || ![&pair.local.host_id, &pair.peer.host_id].contains(&&pair.home_host_id)
            || !hex_id(&pair.guest_broadcast_off_receipt_id, 24)
        {
            return Err(invalid());
        }
        for station in [&pair.local, &pair.peer] {
            if !hex_id(&station.host_id, 32) || !generation_id(&station.generation_id)
                || !matches!(station.channel.as_str(), "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            {
                return Err(invalid());
            }
        }
        for (index, transition) in pair.transitions.iter().enumerate() {
            if ![&pair.local.host_id, &pair.peer.host_id].contains(&&transition.host_id)
                || !generation_id(&transition.from) || !generation_id(&transition.to)
                || transition.from == transition.to || !hex_id(&transition.receipt_id, 24)
                || pair.transitions[..index].iter().any(|prior| prior.host_id == transition.host_id
                    && (prior.from == transition.from || prior.to == transition.to))
            {
                return Err(invalid());
            }
        }
        if pair.transitions.iter().any(|t| !pair.admits_generation(&t.host_id, &t.from)) {
            return Err(invalid());
        }
        let host = private_json(&pair.bridge_root.join("host-instance.json"))?;
        let bindings = private_json(&pair.bridge_root.join("vc.agent-audience-binding.v1.json"))?;
        let binding = &bindings["bindings"][&pair.local.channel];
        let manifest = generation::view(path)?.ok_or_else(invalid)?;
        if host["schema"] != "codescribe.host-instance.v1" || host["host_id"] != pair.local.host_id
            || bindings["schema"] != "vc.agent-audience-binding.v1"
            || binding["provider"] != pair.owner.provider
            || binding["provider_session_id"] != pair.owner.provider_session_id
            || binding.get("lease_id").is_some_and(|id| id != &pair.owner.lease_id)
            || binding["bus"].as_str() != path.to_str()
            || pair.local.generation_id != manifest.stream_id
        {
            return Err(invalid());
        }
        Ok(pair)
    }

    fn admits_generation(&self, host: &str, original: &str) -> bool {
        let Some(station) = [&self.local, &self.peer].into_iter().find(|s| s.host_id == host) else {
            return false;
        };
        let mut current = original;
        let mut seen = std::collections::HashSet::new();
        while current != station.generation_id {
            if !seen.insert(current) {
                return false;
            }
            let Some(next) = self.transitions.iter().find(|t| t.host_id == host && t.from == current) else {
                return false;
            };
            current = &next.to;
        }
        true
    }
}

/// Binding writers already take this lock. Acquire it before any generation
/// lease, so a direct canonical import has the same rebind boundary as helper Send.
struct BindingReadLease(File);

impl BindingReadLease {
    fn acquire(root: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join("vc.agent-audience-binding.v1.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}

impl Drop for BindingReadLease {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    Source,
    Replica,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Typed,
    Voice,
    AgentReply,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReference {
    pub source_host_id: String,
    pub source_generation_id: String,
    pub message_id: String,
}

impl MessageReference {
    pub(crate) fn valid(&self) -> bool {
        hex_id(&self.source_host_id, 32)
            && generation_id(&self.source_generation_id)
            && hex_id(&self.message_id, 24)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalOrder {
    pub counter: u64,
    pub source_host_id: String,
    pub message_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySender {
    pub name: String,
    pub channel: String,
    pub owner: Option<LogicalOwner>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryAttachment {
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
    /// Provenance only. No receiving reader may open this path.
    pub source_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEligibility {
    pub broadcast: bool,
    pub release_control_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSnapshot {
    pub origin: MessageReference,
    pub final_revision: u64,
    pub order: LogicalOrder,
    pub kind: MessageKind,
    pub emitted_at: String,
    pub sender: HistorySender,
    pub recipient: LogicalOwner,
    pub text: String,
    pub reply_to: Option<MessageReference>,
    pub attachments: Vec<HistoryAttachment>,
    pub eligibility: HistoryEligibility,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryProjection {
    pub schema: String,
    pub kind: String,
    pub pair_id: String,
    pub projection: ProjectionKind,
    pub owner: LogicalOwner,
    pub source_receipt: generation::BusAppendReceipt,
    pub message: MessageSnapshot,
}

impl HistoryProjection {
    fn validate(&self, pair: &StationPairing) -> io::Result<()> {
        let message = &self.message;
        self.owner.validate()?;
        if self.schema != HISTORY_PROJECTION_SCHEMA || self.kind != "history_projection"
            || self.pair_id != pair.pair_id || self.owner != pair.owner || message.recipient != pair.owner
            || !message.origin.valid() || message.final_revision == 0
            || !pair.admits_generation(&message.origin.source_host_id, &message.origin.source_generation_id)
            || self.source_receipt.stream_id != message.origin.source_generation_id
            || self.source_receipt.length == 0 || self.source_receipt.length > 24 << 20
            || self.source_receipt.offset.checked_add(self.source_receipt.length).is_none()
            || message.order.source_host_id != message.origin.source_host_id
            || message.order.message_id != message.origin.message_id
            || (self.projection == ProjectionKind::Replica && message.order.counter == 0)
            || (self.projection == ProjectionKind::Source && message.origin.source_host_id != pair.local.host_id)
            || (self.projection == ProjectionKind::Replica && message.origin.source_host_id != pair.peer.host_id)
            || chrono::DateTime::parse_from_rfc3339(&message.emitted_at).is_err()
            || message.text.trim().is_empty() || message.text.len() > 64 * 1024
            || !label(&message.sender.name, 256) || !label(&message.sender.channel, 16)
            || (message.kind == MessageKind::AgentReply && message.sender.owner.as_ref() != Some(&pair.owner))
            || (message.kind != MessageKind::AgentReply && message.sender.owner.is_some())
            || message.reply_to.as_ref().is_some_and(|r| !r.valid() || r == &message.origin)
            || message.attachments.len() > 64
            || message.eligibility.release_control_id.as_ref().is_some_and(|id| !hex_id(id, 24))
        {
            return Err(invalid());
        }
        for attachment in &message.attachments {
            if !label(&attachment.name, 255) || !label(&attachment.media_type, 128)
                || !label(&attachment.source_path, 4096) || attachment.bytes == 0
                || attachment.bytes > 50 * 1024 * 1024 || !hex_id(&attachment.sha256, 64)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

/// The transport can send only this closed reason vocabulary to the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendFailureReason {
    InvalidBinding,
    IncompleteBinding,
    StaleLease,
    NotListening,
    TransportUnavailable,
    AttachmentUnavailable,
    InvalidRequest,
    PublicationUncertain,
}

impl SendFailureReason {
    pub const fn state(self) -> &'static str {
        match self {
            Self::PublicationUncertain => "uncertain",
            _ => "rejected",
        }
    }
}

/// Original typed requests use exact owner + retained message ID, never text.
pub(crate) fn append_original(path: &Path, event: &Value) -> io::Result<Value> {
    let shared = shared_bus_file(path)?;
    let mut recovered = None;
    let receipts = generation::append_durable_many_with(
        &[(path.to_path_buf(), Arc::clone(&shared))],
        || {
            generation::scan_durable_events(path, |row, receipt| {
                if row["schema"] == event["schema"] && row["message_id"] == event["message_id"]
                    && ["provider", "provider_session_id", "lease_id"].iter().all(|key| row[*key] == event[*key])
                {
                    if row != event || recovered.is_some() {
                        return Err(invalid());
                    }
                    recovered = Some(receipt.clone());
                }
                Ok(())
            })?;
            Ok(vec![if recovered.is_some() { None } else { Some(generation::encode(event)?) }])
        },
        None,
    )?;
    let receipt = recovered.or_else(|| receipts.into_iter().next().flatten()).ok_or_else(invalid)?;
    serde_json::to_value(receipt).map_err(io::Error::other)
}

/// Validate/reconcile under the ordinary writer lease. This module never writes
/// a journal or a second receipt store. Source snapshots backfill order once;
/// replicas carry that order verbatim and cannot become an executable event.
pub(crate) fn append_projection(path: &Path, event: &Value) -> io::Result<Value> {
    if !path.is_absolute() || path.canonicalize()? != path
        || serde_json::to_vec(event).map_err(io::Error::other)?.len() > HISTORY_LIMIT
    {
        return Err(invalid());
    }
    let mut projection: HistoryProjection = serde_json::from_value(event.clone()).map_err(|_| invalid())?;
    let initial_pair = StationPairing::load(path)?;
    let _binding = BindingReadLease::acquire(&initial_pair.bridge_root)?;
    let shared = shared_bus_file(path)?;
    let mut recovered = None;
    let mut frozen = None;
    let receipts = generation::append_durable_many_with(
        &[(path.to_path_buf(), Arc::clone(&shared))],
        || {
            let pair = StationPairing::load(path)?;
            if pair != initial_pair {
                return Err(invalid());
            }
            projection.validate(&pair)?;
            let mut maximum = 0u64;
            let mut parent_order = None;
            let mut known_references = Vec::new();
            let mut source = None;
            let mut controls = Vec::new();
            generation::scan_durable_events(path, |row, receipt| {
                if row["schema"] == HISTORY_PROJECTION_SCHEMA {
                    let prior: HistoryProjection = serde_json::from_value(row.clone()).map_err(|_| invalid())?;
                    maximum = maximum.max(prior.message.order.counter);
                    if prior.owner == projection.owner {
                        known_references.push(prior.message.origin.clone());
                        if projection.message.reply_to.as_ref() == Some(&prior.message.origin) {
                            parent_order = Some(prior.message.order.counter);
                        }
                        if prior.message.reply_to.as_ref() == Some(&projection.message.origin)
                            && prior.message.order.counter <= projection.message.order.counter
                        {
                            return Err(invalid());
                        }
                    }
                    if prior.owner == projection.owner && prior.message.origin == projection.message.origin {
                        let mut retry = projection.clone();
                        if retry.projection == ProjectionKind::Source && retry.message.order.counter == 0 {
                            retry.message.order = prior.message.order.clone();
                        }
                        if prior != retry || recovered.is_some() {
                            return Err(invalid());
                        }
                        recovered = Some(receipt.clone());
                        frozen = Some(prior);
                    }
                }
                if receipt == &projection.source_receipt {
                    source = Some(row.clone());
                }
                let selected_control = row["message_id"] == projection.message.origin.message_id
                    || source.as_ref().is_some_and(|original| {
                        row["session_id"].as_str().is_some()
                            && row["session_id"] == original["session_id"]
                    });
                if row["schema"] == "codescribe.delivery-control.v1"
                    && projection.owner.matches(row) && selected_control {
                    controls.push(row.clone());
                }
                Ok(())
            })?;
            if recovered.is_some() {
                return Ok(vec![None]);
            }
            if projection.projection == ProjectionKind::Source {
                validate_source(&projection, &source.ok_or_else(invalid)?, &controls, &known_references)?;
                if projection.message.order.counter != 0 {
                    return Err(invalid());
                }
                projection.message.order.counter = maximum.checked_add(1).ok_or_else(invalid)?;
                if projection.message.reply_to.is_some() && parent_order.is_none() {
                    return Err(invalid());
                }
            }
            if parent_order.is_some_and(|parent| parent >= projection.message.order.counter) {
                return Err(invalid());
            }
            let encoded = generation::encode(&projection)?;
            frozen = Some(projection.clone());
            Ok(vec![Some(encoded)])
        },
        None,
    )?;
    let receipt = recovered.or_else(|| receipts.into_iter().next().flatten()).ok_or_else(invalid)?;
    Ok(json!({"source": receipt, "projection": frozen.ok_or_else(invalid)?}))
}

fn validate_source(
    projection: &HistoryProjection,
    row: &Value,
    controls: &[Value],
    known_references: &[MessageReference],
) -> io::Result<()> {
    let message = &projection.message;
    // Backfill accepts original committed conversational rows only. A source
    // projection can never vouch for another projection or a draft/control.
    let (kind, identity) = match row["schema"].as_str() {
        Some("codescribe.agent-user-message.v1") => (MessageKind::Typed,
            row["message_id"].as_str().ok_or_else(invalid)?.to_owned()),
        Some("codescribe.agent-reply.v1") => (MessageKind::AgentReply,
            row["reply_id"].as_str().ok_or_else(invalid)?.to_owned()),
        Some("codescribe.transcript.v1") if row["status"] == "transcript_sealed"
            && row["closing_decision"] == "publish" => {
            let session = row["session_id"].as_str().filter(|s| s.starts_with("agent-channel-")).ok_or_else(invalid)?;
            // The existing channel-message identity is metadata-only. Keep its
            // exact wire identity; acoustic occurrence identity stays upstream.
            let identity = if let Some(id) = row.get("message_id") {
                id.as_str().filter(|id| hex_id(id, 24)).ok_or_else(invalid)?.to_owned()
            } else {
                hex::encode(Sha256::digest(format!("channel-message\0{session}")))[..24].to_owned()
            };
            (MessageKind::Voice, identity)
        }
        _ => return Err(invalid()),
    };
    let recipient = projection.owner.matches(row) || (kind == MessageKind::Voice
        && row["recipients"].as_array().is_some_and(|items| items.iter().any(|r| projection.owner.matches(r))));
    let sender = if kind == MessageKind::Voice {
        HistorySender { name: "founder".into(), channel: "10".into(), owner: None }
    } else if kind == MessageKind::AgentReply {
        HistorySender {
            name: row["sender"]["name"].as_str().or_else(|| row["name"].as_str()).ok_or_else(invalid)?.into(),
            channel: row["sender"]["channel"].as_str().or_else(|| row["channel"].as_str()).ok_or_else(invalid)?.into(),
            owner: Some(projection.owner.clone()),
        }
    } else {
        HistorySender {
            name: row["sender"]["name"].as_str().ok_or_else(invalid)?.into(),
            channel: row["sender"]["channel"].as_str().ok_or_else(invalid)?.into(),
            owner: None,
        }
    };
    let native_reply = row["user_reply_to"]["message_id"].as_str().or_else(|| {
        (kind == MessageKind::AgentReply && row["association"] == "addressed")
            .then(|| row["message_id"].as_str()).flatten()
    });
    let reply = if !row["history_reply_to"].is_null() {
        Some(serde_json::from_value::<MessageReference>(row["history_reply_to"].clone()).map_err(|_| invalid())?)
    } else if let Some(identity) = native_reply {
        let mut candidates = known_references.iter().filter(|r| r.message_id == identity);
        let selected = candidates.next().ok_or_else(invalid)?.clone();
        if candidates.next().is_some() {
            return Err(invalid());
        }
        Some(selected)
    } else {
        None
    };
    if kind != message.kind || identity != message.origin.message_id || !recipient
        || row["emitted_at"] != message.emitted_at || sender != message.sender
        || reply != message.reply_to
        || (row["association"] == "addressed" && reply.is_none())
        || (native_reply.is_some_and(|id| reply.as_ref().is_none_or(|reply| reply.message_id != id)))
    {
        return Err(invalid());
    }
    let mut text = row["text"].as_str().ok_or_else(invalid)?;
    let mut revision = row["final_revision"].as_u64().or_else(|| row["reducer_revision"].as_u64()).filter(|n| *n > 0).unwrap_or(1);
    let mut withdrawn = false;
    let mut released = None;
    let deadline = row.get("hold_until").map(|deadline| {
        chrono::DateTime::parse_from_rfc3339(deadline.as_str().ok_or_else(invalid)?).map_err(|_| invalid())
    }).transpose()?;
    for control in controls {
        let selected = control["message_id"] == message.origin.message_id
            || (kind == MessageKind::Voice && control["session_id"] == row["session_id"]
                && control["session_id"].as_str().is_some());
        if !selected || deadline.is_none() {
            continue;
        }
        let time = chrono::DateTime::parse_from_rfc3339(control["emitted_at"].as_str().ok_or_else(invalid)?).map_err(|_| invalid())?;
        if time < chrono::DateTime::parse_from_rfc3339(&message.emitted_at).map_err(|_| invalid())? {
            return Err(invalid());
        }
        if deadline.is_some_and(|deadline| time >= deadline) {
            continue;
        }
        match control["kind"].as_str() {
            Some("withdraw") => { withdrawn = true; break; }
            Some("revise") => {
                text = control["text"].as_str().ok_or_else(invalid)?;
                revision = revision.checked_add(1).ok_or_else(invalid)?;
            }
            Some("release") => { released = Some(control["control_id"].as_str().ok_or_else(invalid)?); break; }
            _ => return Err(invalid()),
        }
    }
    let held = deadline.is_some_and(|deadline| deadline > chrono::Utc::now());
    let broadcast = row["origin_channel"] == "0" || row["audience"] == "*";
    let broadcast_eligible = !broadcast || row["target"]["recipients"].as_array().is_some_and(|recipients| {
        recipients.iter().any(|r| r["channel"] == row["channel"] && r["name"] == row["audience"])
    });
    let originals = row["attachments"].as_array().cloned().unwrap_or_default();
    let attachments = message.attachments.iter().map(|a| json!({
        "name": a.name, "media_type": a.media_type, "bytes": a.bytes,
        "sha256": a.sha256, "path": a.source_path,
    })).collect::<Vec<_>>();
    if withdrawn || (held && released.is_none()) || !broadcast_eligible
        || message.eligibility.broadcast != broadcast
        || message.eligibility.release_control_id.as_deref() != released
        || message.text != text || message.final_revision != revision || originals != attachments
    {
        return Err(invalid());
    }
    Ok(())
}
