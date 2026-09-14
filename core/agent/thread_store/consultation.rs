//! Crash boundary for consultation execution. This stores turn identities only;
//! messages remain exclusively in ThreadStore. Pending means potentially
//! executed, never permission to replay after a restart.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{ThreadStore, canonical_existing_child, validate_thread_id};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionState {
    completed: BTreeSet<String>,
    pending: Option<String>,
}

pub(crate) struct ConsultationJournal {
    path: PathBuf,
    state: AdmissionState,
    // Kernel ownership dies with the process; the lock file must not be unlinked.
    _owner: File,
}

impl ConsultationJournal {
    pub(crate) fn open(store: &ThreadStore, id: &str) -> Result<Self> {
        validate_thread_id(id)?;
        let directory = store.threads_dir.join("consultations");
        fs::create_dir_all(&directory)?;
        File::open(&store.threads_dir)?.sync_all()?;
        let directory = canonical_existing_child(&store.threads_dir, &directory)?;
        let owner = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW)
            .open(directory.join(format!("{id}.lock")))?;
        // SAFETY: owner holds a live file descriptor for the full lease lifetime.
        let acquired = unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(acquired == 0, "consultation {id} already owned or lock unavailable: {}", std::io::Error::last_os_error());
        let path = directory.join(format!("{id}.json"));
        let state = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path) {
            Ok(file) => serde_json::from_reader(file).context("corrupt consultation admission state")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AdmissionState::default(),
            Err(error) => return Err(error).context("read consultation admission state"),
        };
        let journal = Self { path, state, _owner: owner };
        ensure!(journal.state.pending.is_none(), "consultation requires recovery; unresolved turn {:?}", journal.state.pending);
        Ok(journal)
    }

    pub(crate) fn begin(&mut self, turn: &str) -> Result<()> {
        ensure!(self.state.pending.is_none(), "consultation requires recovery");
        ensure!(!self.state.completed.contains(turn), "turn {turn} already completed; refusing replay");
        self.state.pending = Some(turn.to_string());
        // Failure deliberately leaves in-memory pending set: do not execute
        // anything if the before-effects receipt could not be made durable.
        self.persist()
    }

    pub(crate) fn complete(&mut self, turn: &str) -> Result<()> {
        ensure!(self.state.pending.as_deref() == Some(turn), "consultation completion identity mismatch");
        self.state.completed.insert(turn.to_string());
        self.state.pending = None;
        if let Err(error) = self.persist() {
            self.state.pending = Some(turn.to_string());
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        let temporary = self.path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(&serde_json::to_vec(&self.state)?)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path)?;
        File::open(self.path.parent().context("journal parent")?)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_is_exclusive_and_unfinished_turn_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(dir.path()).unwrap();
        let mut journal = ConsultationJournal::open(&store, "a").unwrap();
        assert!(ConsultationJournal::open(&store, "a").is_err());
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
        journal.begin("one").unwrap();
        journal.complete("one").unwrap();
        drop(journal);
        let mut reopened = ConsultationJournal::open(&store, "a").unwrap();
        assert!(reopened.begin("one").is_err());
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
