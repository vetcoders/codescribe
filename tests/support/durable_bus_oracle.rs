//! Independent storage oracle for small, synthetic Bus fixtures.
//! It never calls the production generation reader or occurrence decoder.
//! Large chunked documents have separate byte/checksum acceptance controls.

use serde_json::Value;
use std::fs;
use std::path::Path;

pub(crate) fn logical_text(path: &Path) -> String {
    let manifest_path = format!("{}.generations.json", path.display());
    let bytes = match fs::read(&manifest_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("fixture journal: {error}"),
        },
        Err(error) => panic!("fixture manifest: {error}"),
        Ok(bytes) => {
            let manifest: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(manifest["schema"], "codescribe.bus-generations.v1");
            assert!(
                manifest["pending"].is_null(),
                "fixture must have a settled swap"
            );
            let mut bytes = Vec::new();
            for segment in manifest["segments"].as_array().unwrap() {
                assert_eq!(segment["start"].as_u64(), Some(bytes.len() as u64));
                let archive = fs::read(segment["path"].as_str().unwrap()).unwrap();
                assert_eq!(segment["length"].as_u64(), Some(archive.len() as u64));
                bytes.extend(archive);
            }
            assert_eq!(
                manifest["active"]["start"].as_u64(),
                Some(bytes.len() as u64)
            );
            assert_eq!(manifest["active"]["path"].as_str(), path.to_str());
            bytes.extend(fs::read(path).unwrap());
            bytes
        }
    };
    String::from_utf8(bytes).unwrap()
}

pub(crate) fn rows(raw: &str) -> Result<Vec<Value>, String> {
    const FIELDS: [&str; 9] = [
        "sequence",
        "emitted_at",
        "occurrence_session_id",
        "capture_epoch",
        "sample_start",
        "sample_end",
        "document_index",
        "label",
        "acoustic_receipts",
    ];
    let mut out = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let mut row: Value =
            serde_json::from_str(line).map_err(|e| format!("invalid fixture JSON: {e}"))?;
        let Some(encoding) = row.get("persistence_encoding") else {
            out.push(row);
            continue;
        };
        if encoding != "shared-revision.v1" {
            return Err("unsupported fixture encoding".into());
        }
        let mut children = row["occurrence_rows"]
            .as_array()
            .ok_or("missing occurrence array")?
            .clone();
        let object = row.as_object_mut().ok_or("not an object")?;
        object.remove("occurrence_rows");
        object.remove("persistence_encoding");
        let mut previous = row["sequence"].as_u64().ok_or("missing first sequence")?;
        out.push(row.clone());
        for child in children.drain(..) {
            let fields = child.as_object().ok_or("not an occurrence object")?;
            if fields.len() != FIELDS.len() || FIELDS.iter().any(|key| !fields.contains_key(*key)) {
                return Err("missing or unauthorized occurrence fields".into());
            }
            let sequence = child["sequence"]
                .as_u64()
                .ok_or("missing occurrence sequence")?;
            if sequence <= previous {
                return Err("replayed or unordered occurrence sequence".into());
            }
            previous = sequence;
            let mut occurrence = row.clone();
            for key in FIELDS {
                occurrence[key] = child[key].clone();
            }
            out.push(occurrence);
        }
    }
    Ok(out)
}

#[test]
fn shared_wire_oracle_rejects_missing_fields_replay_and_document_overwrite() {
    let child = serde_json::json!({"sequence": 2, "emitted_at": "now", "occurrence_session_id": "take",
        "capture_epoch": 1, "sample_start": 1, "sample_end": 2, "document_index": 1,
        "label": "two", "acoustic_receipts": [{"id":"second"}]});
    let parent = serde_json::json!({"sequence":1,"rendered_text":"one two",
        "persistence_encoding":"shared-revision.v1","occurrence_rows":[child]});
    let decoded = rows(&parent.to_string()).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[1]["rendered_text"], "one two");
    assert_eq!(
        decoded[1]["acoustic_receipts"],
        serde_json::json!([{"id":"second"}])
    );
    for mutation in ["missing", "replay", "overwrite"] {
        let mut mutant = parent.clone();
        let child = &mut mutant["occurrence_rows"][0];
        match mutation {
            "missing" => {
                child.as_object_mut().unwrap().remove("sample_end");
            }
            "replay" => child["sequence"] = 1.into(),
            _ => child["rendered_text"] = "forged".into(),
        }
        assert!(rows(&mutant.to_string()).is_err(), "accepted {mutation}");
    }
}
