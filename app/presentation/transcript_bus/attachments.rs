//! Capture attachments: files staged against an open channel capture, across
//! continuous segments (`-s1`, `-s2`), committed to every recipient bus.

use std::io::{self, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::envelope::*;

/// Parent `channel_session` identity of a channel message document.
/// Continuous voice sessions name each segment `<channel_session_id>-s<index>`,
/// while the session's `open` and terminal `sealed` rows carry `<channel_session_id>`.
pub(super) fn channel_session_id_of_document(session: &str) -> &str {
    if let Some((prefix, segment)) = session.rsplit_once("-s")
        && prefix.starts_with("agent-channel-")
        && !segment.is_empty()
        && segment.bytes().all(|b| b.is_ascii_digit())
        && segment.parse::<u32>().is_ok_and(|index| index >= 1)
    {
        return prefix;
    }
    session
}

/// Read an exact recent capture through the canonical generation/chunk decoder.
/// Missing old history is a refusal; an append target is never guessed.
pub(super) fn capture_attachment_source(
    path: &Path,
    session: &str,
    request: &serde_json::Value,
) -> io::Result<(Option<serde_json::Value>, bool)> {
    use crate::presentation::transcript_bus_maintenance::generation;
    use std::io::{BufRead, BufReader};
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture attachment target unavailable",
        )
    };
    generation::view(path)?;
    let mut reader = generation::Reader::open(path)?;
    // Bound lock-held history work. Current live captures must fit this window.
    let start = reader.len().saturating_sub(64 << 20);
    reader.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(reader);
    if start != 0 {
        let mut discarded = String::new();
        reader.read_line(&mut discarded)?;
    }
    let mut decoder = if start == 0 {
        generation::Decoder::default()
    } else {
        generation::Decoder::from_tail_boundary()
    };
    let channel_session = channel_session_id_of_document(session);
    let mut latest = None;
    let mut selected = None;
    let mut open = false;
    let mut seen_stage = false;
    let mut ended = false;
    let mut line = String::new();
    while reader.read_line(&mut line)? != 0 {
        if !line.ends_with('\n') {
            return Err(invalid());
        }
        let raw: serde_json::Value = serde_json::from_str(&line).map_err(|_| invalid())?;
        line.clear();
        for packed in decoder.push(raw)? {
            for row in generation::rows(packed)? {
                if row["schema"] == "codescribe.channel-session.v1" {
                    if row["session_id"] == session {
                        if row["state"] == "open" && !ended {
                            open = true;
                        } else {
                            open = false;
                            ended = true;
                        }
                    } else if row["session_id"] == channel_session {
                        if row["state"] == "open" && !ended {
                            open = true;
                        } else if row["state"] != "segment_sealed" {
                            open = false;
                            ended = true;
                        }
                    }
                }
                if row["session_id"] != session {
                    continue;
                }
                if row["schema"] == "codescribe.capture-attachment.v1"
                    && row["stage_id"] == request["stage_id"]
                {
                    if ["attachments", "capture_epoch", "source_sequence"]
                        .iter()
                        .any(|key| row[*key] != request[*key])
                    {
                        return Err(invalid());
                    }
                    seen_stage = true;
                }
                if row["status"] == "session_ended"
                    || row["reducer_action"] == "session_ended"
                    || row["schema"] == "codescribe.transcript-publication.v1"
                    || (row["status"] == "transcript_sealed" && row["terminal"] == true)
                {
                    ended = true;
                    open = false;
                }
                if row["schema"] == "codescribe.transcript-evidence.v1" {
                    if row["capture_epoch"] == request["capture_epoch"]
                        && row["sequence"] == request["source_sequence"]
                    {
                        selected = Some(row.clone());
                    }
                    latest = Some(row);
                }
            }
        }
    }
    if decoder.incomplete() {
        return Err(invalid());
    }
    // The UI and the append owner observe the same evolving capture at
    // different times. Continued speech may advance sequence, revision and
    // occurrence while a pasted file is being described. Revalidate the exact
    // selected row, not a demand that speaking stop until the append finishes.
    // A different epoch/address or the capture's close still ends admission.
    // A ledger terminal observation only seals a segment of an open capture.
    let same_capture = selected
        .as_ref()
        .zip(latest.as_ref())
        .is_some_and(|(selected, latest)| {
            ["capture_epoch", "audience", "recipients"]
                .iter()
                .all(|key| selected[*key] == latest[*key])
        });
    if !open || ended || !same_capture {
        return Ok((None, seen_stage));
    }
    Ok((selected, seen_stage))
}

/// Attachment staging consumes the capture's frozen address; it cannot create
/// transcript text, a recorder, an occurrence or a new routing decision.
pub(super) fn append_capture_attachment(
    path: &Path,
    request: &serde_json::Value,
) -> io::Result<serde_json::Value> {
    use crate::presentation::transcript_bus_maintenance::generation;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture attachment refused; target closed or changed",
        )
    };
    if path.canonicalize()?.as_path() != path {
        return Err(invalid());
    }
    let session = request["session_id"]
        .as_str()
        .filter(|s| s.starts_with("agent-channel-"))
        .ok_or_else(invalid)?;
    let stage_id = request["stage_id"]
        .as_str()
        .filter(|s| {
            s.len() == 24
                && s.bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
        .ok_or_else(invalid)?;
    let epoch = request["capture_epoch"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    let sequence = request["source_sequence"].as_u64().ok_or_else(invalid)?;
    if request["kind"] != "capture_attachment"
        || !path.is_absolute()
        || request["attachments"]
            .as_array()
            .is_none_or(|items| items.is_empty() || items.len() > 64)
        || chrono::DateTime::parse_from_rfc3339(request["emitted_at"].as_str().ok_or_else(invalid)?)
            .is_err()
    {
        return Err(invalid());
    }
    for item in request["attachments"].as_array().ok_or_else(invalid)? {
        let file = item["path"].as_str().map(Path::new).ok_or_else(invalid)?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(invalid)?;
        let canonical = file.canonicalize()?;
        if !file.is_absolute()
            || !canonical.starts_with(home.canonicalize()?)
            || !canonical.is_file()
            || item["name"].as_str().is_none_or(str::is_empty)
            || item["media_type"].as_str().is_none_or(str::is_empty)
            || item["bytes"]
                .as_u64()
                .is_none_or(|n| n == 0 || n > 50 * 1024 * 1024)
            || item["bytes"].as_u64() != Some(canonical.metadata()?.len())
            || item["sha256"]
                .as_str()
                .is_none_or(|s| s.len() != 64 || !s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
    }
    let (initial, _) = capture_attachment_source(path, session, request)?;
    let initial = initial.ok_or_else(invalid)?;
    let recipients = initial["recipients"]
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(invalid)?;
    let mut paths = vec![path.to_path_buf()];
    for recipient in recipients {
        let bus = PathBuf::from(recipient["bus"].as_str().ok_or_else(invalid)?);
        if !bus.is_absolute() || bus.canonicalize()? != bus {
            return Err(invalid());
        }
        paths.push(bus);
    }
    paths.sort();
    paths.dedup();
    let destinations = paths
        .iter()
        .map(|path| Ok((path.clone(), shared_bus_file(path)?)))
        .collect::<io::Result<Vec<_>>>()?;
    let mut staged = serde_json::json!({
        "schema":"codescribe.capture-attachment.v1", "kind":"capture_attachment",
        "stage_id":stage_id, "session_id":session, "capture_epoch":epoch,
        "source_sequence":sequence, "emitted_at":request["emitted_at"],
        "attachments":request["attachments"], "commit_bus":path,
    });
    let commit = serde_json::json!({
        "schema":"codescribe.capture-attachment-commit.v1", "kind":"capture_attachment_commit",
        "stage_id":stage_id, "session_id":session, "capture_epoch":epoch,
        "source_sequence":sequence, "commit_bus":path, "emitted_at":request["emitted_at"],
    });
    let primary = paths
        .iter()
        .position(|candidate| candidate == path)
        .ok_or_else(invalid)?;
    let receipts = generation::append_durable_many_with(
        &destinations,
        || {
            let mut known = Vec::new();
            for bus in &paths {
                let (source, exists) = capture_attachment_source(bus, session, request)?;
                let source = source.ok_or_else(invalid)?;
                if source["capture_epoch"].as_u64() != Some(epoch)
                    || source["sequence"].as_u64() != Some(sequence)
                    || source["recipients"] != initial["recipients"]
                    || source["audience"] != initial["audience"]
                    || source["reducer_revision"] != initial["reducer_revision"]
                    || source["occurrence_session_id"] != initial["occurrence_session_id"]
                    || source["sample_start"] != initial["sample_start"]
                    || source["sample_end"] != initial["sample_end"]
                    || source["document_index"] != initial["document_index"]
                {
                    return Err(invalid());
                }
                known.push(exists);
            }
            for key in [
                "channel",
                "audience",
                "recipients",
                "occurrence_session_id",
                "sample_start",
                "sample_end",
                "document_index",
                "reducer_revision",
            ] {
                staged[key] = initial[key].clone();
            }
            let bytes = generation::encode(&staged)?;
            Ok(known
                .into_iter()
                .map(|exists| if exists { None } else { Some(bytes.clone()) })
                .collect())
        },
        Some((primary, generation::encode(&commit)?)),
    )?;
    Ok(
        serde_json::json!({"stage_id":stage_id, "session_id":session, "capture_epoch":epoch,
        "source_sequence":sequence, "sources":receipts, "buses":paths}),
    )
}
