//! Crash boundary for consultation execution. Unfinished input is retained here;
//! completed conversation history remains exclusively in ThreadStore. Pending
//! means potentially executed, never permission to replay after a restart.
//!
//! A turn interrupted by a crash or a failure is therefore *abandoned*, not
//! retained for recovery: the next `open` moves it into `abandoned`, drops the
//! instructions still waiting behind it, and the conversation continues. The
//! abandoned and completed identities are both permanently refused, so nothing
//! is ever replayed and no tool effect is rolled back or retried. An
//! unwritable journal is the only state that still blocks execution, because
//! there the owner cannot prove what it recorded.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{ThreadStore, canonical_existing_child, validate_thread_id};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionState {
    completed: BTreeSet<String>,
    pending: Option<String>,
    #[serde(default)]
    queued: Vec<QueuedInstruction>,
    /// Turns that began and never completed. Refused for life exactly like
    /// `completed`, but they left no history, so they are tracked separately.
    #[serde(default)]
    abandoned: BTreeSet<String>,
}

/// Characters of a dropped instruction kept to explain the gap to its reader.
const DROPPED_PREVIEW_CHARS: usize = 120;

/// One instruction dropped without being executed. The preview exists to
/// explain the gap in the conversation; it is never an input for a replay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DroppedInput {
    pub turn_id: String,
    pub text_preview: String,
}

/// What self-healing cost the conversation. Recording this is a note for the
/// reader, never a retry: effects already executed stay exactly as they are.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConsultationRecovery {
    pub abandoned_turn_id: Option<String>,
    pub dropped_inputs: Vec<DroppedInput>,
    pub reason: String,
}

/// Source user text of a dropped instruction, whitespace-normalized and
/// clipped by `char` so a multi-byte boundary cannot panic. Control blocks and
/// images contribute nothing, so a turn without text yields an empty preview.
fn dropped_input(entry: &QueuedInstruction) -> DroppedInput {
    use crate::agent::{ContentBlock, Role};
    let mut text_preview = String::new();
    if entry.input.role == Role::User {
        let words = entry
            .input
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        text_preview = words
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(DROPPED_PREVIEW_CHARS)
            .collect();
    }
    DroppedInput {
        turn_id: entry.turn_id.clone(),
        text_preview,
    }
}

/// Recovery data, not an executable provider or permission grant. Never load
/// credentials into this record. Group evidence requires ledger revalidation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueuedInstruction {
    pub turn_id: String,
    pub input: crate::agent::Message,
    pub provider_name: String,
    pub options: serde_json::Value,
    pub group: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedConsultation {
    thread_id: String,
}

/// Reading settings must not mint a consultation or acquire execution ownership.
pub(crate) fn inspect_selected_max_consultation(
    store: &ThreadStore,
) -> Result<Option<crate::agent::thread_delivery::ConsultationRecoverySnapshot>> {
    let Some(directory) = existing_consultation_directory(&store.threads_dir, "consultations")?
    else {
        return Ok(None);
    };
    let Some(selection) = existing_consultation_directory(&directory, "selection")? else {
        return Ok(None);
    };
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(selection.join("current.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect Max consultation selection"),
    };
    let selected: SelectedConsultation =
        serde_json::from_reader(file).context("corrupt Max consultation selection")?;
    validate_thread_id(&selected.thread_id)?;
    // Selection can change after this read. Return the identity actually read,
    // never relabel its journal as belonging to a subsequently selected thread.
    Ok(Some(
        inspect_retained_input(store, &selected.thread_id)?.unwrap_or(
            crate::agent::thread_delivery::ConsultationRecoverySnapshot {
                consultation_id: selected.thread_id,
                pending_turn_id: None,
                retained_inputs: Vec::new(),
            },
        ),
    ))
}

fn existing_consultation_directory(parent: &Path, name: &str) -> Result<Option<PathBuf>> {
    let directory = parent.join(name);
    match fs::symlink_metadata(&directory) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect consultation directory"),
    }
    let directory = canonical_existing_child(parent, &directory)?;
    ensure!(
        directory.is_dir(),
        "consultation directory is not a directory"
    );
    Ok(Some(directory))
}

pub(crate) fn inspect_retained_input(
    store: &ThreadStore,
    id: &str,
) -> Result<Option<crate::agent::thread_delivery::ConsultationRecoverySnapshot>> {
    use crate::agent::thread_delivery::{ConsultationInputSnapshot, ConsultationRecoverySnapshot};
    use crate::agent::{ContentBlock, Role};
    validate_thread_id(id)?;
    let Some(directory) = existing_consultation_directory(&store.threads_dir, "consultations")?
    else {
        return Ok(None);
    };
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(format!("{id}.json")))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect consultation input"),
    };
    // Atomic rename makes this a coherent old-or-new snapshot while an owner
    // is running. It does not make the snapshot a lease or a recovery decision.
    let state: AdmissionState =
        serde_json::from_reader(file).context("corrupt consultation admission state")?;
    let mut ids = BTreeSet::new();
    for entry in &state.queued {
        ensure!(
            !entry.turn_id.trim().is_empty()
                && ids.insert(entry.turn_id.clone())
                && !state.completed.contains(&entry.turn_id),
            "invalid retained consultation identity"
        );
        ensure!(
            entry.input.role == Role::User
                && entry.input.content.iter().all(|block| matches!(
                    block,
                    ContentBlock::Text(_) | ContentBlock::Image { .. }
                )),
            "retained consultation input is not source user content"
        );
    }
    if let Some(pending) = &state.pending {
        ensure!(
            !pending.trim().is_empty() && !state.completed.contains(pending),
            "invalid pending consultation identity"
        );
        ensure!(
            state.queued.is_empty() || state.queued[0].turn_id == *pending,
            "pending consultation is not the retained front"
        );
    }
    Ok(Some(ConsultationRecoverySnapshot {
        consultation_id: id.to_string(),
        pending_turn_id: state.pending,
        retained_inputs: state
            .queued
            .into_iter()
            .map(|entry| ConsultationInputSnapshot {
                turn_id: entry.turn_id,
                input: entry.input,
                provider_name: entry.provider_name,
            })
            .collect(),
    }))
}

/// Conversation selection is thread state, not an ASR/provider setting. Only
/// first use mints an id; corrupt state and unresolved turns never choose a
/// different conversation as a side effect of recovery.
pub(crate) fn selected_id(store: &ThreadStore) -> Result<String> {
    let path = selection_path(store)?;
    let _selection_owner = exclusive_owner(&path.with_file_name("owner.lock"))?;
    let selected: SelectedConsultation = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => serde_json::from_reader(file).context("corrupt Max consultation selection")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let selected = SelectedConsultation {
                thread_id: ThreadStore::generate_id(),
            };
            persist_json(&path, &selected)?;
            selected
        }
        Err(error) => return Err(error).context("read Max consultation selection"),
    };
    validate_thread_id(&selected.thread_id)?;
    Ok(selected.thread_id)
}

/// Explicit new-conversation action. Preserve the old journal and history,
/// including unresolved effects; never reinterpret them as successful work.
pub(crate) fn begin_new(store: &ThreadStore, expected: &str) -> Result<String> {
    validate_thread_id(expected)?;
    let path = selection_path(store)?;
    let _selection_owner = exclusive_owner(&path.with_file_name("owner.lock"))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let selected: SelectedConsultation =
        serde_json::from_reader(file).context("corrupt Max consultation selection")?;
    ensure!(
        selected.thread_id == expected,
        "Max consultation selection changed; refusing stale reset"
    );
    // Caller must first close its owner. Another process still owning that
    // conversation also prevents reset, regardless of its apparent UI state.
    let directory = consultation_directory(store)?;
    let _old_owner = exclusive_owner(&directory.join(format!("{expected}.lock")))?;
    let replacement = SelectedConsultation {
        thread_id: ThreadStore::generate_id(),
    };
    persist_json(&path, &replacement)?;
    Ok(replacement.thread_id)
}

fn selection_path(store: &ThreadStore) -> Result<PathBuf> {
    let directory = consultation_directory(store)?;
    let selection_dir = directory.join("selection");
    fs::create_dir_all(&selection_dir)?;
    // Directory durability only: canonical store child, no content read or write.
    File::open(&directory)?.sync_all()?;
    let selection_dir = canonical_existing_child(&directory, &selection_dir)?;
    Ok(selection_dir.join("current.json"))
}

fn consultation_directory(store: &ThreadStore) -> Result<PathBuf> {
    let directory = store.threads_dir.join("consultations");
    fs::create_dir_all(&directory)?;
    // Sync the configured storage root, not a path supplied by a consultation.
    File::open(&store.threads_dir)?.sync_all()?;
    canonical_existing_child(&store.threads_dir, &directory)
}

struct HeldExclusiveLock {
    file: File,
}

impl AsRawFd for HeldExclusiveLock {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

impl Drop for HeldExclusiveLock {
    fn drop(&mut self) {
        // close() drops one flock reference. A duplicated descriptor, including
        // one inherited across fork before exec, keeps the lock. LOCK_UN
        // releases it at drop. O_CLOEXEC does not cover that window.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn exclusive_owner(path: &Path) -> Result<HeldExclusiveLock> {
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    // SAFETY: owner holds a live file descriptor for the full lease lifetime.
    let acquired = unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    ensure!(
        acquired == 0,
        "consultation already owned or lock unavailable: {}",
        std::io::Error::last_os_error()
    );
    Ok(HeldExclusiveLock { file: owner })
}

fn persist_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    // Sync the validated journal/selection parent after rename; no content access.
    File::open(path.parent().context("consultation state parent")?)?.sync_all()?;
    Ok(())
}

pub(crate) struct ConsultationJournal {
    path: PathBuf,
    state: AdmissionState,
    recovery_reason: Option<String>,
    recovery: Option<ConsultationRecovery>,
    // Kernel ownership dies with the process; the lock file must not be unlinked.
    _owner: HeldExclusiveLock,
}

impl ConsultationJournal {
    pub(crate) fn recovery_reason(&self) -> Option<&str> {
        self.recovery_reason.as_deref()
    }

    /// Hand the self-heal performed by this journal to its owner exactly once,
    /// so the gap can be explained in history. Taking it changes no durable
    /// state; the abandoned identity is already refused on disk.
    pub(crate) fn take_recovery(&mut self) -> Option<ConsultationRecovery> {
        self.recovery.take()
    }

    /// Last resort for a journal that cannot be written: a failed turn now
    /// self-heals through `abandon_unfinished`, so this gate is reached only
    /// when the abandonment itself is unprovable. It stops all further
    /// acceptance by this owner, because nothing it records can be trusted.
    pub(crate) fn require_recovery(&mut self, reason: String) {
        self.recovery_reason.get_or_insert(reason);
    }

    /// Only completed turns wrote history, so only they can require it back.
    /// An abandoned turn left none and must not make restoration refuse.
    pub(crate) fn has_completed_turns(&self) -> bool {
        !self.state.completed.is_empty()
    }

    /// Abandon the unfinished turn instead of blocking the consultation.
    ///
    /// The pending identity is retired into `abandoned` — refused for life, so
    /// its possibly-executed effects are never repeated — and every waiting
    /// instruction is dropped, because each is either that turn's own input or
    /// an input whose execution never began. Returns `Ok(None)` when there was
    /// nothing unfinished. A failed write restores the last durable shape and
    /// leaves this owner requiring recovery: an unprovable journal must not
    /// look healed.
    pub(crate) fn abandon_unfinished(
        &mut self,
        reason: &str,
    ) -> Result<Option<ConsultationRecovery>> {
        if self.state.pending.is_none() && self.state.queued.is_empty() {
            return Ok(None);
        }
        let abandoned_turn_id = self.state.pending.take();
        let dropped = std::mem::take(&mut self.state.queued);
        let dropped_inputs = dropped.iter().map(dropped_input).collect::<Vec<_>>();
        if let Some(turn) = &abandoned_turn_id {
            self.state.abandoned.insert(turn.clone());
        }
        if let Err(error) = self.persist() {
            if let Some(turn) = &abandoned_turn_id {
                self.state.abandoned.remove(turn);
            }
            self.state.queued = dropped;
            self.state.pending = abandoned_turn_id;
            return Err(error);
        }
        Ok(Some(ConsultationRecovery {
            abandoned_turn_id,
            dropped_inputs,
            reason: reason.to_string(),
        }))
    }

    pub(crate) fn open(store: &ThreadStore, id: &str) -> Result<Self> {
        validate_thread_id(id)?;
        let directory = consultation_directory(store)?;
        let owner = exclusive_owner(&directory.join(format!("{id}.lock")))?;
        let path = directory.join(format!("{id}.json"));
        let state = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => {
                serde_json::from_reader(file).context("corrupt consultation admission state")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AdmissionState::default(),
            Err(error) => return Err(error).context("read consultation admission state"),
        };
        let mut journal = Self {
            path,
            state,
            recovery_reason: None,
            recovery: None,
            _owner: owner,
        };
        // Self-heal before the owner serves anything. A journal that cannot
        // record the abandonment stays closed: refusing to open is the only
        // honest answer when the retirement of a started turn is unprovable.
        let recovery = journal.abandon_unfinished("owner restarted before the turn finished")?;
        journal.recovery = recovery;
        Ok(journal)
    }

    pub(crate) fn accept_input(&mut self, input: QueuedInstruction) -> Result<()> {
        ensure!(
            self.recovery_reason.is_none(),
            "consultation requires recovery"
        );
        ensure!(
            !input.turn_id.trim().is_empty(),
            "turn identity is required"
        );
        ensure!(
            !self.state.completed.contains(&input.turn_id)
                && !self.state.abandoned.contains(&input.turn_id)
                && !self
                    .state
                    .queued
                    .iter()
                    .any(|entry| entry.turn_id == input.turn_id),
            "turn already admitted; refusing replay"
        );
        self.state.queued.push(input);
        self.persist()
    }

    /// Only before begin, when the owner proves no provider/tool work started.
    pub(crate) fn discard_unstarted(&mut self, turn: &str) -> Result<()> {
        ensure!(
            self.recovery_reason.is_none(),
            "consultation requires recovery"
        );
        ensure!(
            self.state.pending.is_none(),
            "cannot discard a potentially executed turn"
        );
        ensure!(
            self.state
                .queued
                .first()
                .is_some_and(|entry| entry.turn_id == turn),
            "consultation discard is out of order"
        );
        let input = self.state.queued.remove(0);
        if let Err(error) = self.persist() {
            self.state.queued.insert(0, input);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn begin(&mut self, turn: &str) -> Result<()> {
        ensure!(
            self.recovery_reason.is_none(),
            "consultation requires recovery"
        );
        ensure!(
            self.state.pending.is_none(),
            "a consultation turn is already executing"
        );
        ensure!(
            !self.state.completed.contains(turn) && !self.state.abandoned.contains(turn),
            "turn {turn} is already resolved; refusing replay"
        );
        ensure!(
            self.state
                .queued
                .first()
                .is_some_and(|entry| entry.turn_id == turn),
            "consultation execution is not the admitted front"
        );
        self.state.pending = Some(turn.to_string());
        // Failure deliberately leaves in-memory pending set: do not execute
        // anything if the before-effects receipt could not be made durable.
        self.persist()
    }

    pub(crate) fn complete(&mut self, turn: &str) -> Result<()> {
        ensure!(
            self.recovery_reason.is_none(),
            "consultation requires recovery"
        );
        ensure!(
            self.state.pending.as_deref() == Some(turn),
            "consultation completion identity mismatch"
        );
        ensure!(
            self.state
                .queued
                .first()
                .is_some_and(|entry| entry.turn_id == turn),
            "consultation completion is not the admitted front"
        );
        let input = self.state.queued.remove(0);
        self.state.completed.insert(turn.to_string());
        self.state.pending = None;
        if let Err(error) = self.persist() {
            self.state.pending = Some(turn.to_string());
            self.state.queued.insert(0, input);
            return Err(error);
        }
        Ok(())
    }

    fn persist(&mut self) -> Result<()> {
        let result = persist_json(&self.path, &self.state);
        if result.is_err() {
            self.require_recovery("consultation journal write uncertain".into());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_accept(journal: &mut ConsultationJournal, turn: &str) -> Result<()> {
        journal.accept_input(QueuedInstruction {
            turn_id: turn.into(),
            input: crate::agent::Message::new(
                crate::agent::Role::User,
                vec![crate::agent::ContentBlock::Text("instruction".into())],
            ),
            provider_name: "fixture".into(),
            options: serde_json::json!({}),
            group: None,
        })
    }

    fn accept(journal: &mut ConsultationJournal, turn: &str) {
        try_accept(journal, turn).unwrap();
    }

    #[test]
    fn dropped_owner_releases_lock_while_a_duplicate_fd_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let journal = ConsultationJournal::open(&store, "release-dup").unwrap();
        let duplicated = unsafe { libc::dup(journal._owner.as_raw_fd()) };
        assert!(duplicated >= 0, "dup: {}", std::io::Error::last_os_error());
        drop(journal);
        let reopened = ConsultationJournal::open(&store, "release-dup");
        unsafe { libc::close(duplicated) };
        reopened.expect("drop must release the consultation lock while a duplicated fd is open");
    }

    #[test]
    fn selected_inspection_never_creates_selection_or_restarts_retained_work() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let gateway = crate::agent::ThreadDeliveryGateway::new_in(dir.path()).unwrap();
        assert!(
            gateway
                .inspect_selected_max_consultation()
                .unwrap()
                .is_none()
        );
        assert!(!dir.path().join("consultations").exists());
        fs::create_dir(dir.path().join("consultations")).unwrap();
        assert!(
            gateway
                .inspect_selected_max_consultation()
                .unwrap()
                .is_none()
        );
        assert!(!dir.path().join("consultations/selection").exists());
        let id = selected_id(&store).unwrap();
        let selection = dir.path().join("consultations/selection/current.json");
        let selected_bytes = fs::read(&selection).unwrap();
        let empty = gateway
            .inspect_selected_max_consultation()
            .unwrap()
            .unwrap();
        assert_eq!(empty.consultation_id, id);
        assert!(empty.retained_inputs.is_empty());
        let journal_path = dir.path().join(format!("consultations/{id}.json"));
        assert!(!journal_path.exists());
        let mut journal = ConsultationJournal::open(&store, &id).unwrap();
        accept(&mut journal, "interrupted");
        journal.begin("interrupted").unwrap();
        let before = fs::read(&journal_path).unwrap();
        let active = gateway
            .inspect_selected_max_consultation()
            .unwrap()
            .unwrap();
        assert_eq!(active.consultation_id, id);
        assert_eq!(active.pending_turn_id.as_deref(), Some("interrupted"));
        assert_eq!(active.retained_inputs.len(), 1);
        drop(journal);
        assert_eq!(
            gateway.inspect_selected_max_consultation().unwrap(),
            Some(active),
            "a closed owner still leaves the interrupted turn readable"
        );
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        // Reopening heals: the interrupted turn is retired, never replayed.
        let mut healed = ConsultationJournal::open(&store, &id).unwrap();
        assert_eq!(
            healed
                .take_recovery()
                .expect("reopen reports the abandoned turn")
                .abandoned_turn_id
                .as_deref(),
            Some("interrupted")
        );
        let settled = gateway
            .inspect_selected_max_consultation()
            .unwrap()
            .unwrap();
        assert_eq!(settled.consultation_id, id);
        assert!(settled.pending_turn_id.is_none());
        assert!(settled.retained_inputs.is_empty());
        drop(healed);
        assert_eq!(fs::read(&selection).unwrap(), selected_bytes);
        fs::write(&selection, b"{broken").unwrap();
        assert!(gateway.inspect_selected_max_consultation().is_err());
        assert_eq!(fs::read(&selection).unwrap(), b"{broken");
        fs::write(&selection, br#"{"thread_id":"../outside"}"#).unwrap();
        assert!(gateway.inspect_selected_max_consultation().is_err());
    }

    #[test]
    fn inspection_reads_retained_work_without_owner_or_disk_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let gateway = crate::agent::ThreadDeliveryGateway::new_in(dir.path()).unwrap();
        assert!(gateway.inspect_consultation("missing").unwrap().is_none());
        assert!(!dir.path().join("consultations").exists());
        assert!(gateway.inspect_consultation("../outside").is_err());
        let mut journal = ConsultationJournal::open(&store, "inspect").unwrap();
        accept(&mut journal, "one");
        accept(&mut journal, "two");
        journal.begin("one").unwrap();
        let path = dir.path().join("consultations/inspect.json");
        let before = fs::read(&path).unwrap();
        let snapshot = gateway.inspect_consultation("inspect").unwrap().unwrap();
        assert_eq!(snapshot.consultation_id, "inspect");
        assert_eq!(snapshot.pending_turn_id.as_deref(), Some("one"));
        assert_eq!(
            snapshot
                .retained_inputs
                .iter()
                .map(|entry| entry.turn_id.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(
            snapshot.retained_inputs[1].input.role,
            crate::agent::Role::User
        );
        assert_eq!(
            snapshot.retained_inputs[1].input.content,
            vec![crate::agent::ContentBlock::Text("instruction".into())]
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(
            ConsultationJournal::open(&store, "inspect").is_err(),
            "inspection did not take or release execution ownership"
        );
        drop(journal);
        assert_eq!(
            gateway.inspect_consultation("inspect").unwrap(),
            Some(snapshot)
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "inspection must not resolve uncertainty"
        );
        fs::write(&path, b"{broken").unwrap();
        assert!(gateway.inspect_consultation("inspect").is_err());
        assert_eq!(fs::read(path).unwrap(), b"{broken");
    }

    #[test]
    fn inspection_refuses_control_blocks_and_conflicting_pending_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let gateway = crate::agent::ThreadDeliveryGateway::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "invalid-input").unwrap();
        accept(&mut journal, "one");
        let original = journal.state.queued[0].clone();
        journal.state.queued[0].input.role = crate::agent::Role::Assistant;
        journal.persist().unwrap();
        assert!(gateway.inspect_consultation("invalid-input").is_err());
        journal.state.queued[0] = original;
        journal.state.pending = Some("other".into());
        journal.persist().unwrap();
        assert!(gateway.inspect_consultation("invalid-input").is_err());
        journal.state.pending = None;
        journal.state.queued[0].input.content = vec![crate::agent::ContentBlock::ToolUse {
            id: "call".into(),
            name: "execute".into(),
            input: serde_json::json!({}),
        }];
        journal.persist().unwrap();
        assert!(gateway.inspect_consultation("invalid-input").is_err());
    }

    #[test]
    fn queued_input_survives_owner_drop_without_becoming_execution_permission() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "waiting").unwrap();
        assert!(journal.begin("not-admitted").is_err());
        accept(&mut journal, "one");
        accept(&mut journal, "two");
        assert!(
            journal.begin("two").is_err(),
            "execution must follow persisted order"
        );
        assert!(
            journal.complete("one").is_err(),
            "acceptance is not completion"
        );
        let path = dir.path().join("consultations/waiting.json");
        drop(journal);
        let mut healed = ConsultationJournal::open(&store, "waiting").unwrap();
        let recovery = healed
            .take_recovery()
            .expect("waiting inputs are reported as dropped");
        assert!(
            recovery.abandoned_turn_id.is_none(),
            "nothing began, so nothing is abandoned"
        );
        assert_eq!(
            recovery
                .dropped_inputs
                .iter()
                .map(|dropped| (dropped.turn_id.as_str(), dropped.text_preview.as_str()))
                .collect::<Vec<_>>(),
            vec![("one", "instruction"), ("two", "instruction")]
        );
        let state: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(state["pending"], serde_json::Value::Null);
        assert!(state["queued"].as_array().unwrap().is_empty());
        assert!(
            state["abandoned"].as_array().unwrap().is_empty(),
            "an input that never began is dropped, not retired"
        );
        // Never begun is never executed, so the same identity may be resubmitted.
        accept(&mut healed, "one");
        healed.begin("one").unwrap();
    }

    #[test]
    fn never_begun_instruction_is_dropped_with_a_clipped_preview() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "preview").unwrap();
        let long = "słowo ".repeat(60);
        journal
            .accept_input(QueuedInstruction {
                turn_id: "wordy".into(),
                input: crate::agent::Message::new(
                    crate::agent::Role::User,
                    vec![
                        crate::agent::ContentBlock::Text(format!("  {long}\n")),
                        crate::agent::ContentBlock::Image {
                            data: vec![1, 2, 3],
                            media_type: "image/png".into(),
                        },
                    ],
                ),
                provider_name: "fixture".into(),
                options: serde_json::json!({}),
                group: None,
            })
            .unwrap();
        journal
            .accept_input(QueuedInstruction {
                turn_id: "wordless".into(),
                input: crate::agent::Message::new(
                    crate::agent::Role::User,
                    vec![crate::agent::ContentBlock::Image {
                        data: vec![4, 5],
                        media_type: "image/png".into(),
                    }],
                ),
                provider_name: "fixture".into(),
                options: serde_json::json!({}),
                group: None,
            })
            .unwrap();
        let recovery = journal
            .abandon_unfinished("explicit drop")
            .unwrap()
            .expect("waiting inputs are dropped");
        assert_eq!(recovery.reason, "explicit drop");
        assert!(recovery.abandoned_turn_id.is_none());
        assert_eq!(recovery.dropped_inputs.len(), 2);
        let wordy = &recovery.dropped_inputs[0].text_preview;
        assert_eq!(wordy.chars().count(), DROPPED_PREVIEW_CHARS);
        assert!(wordy.starts_with("słowo słowo"), "{wordy}");
        assert_eq!(
            recovery.dropped_inputs[1].text_preview, "",
            "an image carries no explanation for the reader"
        );
        assert!(
            journal.abandon_unfinished("again").unwrap().is_none(),
            "a settled journal has nothing left to abandon"
        );
    }

    #[test]
    fn journal_without_the_abandoned_key_loads_and_still_heals() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        drop(ConsultationJournal::open(&store, "legacy").unwrap());
        let path = dir.path().join("consultations/legacy.json");

        fs::write(
            &path,
            br#"{"completed":["done"],"pending":null,"queued":[]}"#,
        )
        .unwrap();
        let mut legacy = ConsultationJournal::open(&store, "legacy").unwrap();
        assert!(
            legacy.take_recovery().is_none(),
            "nothing was unfinished, so nothing is reported"
        );
        assert!(legacy.has_completed_turns());
        assert!(legacy.begin("done").is_err());
        drop(legacy);

        fs::write(
            &path,
            br#"{"completed":[],"pending":"interrupted","queued":[]}"#,
        )
        .unwrap();
        let mut migrated = ConsultationJournal::open(&store, "legacy").unwrap();
        assert_eq!(
            migrated
                .take_recovery()
                .expect("an old-shape pending turn is retired")
                .abandoned_turn_id
                .as_deref(),
            Some("interrupted")
        );
        assert!(
            !migrated.has_completed_turns(),
            "an abandoned turn wrote no history to require back"
        );
        assert!(migrated.begin("interrupted").is_err());
        drop(migrated);

        fs::write(&path, b"{broken").unwrap();
        assert!(
            ConsultationJournal::open(&store, "legacy").is_err(),
            "an unreadable journal is not an empty consultation"
        );
    }

    #[test]
    fn uncertain_acceptance_write_prevents_later_execution_or_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "write-error").unwrap();
        accept(&mut journal, "first");
        let path = journal.path.clone();
        let before = fs::read(&path).unwrap();
        let mut second = journal.state.queued[0].clone();
        second.turn_id = "second".into();
        journal.path = dir.path().join("absent-parent/state.json");
        assert!(journal.accept_input(second.clone()).is_err());
        journal.path = path.clone();
        assert!(journal.begin("first").is_err());
        assert!(journal.discard_unstarted("first").is_err());
        second.turn_id = "third".into();
        assert!(journal.accept_input(second).is_err());
        assert_eq!(
            fs::read(path).unwrap(),
            before,
            "uncertainty must not rewrite the last known durable input"
        );
        assert_eq!(
            journal.state.queued.len(),
            2,
            "retain the attempted input until explicit recovery"
        );
    }

    #[test]
    fn only_unstarted_front_can_be_discarded_and_resubmitted() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "refused").unwrap();
        accept(&mut journal, "one");
        assert!(journal.discard_unstarted("other").is_err());
        journal.discard_unstarted("one").unwrap();
        accept(&mut journal, "one");
        journal.begin("one").unwrap();
        assert!(journal.discard_unstarted("one").is_err());
        assert_eq!(journal.state.queued.len(), 1);
        journal.complete("one").unwrap();
        assert!(journal.state.queued.is_empty());
        drop(journal);
        assert!(ConsultationJournal::open(&store, "refused").is_ok());
    }

    #[test]
    fn explicit_reset_requires_closed_owner_and_preserves_unresolved_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let old = selected_id(&store).unwrap();
        let mut journal = ConsultationJournal::open(&store, &old).unwrap();
        accept(&mut journal, "unresolved");
        journal.begin("unresolved").unwrap();
        let journal_path = dir.path().join("consultations").join(format!("{old}.json"));
        let before = fs::read(&journal_path).unwrap();
        assert!(begin_new(&store, &old).is_err());
        assert_eq!(selected_id(&store).unwrap(), old);
        drop(journal);
        let new = begin_new(&store, &old).unwrap();
        assert_ne!(new, old);
        assert_eq!(selected_id(&store).unwrap(), new);
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        let mut reopened = ConsultationJournal::open(&store, &old).unwrap();
        assert_eq!(
            reopened
                .take_recovery()
                .expect("the old conversation heals when reopened")
                .abandoned_turn_id
                .as_deref(),
            Some("unresolved")
        );
        drop(reopened);
        assert!(
            begin_new(&store, &old).is_err(),
            "stale reset cannot replace newer selection"
        );
        assert_eq!(selected_id(&store).unwrap(), new);
    }

    #[test]
    fn selected_conversation_survives_reopen_and_pending_does_not_replace_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let id = selected_id(&store).unwrap();
        let mut journal = ConsultationJournal::open(&store, &id).unwrap();
        accept(&mut journal, "unresolved");
        journal.begin("unresolved").unwrap();
        drop(journal);
        drop(store);
        let reopened = ThreadStore::new_in(dir.path()).unwrap();
        assert_eq!(selected_id(&reopened).unwrap(), id);
        let mut healed = ConsultationJournal::open(&reopened, &id).unwrap();
        let recovery = healed
            .take_recovery()
            .expect("the selected conversation heals itself");
        assert_eq!(recovery.abandoned_turn_id.as_deref(), Some("unresolved"));
        assert_eq!(
            recovery
                .dropped_inputs
                .iter()
                .map(|dropped| dropped.turn_id.as_str())
                .collect::<Vec<_>>(),
            vec!["unresolved"]
        );
        assert!(healed.take_recovery().is_none(), "reported exactly once");
        let path = dir.path().join(format!("consultations/{id}.json"));
        let state: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(state["pending"], serde_json::Value::Null);
        assert!(state["queued"].as_array().unwrap().is_empty());
        assert!(state["completed"].as_array().unwrap().is_empty());
        assert_eq!(state["abandoned"], serde_json::json!(["unresolved"]));
        assert!(
            healed.begin("unresolved").is_err(),
            "an abandoned turn is never replayed"
        );
        assert!(
            try_accept(&mut healed, "unresolved").is_err(),
            "an abandoned turn is never re-admitted either"
        );
        accept(&mut healed, "next");
        healed.begin("next").unwrap();
        healed.complete("next").unwrap();
    }

    #[test]
    fn corrupt_selection_is_not_replaced_with_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        selected_id(&store).unwrap();
        let path = dir.path().join("consultations/selection/current.json");
        for invalid in [
            b"{broken".as_slice(),
            br#"{"thread_id":"../escape"}"#.as_slice(),
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(selected_id(&store).is_err());
            assert_eq!(fs::read(&path).unwrap(), invalid);
        }
    }

    #[test]
    fn owner_is_exclusive_and_unfinished_turn_is_abandoned_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "a").unwrap();
        assert!(ConsultationJournal::open(&store, "a").is_err());
        accept(&mut journal, "one");
        journal.begin("one").unwrap();
        drop(journal);
        let mut reopened = ConsultationJournal::open(&store, "a").unwrap();
        assert_eq!(
            reopened
                .take_recovery()
                .expect("reopen retires the unfinished turn")
                .abandoned_turn_id
                .as_deref(),
            Some("one")
        );
        assert!(reopened.begin("one").is_err());
        assert!(try_accept(&mut reopened, "one").is_err());
        accept(&mut reopened, "two");
        reopened.begin("two").unwrap();
        assert!(ConsultationJournal::open(&store, "a").is_err());
        assert!(ConsultationJournal::open(&store, "b").is_ok());
    }

    #[test]
    fn completed_turn_cannot_replay_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "a").unwrap();
        accept(&mut journal, "one");
        journal.begin("one").unwrap();
        journal.complete("one").unwrap();
        drop(journal);
        let mut reopened = ConsultationJournal::open(&store, "a").unwrap();
        assert!(reopened.begin("one").is_err());
        accept(&mut reopened, "two");
        reopened.begin("two").unwrap();
    }

    #[test]
    fn malformed_state_and_path_escape_are_not_empty_consultations() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        drop(ConsultationJournal::open(&store, "a").unwrap());
        fs::write(dir.path().join("consultations/a.json"), b"{broken").unwrap();
        assert!(ConsultationJournal::open(&store, "a").is_err());
        assert!(ConsultationJournal::open(&store, "../outside").is_err());
    }

    #[test]
    fn consultation_directory_escape_refuses_before_external_state_changes() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path()).unwrap();
        fs::write(outside.path().join("sentinel"), b"unchanged").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("consultations")).unwrap();

        assert!(selected_id(&store).is_err());
        assert!(ConsultationJournal::open(&store, "a").is_err());
        assert!(inspect_selected_max_consultation(&store).is_err());
        assert!(inspect_retained_input(&store, "a").is_err());
        assert_eq!(
            fs::read(outside.path().join("sentinel")).unwrap(),
            b"unchanged"
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    #[test]
    fn selection_directory_escape_preserves_external_selection() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path()).unwrap();
        let directory = root.path().join("consultations");
        fs::create_dir(&directory).unwrap();
        let original = br#"{"thread_id":"external"}"#;
        fs::write(outside.path().join("current.json"), original).unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.join("selection")).unwrap();

        assert!(selected_id(&store).is_err());
        assert!(begin_new(&store, "external").is_err());
        assert!(inspect_selected_max_consultation(&store).is_err());
        assert_eq!(
            fs::read(outside.path().join("current.json")).unwrap(),
            original
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    #[test]
    fn symlinked_state_files_are_not_admitted_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path()).unwrap();
        let path = selection_path(&store).unwrap();
        let selection = br#"{"thread_id":"external"}"#;
        let journal = br#"{"completed":[],"pending":null,"queued":[]}"#;
        fs::write(outside.path().join("selection.json"), selection).unwrap();
        fs::write(outside.path().join("journal.json"), journal).unwrap();
        std::os::unix::fs::symlink(outside.path().join("selection.json"), &path).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("journal.json"),
            root.path().join("consultations/external.json"),
        )
        .unwrap();

        assert!(selected_id(&store).is_err());
        assert!(begin_new(&store, "external").is_err());
        assert!(inspect_selected_max_consultation(&store).is_err());
        assert!(ConsultationJournal::open(&store, "external").is_err());
        assert!(inspect_retained_input(&store, "external").is_err());
        assert_eq!(
            fs::read(outside.path().join("selection.json")).unwrap(),
            selection
        );
        assert_eq!(
            fs::read(outside.path().join("journal.json")).unwrap(),
            journal
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 2);
    }
}
