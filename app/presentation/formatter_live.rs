//! Live formatted view of a take that is still capturing.
//!
//! Accepted Correction and Smart fragments already reach the reducer as
//! source-bound derived versions. This module composes them with the current
//! canvas into one whole document and publishes that document through the
//! derived-projection callback: formatted text where a fragment's own source
//! is still current, the canvas — Raw, Light+ shape, provisional tail —
//! everywhere else.
//!
//! The view is presentation only. It is never pushed into the reducer's
//! derived history, never journaled, never selected for delivery and never
//! advances a Raw revision, seal or occurrence. Stop keeps choosing between the
//! complete formatted take and the complete Raw floor in
//! `PresentationEmitter::delivery_projection`; once that selection exists, or
//! the take is terminal, this view stands down.

use std::sync::Mutex;

use codescribe_core::config::FormattingPolicy;
use codescribe_core::pipeline::contracts::EngineEvent;

use super::{
    DerivedTranscriptProjection, PresentationEmitter, TranscriptDocumentEntry, TranscriptReducer,
    VisibleWordSource, append_exact_fragment,
};

/// Delivery selection only trusts `raw_document` whole versions; a live view
/// carries its own state so it can never be mistaken for one.
const LIVE_SOURCE_STATE: &str = "live_view";

/// Live revisions sit above the derived-history band (`1 << 63` plus a count),
/// so a live view never shares a revision number with a minted version.
const LIVE_REVISION_BAND: u64 = (1 << 63) | (1 << 62);

/// What the emitter remembers between views: the policy of the latest
/// proposal, the last view number and the exact text last published.
#[derive(Default)]
pub(super) struct LiveViewMemo {
    inner: Mutex<LiveViewState>,
}

#[derive(Default)]
struct LiveViewState {
    policy: Option<FormattingPolicy>,
    sequence: u64,
    published: Option<String>,
}

/// Republishes the live view once an engine event has been fully handled.
/// Created first in `on_event`, so it drops last — after every arm's ledger
/// and reducer guard — and covers early returns too. New Raw words after the
/// last formatter reply keep their place in the whole view this way.
pub(super) struct LiveViewRefresh<'emitter> {
    emitter: &'emitter PresentationEmitter,
}

impl<'emitter> LiveViewRefresh<'emitter> {
    pub(super) fn after(emitter: &'emitter PresentationEmitter, event: &EngineEvent) -> Self {
        // A proposal arms the view with its policy; no proposal, no view.
        if let EngineEvent::OccurrenceLabelProposal { proposal } = event
            && matches!(
                proposal.policy,
                FormattingPolicy::Correction | FormattingPolicy::Smart
            )
        {
            emitter
                .live_view
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .policy = Some(proposal.policy);
        }
        Self { emitter }
    }
}

impl Drop for LiveViewRefresh<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.emitter.refresh_live_view();
        }
    }
}

impl PresentationEmitter {
    /// Compose and publish the whole view when it changed. The memo lock only
    /// orders consecutive views; it is never held together with the ledger,
    /// and the reducer lock is released before the callback runs.
    fn refresh_live_view(&self) {
        let (Some(bus), Some(callback)) = (&self.transcript_bus, &self.projection_callback) else {
            return;
        };
        let mut memo = self
            .live_view
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(policy) = memo.policy else {
            return;
        };
        let take = self
            .cursor_capture
            .get()
            .map(|(session, epoch)| (session.as_str(), *epoch));
        let sequence = memo.sequence + 1;
        let view = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .live_formatted_view(policy, take, sequence);
        let Some(view) = view else {
            // Nothing formatted is current any more: the next view may repeat
            // an earlier text and must not be mistaken for a duplicate.
            memo.published = None;
            return;
        };
        if memo.published.as_deref() == Some(view.rendered_text.as_str()) {
            return;
        }
        let Some(event) = bus.derived_presentation_event(&view) else {
            return;
        };
        memo.sequence = sequence;
        memo.published = Some(view.rendered_text);
        self.with_active_presentation(|| callback(&event));
    }
}

impl TranscriptReducer {
    /// The latest derived version of one document entry's own label, under the
    /// exact observation that produced it. A version whose source label,
    /// observation receipt or manual-edit state no longer matches the entry is
    /// not found. Delivery selection, the Stop wait and the live view share
    /// this one predicate.
    pub(super) fn latest_source_projection(
        &self,
        entry: &TranscriptDocumentEntry,
        policy: FormattingPolicy,
    ) -> Option<&DerivedTranscriptProjection> {
        self.derived_projections.iter().rev().find(|projection| {
            projection.requested_mode == policy.as_str()
                && projection
                    .source_observation
                    .as_ref()
                    .is_some_and(|source| {
                        source.occurrence == entry.occurrence
                            && projection.source_raw_text == entry.label
                            && entry.manual_edit_receipt.is_none()
                            && entry.observation_receipt.starts_with(&format!(
                                "{}:{}:{}:",
                                source.producer.as_str(),
                                source.request,
                                source.generation
                            ))
                    })
        })
    }

    /// Whole visible document with every currently valid formatted fragment in
    /// its occurrence's place and the canvas everywhere else. `None` when the
    /// take cannot be shown this way or when no fragment would change the
    /// canvas: an empty, refused, failed, stale or edited fragment never
    /// replaces its words, and nothing here can blank the document.
    ///
    /// `take` is the opened capture's session and epoch when the emitter knows
    /// them; every document entry must belong to it.
    pub(super) fn live_formatted_view(
        &self,
        policy: FormattingPolicy,
        take: Option<(&str, u64)>,
        sequence: u64,
    ) -> Option<DerivedTranscriptProjection> {
        // Terminal, selected, hand-edited or marker-anchored documents belong
        // to their own corridors: markers are positioned against Raw offsets
        // that a rewritten fragment would move.
        if !matches!(
            policy,
            FormattingPolicy::Correction | FormattingPolicy::Smart
        ) || self.terminal
            || self.terminal_sealed
            || self.delivery_selection.is_some()
            || self.raw_human_revision
            || self.manual_rendered_text.is_some()
            || !self.context_markers.is_empty()
            || self.document_by_occurrence.is_empty()
            || !self.direct_document.is_empty()
        {
            return None;
        }
        let first = self.document_by_occurrence.keys().next()?;
        let (session, epoch) = (first.session.clone(), first.capture_epoch);
        if self.document_by_occurrence.keys().any(|occurrence| {
            occurrence.session != session
                || occurrence.capture_epoch != epoch
                || take.is_some_and(|(open, open_epoch)| {
                    occurrence.session != open || occurrence.capture_epoch != open_epoch
                })
        }) {
            return None;
        }
        let mut view = String::new();
        let mut canvas = String::new();
        let mut formatted = 0_usize;
        for (_, text, source, _) in self.read_paint_fragments() {
            let accepted = match &source {
                VisibleWordSource::Committed(paint) => self
                    .document_by_occurrence
                    .get(&paint.occurrence)
                    .and_then(|entry| self.latest_source_projection(entry, policy))
                    .filter(|projection| {
                        projection.status == "applied"
                            && !projection.rendered_text.trim().is_empty()
                    }),
                _ => None,
            };
            append_exact_fragment(&mut canvas, &text);
            match accepted {
                Some(projection) => {
                    formatted += 1;
                    append_exact_fragment(&mut view, &projection.rendered_text);
                }
                None => append_exact_fragment(&mut view, &text),
            }
        }
        if formatted == 0 || view.trim().is_empty() || view == canvas {
            return None;
        }
        let mut projection = DerivedTranscriptProjection {
            schema: "codescribe.derived-transcript.v1".into(),
            emitted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            session_id: session.clone(),
            revision: LIVE_REVISION_BAND | sequence,
            source_raw_revision: self.revision,
            source_raw_text: self.committed_rendered_text(),
            source_state: LIVE_SOURCE_STATE.into(),
            requested_mode: policy.as_str().into(),
            delivered_mode: policy.as_str().into(),
            rendered_text: view,
            status: "applied".into(),
            receipt_id: format!("formatter-live-{session}-{}-{sequence}", self.revision),
            source_pcm: None,
            source_observation_id: None,
            source_observation: None,
            publication_digest: [0; 32],
        };
        projection.publication_digest = projection.digest();
        Some(projection)
    }
}
