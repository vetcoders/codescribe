//! Crash boundary for consultation execution. Unfinished input is retained here;
//! completed conversation history remains exclusively in ThreadStore. Pending
//! means potentially executed, never permission to replay after a restart.

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

/// Conversation selection is thread state, not an ASR/provider setting. Only
/// first use mints an id; corrupt state and unresolved turns never choose a
/// different conversation as a side effect of recovery.
pub(crate) fn selected_id(store: &ThreadStore) -> Result<String> {
    let path = selection_path(store)?;
    let _selection_owner = exclusive_owner(&path.with_file_name("owner.lock"))?;
    let selected: SelectedConsultation = match OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW).open(&path) {
        Ok(file) => serde_json::from_reader(file).context("corrupt Max consultation selection")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let selected = SelectedConsultation { thread_id: ThreadStore::generate_id() };
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
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path)?;
    let selected: SelectedConsultation = serde_json::from_reader(file).context("corrupt Max consultation selection")?;
    ensure!(selected.thread_id == expected, "Max consultation selection changed; refusing stale reset");
    // Caller must first close its owner. Another process still owning that
    // conversation also prevents reset, regardless of its apparent UI state.
    let directory = consultation_directory(store)?;
    let _old_owner = exclusive_owner(&directory.join(format!("{expected}.lock")))?;
    let replacement = SelectedConsultation { thread_id: ThreadStore::generate_id() };
    persist_json(&path, &replacement)?;
    Ok(replacement.thread_id)
}

fn selection_path(store: &ThreadStore) -> Result<PathBuf> {
    let directory = consultation_directory(store)?;
    let selection_dir = directory.join("selection");
    fs::create_dir_all(&selection_dir)?;
    File::open(&directory)?.sync_all()?;
    let selection_dir = canonical_existing_child(&directory, &selection_dir)?;
    Ok(selection_dir.join("current.json"))
}

fn consultation_directory(store: &ThreadStore) -> Result<PathBuf> {
    let directory = store.threads_dir.join("consultations");
    fs::create_dir_all(&directory)?;
    File::open(&store.threads_dir)?.sync_all()?;
    canonical_existing_child(&store.threads_dir, &directory)
}

fn exclusive_owner(path: &Path) -> Result<File> {
    let owner = OpenOptions::new().read(true).write(true).create(true)
        .truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)?;
    // SAFETY: owner holds a live file descriptor for the full lease lifetime.
    let acquired = unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    ensure!(acquired == 0, "consultation already owned or lock unavailable: {}", std::io::Error::last_os_error());
    Ok(owner)
}

fn persist_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(path.parent().context("consultation state parent")?)?.sync_all()?;
    Ok(())
}

pub(crate) struct ConsultationJournal {
    path: PathBuf,
    state: AdmissionState,
    write_uncertain: bool,
    // Kernel ownership dies with the process; the lock file must not be unlinked.
    _owner: File,
}

impl ConsultationJournal {
    pub(crate) fn has_completed_turns(&self) -> bool {
        !self.state.completed.is_empty()
    }

    pub(crate) fn open(store: &ThreadStore, id: &str) -> Result<Self> {
        validate_thread_id(id)?;
        let directory = consultation_directory(store)?;
        let owner = exclusive_owner(&directory.join(format!("{id}.lock")))?;
        let path = directory.join(format!("{id}.json"));
        let state = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path) {
            Ok(file) => serde_json::from_reader(file).context("corrupt consultation admission state")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AdmissionState::default(),
            Err(error) => return Err(error).context("read consultation admission state"),
        };
        let journal = Self { path, state, write_uncertain: false, _owner: owner };
        ensure!(journal.state.pending.is_none(), "consultation requires recovery; unresolved turn {:?}", journal.state.pending);
        ensure!(journal.state.queued.is_empty(), "consultation requires recovery; retained waiting instructions");
        Ok(journal)
    }

    pub(crate) fn accept_input(&mut self, input: QueuedInstruction) -> Result<()> {
        ensure!(!self.write_uncertain, "consultation journal write requires recovery");
        ensure!(!input.turn_id.trim().is_empty(), "turn identity is required");
        ensure!(!self.state.completed.contains(&input.turn_id)
            && !self.state.queued.iter().any(|entry| entry.turn_id == input.turn_id),
            "turn already admitted; refusing replay");
        self.state.queued.push(input);
        self.persist()
    }

    /// Only before begin, when the owner proves no provider/tool work started.
    pub(crate) fn discard_unstarted(&mut self, turn: &str) -> Result<()> {
        ensure!(!self.write_uncertain, "consultation journal write requires recovery");
        ensure!(self.state.pending.is_none(), "cannot discard a potentially executed turn");
        ensure!(self.state.queued.first().is_some_and(|entry| entry.turn_id == turn),
            "consultation discard is out of order");
        let input = self.state.queued.remove(0);
        if let Err(error) = self.persist() {
            self.state.queued.insert(0, input);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn begin(&mut self, turn: &str) -> Result<()> {
        ensure!(!self.write_uncertain, "consultation journal write requires recovery");
        ensure!(self.state.pending.is_none(), "consultation requires recovery");
        ensure!(!self.state.completed.contains(turn), "turn {turn} already completed; refusing replay");
        ensure!(self.state.queued.first().is_some_and(|entry| entry.turn_id == turn),
            "consultation execution is not the admitted front");
        self.state.pending = Some(turn.to_string());
        // Failure deliberately leaves in-memory pending set: do not execute
        // anything if the before-effects receipt could not be made durable.
        self.persist()
    }

    pub(crate) fn complete(&mut self, turn: &str) -> Result<()> {
        ensure!(!self.write_uncertain, "consultation journal write requires recovery");
        ensure!(self.state.pending.as_deref() == Some(turn), "consultation completion identity mismatch");
        ensure!(self.state.queued.first().is_some_and(|entry| entry.turn_id == turn),
            "consultation completion is not the admitted front");
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
        if result.is_err() { self.write_uncertain = true; }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(journal: &mut ConsultationJournal, turn: &str) {
        journal.accept_input(QueuedInstruction {
            turn_id: turn.into(),
            input: crate::agent::Message::new(crate::agent::Role::User,
                vec![crate::agent::ContentBlock::Text("instruction".into())]),
            provider_name: "fixture".into(), options: serde_json::json!({}), group: None,
        }).unwrap();
    }

    #[test]
    fn queued_input_survives_owner_drop_without_becoming_execution_permission() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "waiting").unwrap();
        assert!(journal.begin("not-admitted").is_err());
        accept(&mut journal, "one");
        accept(&mut journal, "two");
        assert!(journal.begin("two").is_err(), "execution must follow persisted order");
        assert!(journal.complete("one").is_err(), "acceptance is not completion");
        let path = dir.path().join("consultations/waiting.json");
        let before = fs::read(&path).unwrap();
        drop(journal);
        assert!(ConsultationJournal::open(&store, "waiting").is_err());
        assert_eq!(fs::read(path).unwrap(), before, "recovery refusal preserves every input");
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
        assert_eq!(fs::read(path).unwrap(), before, "uncertainty must not rewrite the last known durable input");
        assert_eq!(journal.state.queued.len(), 2, "retain the attempted input until explicit recovery");
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
        assert!(ConsultationJournal::open(&store, &old).is_err());
        assert!(begin_new(&store, &old).is_err(), "stale reset cannot replace newer selection");
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
        assert!(ConsultationJournal::open(&reopened, &id).is_err());
    }

    #[test]
    fn corrupt_selection_is_not_replaced_with_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        selected_id(&store).unwrap();
        let path = dir.path().join("consultations/selection/current.json");
        for invalid in [b"{broken".as_slice(), br#"{"thread_id":"../escape"}"#.as_slice()] {
            fs::write(&path, invalid).unwrap();
            assert!(selected_id(&store).is_err());
            assert_eq!(fs::read(&path).unwrap(), invalid);
        }
    }

    #[test]
    fn owner_is_exclusive_and_unfinished_turn_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "a").unwrap();
        assert!(ConsultationJournal::open(&store, "a").is_err());
        accept(&mut journal, "one");
        journal.begin("one").unwrap();
        drop(journal);
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
}
