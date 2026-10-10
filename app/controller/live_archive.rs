//! Durable history of the live take.
//!
//! The reducer's document timeline is the one authority while the take is
//! current; it lives in memory and ends with the next take or a restart. The
//! archived `.txt` of the same take carries a revision chain that a reopened
//! take replays. This mirror admits every accepted live step and every cursor
//! move into that chain, through the chain's own head CAS and evidence
//! binding, so the reopened take shows the same versions and the same
//! selection. It never decides anything: it copies what the reducer accepted,
//! in order, and stops visibly the moment the chain holds a write it did not
//! make.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use codescribe_core::state::history::{self, ArchiveRevisionProvenance};

use crate::presentation::emitter::DocumentVersionStep;

/// Identity of one live step: the receipt that accepted it and when. Bytes
/// are never part of it, so two equal attempts stay two steps.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StepKey {
    receipt_id: String,
    emitted_at: String,
    provenance: String,
}

impl StepKey {
    fn of(step: &DocumentVersionStep) -> Self {
        Self {
            receipt_id: step.receipt_id.clone(),
            emitted_at: step.emitted_at.clone(),
            provenance: step.provenance.clone(),
        }
    }
}

/// One live step already in the chain, with the chain revision that holds it
/// (0 is the archived original itself).
#[derive(Debug, Clone)]
struct Mirrored {
    key: StepKey,
    revision: u64,
}

/// The link from one live take to its archived transcript.
#[derive(Debug)]
pub(crate) struct LiveArchiveMirror {
    session_id: String,
    transcript: PathBuf,
    /// Live step selected when the take was archived: the bytes the `.txt`
    /// was written from. Decides, once, where the original sits in history.
    archived_step: usize,
    /// Chain head this mirror wrote last; `None` before the first sync.
    head: Option<u64>,
    mirrored: Vec<Mirrored>,
    /// Why the mirror stopped. Once set, nothing more is written.
    diverged: Option<String>,
}

impl LiveArchiveMirror {
    pub(crate) fn link(session_id: String, transcript: PathBuf, archived_step: usize) -> Self {
        Self {
            session_id,
            transcript,
            archived_step,
            head: None,
            mirrored: Vec::new(),
            diverged: None,
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn transcript(&self) -> &Path {
        &self.transcript
    }

    pub(crate) fn diverged(&self) -> Option<&str> {
        self.diverged.as_deref()
    }

    /// Bring the chain up to the live timeline `steps` with `cursor`
    /// selected. A failure stops the mirror for good and is returned, so the
    /// caller reports it; the live take itself is untouched.
    pub(crate) fn sync(&mut self, steps: &[DocumentVersionStep], cursor: usize) -> Result<()> {
        if let Some(reason) = &self.diverged {
            anyhow::bail!("live history is no longer mirrored to the archive: {reason}");
        }
        if steps.is_empty() {
            // The reducer holds another take or none: nothing of this take
            // can change any more.
            return Ok(());
        }
        let result = self.sync_inner(steps, cursor);
        if let Err(error) = &result {
            self.diverged = Some(format!("{error:#}"));
        }
        result
    }

    fn sync_inner(&mut self, steps: &[DocumentVersionStep], cursor: usize) -> Result<()> {
        let document = history::read_archived_document(&self.transcript)?;
        let mut head = match self.head {
            Some(expected) => {
                anyhow::ensure!(
                    document.head_revision() == expected,
                    "the archived history of this take changed outside the live take (head {} instead of {expected})",
                    document.head_revision()
                );
                expected
            }
            None => self.seed(&document, steps)?,
        };

        // First live step the chain does not hold yet. Steps the reducer
        // dropped (a redo branch ended by a new operation) stop matching here.
        let common = self
            .mirrored
            .iter()
            .zip(steps)
            .take_while(|(mirrored, step)| mirrored.key == StepKey::of(step))
            .count();
        if common < steps.len() {
            // The chain truncates a redo branch exactly as the reducer did:
            // select the step the new operation was made on, then append it.
            let anchor = match common {
                0 => 0,
                index => self.mirrored[index - 1].revision,
            };
            self.mirrored.truncate(common);
            head = self.select(head, anchor)?;
            for step in &steps[common..] {
                let (provenance, detail) = archive_provenance(step);
                let record = history::admit_live_revision(
                    &self.transcript,
                    head,
                    &step.rendered_text,
                    provenance,
                    detail,
                    false,
                )
                .with_context(|| {
                    format!(
                        "live {} step was not admitted to the archive",
                        step.provenance
                    )
                })?;
                head = record.revision;
                self.head = Some(head);
                self.mirrored.push(Mirrored {
                    key: StepKey::of(step),
                    revision: head,
                });
            }
        }

        let target = self
            .mirrored
            .get(cursor)
            .map(|mirrored| mirrored.revision)
            .context("the selected live version has no archived step")?;
        self.select(head, target)?;
        Ok(())
    }

    /// Place the archived original in the live history, once. When the
    /// archived bytes are the step selected at Stop, that step IS the
    /// original and the steps before it lead the chain; otherwise the
    /// original stays its own first version and every live step follows it.
    /// The comparison only identifies the original; it never merges steps.
    fn seed(
        &mut self,
        document: &history::ArchivedDocument,
        steps: &[DocumentVersionStep],
    ) -> Result<u64> {
        anyhow::ensure!(
            document.revisions.is_empty(),
            "the archived transcript already has a history the live take did not write"
        );
        // Position decides; the byte check only refuses to call a version
        // the original when the archive was written from something else.
        let original = Some(self.archived_step).filter(|&index| {
            steps
                .get(index)
                .is_some_and(|step| step.rendered_text.trim() == document.original_text.trim())
        });
        let mut head = 0;
        self.head = Some(head);
        if let Some(original) = original {
            for step in &steps[..original] {
                let (provenance, detail) = archive_provenance(step);
                let record = history::admit_live_revision(
                    &self.transcript,
                    head,
                    &step.rendered_text,
                    provenance,
                    detail,
                    true,
                )
                .with_context(|| {
                    format!(
                        "live {} step before the archived transcript was not admitted",
                        step.provenance
                    )
                })?;
                head = record.revision;
                self.head = Some(head);
                self.mirrored.push(Mirrored {
                    key: StepKey::of(step),
                    revision: head,
                });
            }
            self.mirrored.push(Mirrored {
                key: StepKey::of(&steps[original]),
                revision: 0,
            });
        } else {
            tracing::warn!(
                transcript = %self.transcript.display(),
                "archived transcript is not a live version of this take; it stays the first version"
            );
        }
        Ok(head)
    }

    /// Select chain step `target` unless it is already selected; returns the
    /// new head.
    fn select(&mut self, head: u64, target: u64) -> Result<u64> {
        let document = history::read_archived_document(&self.transcript)?;
        if document.timeline().selected_revision() == target {
            return Ok(head);
        }
        let record = history::navigate_archived_revision(&self.transcript, head, target)
            .context("the selected live version was not selected in the archive")?;
        self.head = Some(record.revision);
        Ok(record.revision)
    }
}

/// Live provenance → chain provenance. Raw and Light+ are the take's own
/// transcript; the detail keeps which one.
fn archive_provenance(step: &DocumentVersionStep) -> (ArchiveRevisionProvenance, Option<String>) {
    match step.provenance.as_str() {
        "user-edit" => (ArchiveRevisionProvenance::UserEdit, None),
        "retranscribe" => (ArchiveRevisionProvenance::Retranscribe, step.detail.clone()),
        "formatter" => (ArchiveRevisionProvenance::Formatter, step.detail.clone()),
        "raw" => (ArchiveRevisionProvenance::Transcript, None),
        other => (
            ArchiveRevisionProvenance::Transcript,
            Some(other.to_string()),
        ),
    }
}
