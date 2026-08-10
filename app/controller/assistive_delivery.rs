//! Assistive voice→agent delivery lanes and context-bucket wire assembly.
//!
//! VoiceChat vs ActOnSelection (W10-D), combo capture with optional image
//! (W10-C), and raw/format paste wire that consumes selection tags but strips
//! vision markers (agent-only contract).

use anyhow::Result;

use crate::os::selection::{AssistiveContext, build_assistive_input};

use super::context_bucket::{ContextBucket, ContextMarker};

/// Combo capture with optional clipboard-image provider (W10-C).
/// `image_provider` returns PNG bytes when the pasteboard holds an image.
/// Tests pass `|| None` when only selection capture is under exercise.
pub(super) fn capture_combo_context_with_image(
    bucket: &mut ContextBucket,
    position: usize,
    provider: impl FnOnce() -> AssistiveContext,
    image_provider: impl FnOnce() -> Option<Vec<u8>>,
) -> Result<(AssistiveContext, Option<ContextMarker>)> {
    let mut context = provider();
    let marker = match context.selected_text.take() {
        Some(selected_text) => bucket.add_selection(position, selected_text)?,
        None => None,
    };
    if let Some(png) = image_provider() {
        let _ = bucket.add_image_png(&png)?;
    }
    Ok((context, marker))
}

/// Voice→agent prompt lane (W10-D).
///
/// - **VoiceChat**: clean assistive / armed hold without a selection transform.
///   Wire is spoken text (+ context tags / image markers). No assistive skeleton,
///   no `assistive.txt` persona.
/// - **ActOnSelection**: armed with a selection targeting a transformation.
///   Skeleton + `assistive.txt` persona remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssistiveLane {
    /// Spoken text goes to the agent as-is; no skeleton, no persona.
    VoiceChat,
    /// A selection is present and is the thing being transformed.
    ActOnSelection,
}

impl AssistiveLane {
    /// Whether this lane wraps the wire in the `assistive.txt` persona.
    pub(crate) fn use_assistive_persona(self) -> bool {
        matches!(self, Self::ActOnSelection)
    }
}

/// What the assistive path actually sends, plus how it was decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssistiveDelivery {
    /// The assembled message handed to the agent (skeleton and/or context tags).
    pub wire: String,
    /// Which lane produced [`Self::wire`].
    pub lane: AssistiveLane,
    /// The spoken transcript before any assembly — kept for display and logging.
    pub raw_transcript: String,
}

/// Pick the lane from whether a selection exists, wherever it currently lives.
///
/// A selection may still sit inline on the context (legacy path) or have already
/// moved into the bucket during combo capture; either counts.
pub(super) fn decide_assistive_lane(
    context: &AssistiveContext,
    bucket: &ContextBucket,
) -> AssistiveLane {
    // Selection may live on the context (legacy) or already be in the bucket
    // (capture_combo_context_with takes selected_text into the bucket).
    let has_inline_selection = context
        .selected_text
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty());
    if has_inline_selection || bucket.has_selection_items() {
        AssistiveLane::ActOnSelection
    } else {
        AssistiveLane::VoiceChat
    }
}

/// Build the outgoing agent message for whichever lane applies.
///
/// Both lanes append the bucket's context tags; only ActOnSelection prepends the
/// instruction skeleton, and it is given the bucket length so the header can
/// never claim "no selection" while `<selection_N>` tags ride below it.
pub(super) fn assemble_assistive_delivery_lane(
    transcript: &str,
    context: &AssistiveContext,
    bucket: &ContextBucket,
) -> AssistiveDelivery {
    let lane = decide_assistive_lane(context, bucket);
    let raw_transcript = transcript.to_string();
    let wire = match lane {
        AssistiveLane::ActOnSelection => {
            // Skeleton + selection/context fields; bucket tags append after.
            // The bucket truth flows into the header so the skeleton never
            // claims "no selection" while <selection_N> tags ride below.
            bucket.append_to_message(&build_assistive_input(transcript, context, bucket.len()))
        }
        AssistiveLane::VoiceChat => {
            // Spoken text only (+ optional context tags / image markers).
            // No USER_INSTRUCTION skeleton.
            bucket.append_to_message(transcript.trim())
        }
    };
    AssistiveDelivery {
        wire,
        lane,
        raw_transcript,
    }
}

/// Raw/format paste lane (channel 2): the finalized transcript pasted into the
/// frontmost app consumes the `ContextBucket` exactly like the assistive lanes
/// do — selections ride along as `<codescribe_context>` tags (inline under the
/// byte limit, `PATH:` for oversized spills). Images never enter the text
/// paste: the vision marker block is an agent contract, so the paste lane
/// skips it. An empty (or images-only) bucket leaves the transcript
/// byte-for-byte untouched.
pub(super) fn assemble_raw_paste_wire(transcript: &str, bucket: &ContextBucket) -> String {
    if !bucket.has_selection_items() {
        return transcript.to_string();
    }
    strip_trailing_image_marker_block(&bucket.append_to_message(transcript))
}

/// Drop the trailing vision-marker block appended by
/// `ContextBucket::append_to_message` when images share the bucket with
/// selections. Only a suffix that structurally matches the block (marker
/// header followed exclusively by `- <path>` lines) is stripped, so selection
/// payloads keep arbitrary content intact.
pub(super) fn strip_trailing_image_marker_block(wire: &str) -> String {
    let header = format!(
        "\n\n---\n{}\n",
        codescribe_core::attachment::IMAGE_PATHS_MARKER
    );
    if let Some(idx) = wire.rfind(&header) {
        let tail = &wire[idx + header.len()..];
        if !tail.is_empty() && tail.lines().all(|line| line.starts_with("- ")) {
            return wire[..idx].to_string();
        }
    }
    wire.to_string()
}
