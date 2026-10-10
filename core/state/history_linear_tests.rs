use super::*;
use serial_test::serial;
use tempfile::TempDir;

#[test]
#[serial]
fn four_equal_successful_attempts_survive_reopen_and_four_undo_redo_steps() {
    let root = TempDir::new().unwrap();
    let _env =
        crate::test_isolation::EnvGuard::set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
    let day = transcriptions_base_dir().join("2026-10-10");
    fs::create_dir_all(&day).unwrap();
    let path = day.join("010000_linear_raw.txt");
    let raw = "Iwo Iwo Iwo Iwo Iwo.";
    fs::write(&path, raw).unwrap();
    let audio = path.with_extension("wav");
    fs::write(&audio, b"five physical occurrences, immutable PCM fixture").unwrap();
    for index in 0..4 {
        let provenance = if index == 3 {
            ArchiveRevisionProvenance::Formatter
        } else {
            ArchiveRevisionProvenance::Retranscribe
        };
        let receipt = commit_archived_revision(&path, index, raw, provenance, None).unwrap();
        assert_eq!(receipt.revision, index + 1);
    }
    let first = read_archived_document(&path).unwrap();
    assert_eq!(first.timeline().steps.len(), 5);
    assert_eq!(first.timeline().cursor, 4);
    let identities = first
        .timeline()
        .steps
        .iter()
        .map(|step| step.revision)
        .collect::<Vec<_>>();
    assert_eq!(identities, [0, 1, 2, 3, 4]);
    let receipts = first
        .revisions
        .iter()
        .map(|step| &step.receipt_id)
        .collect::<HashSet<_>>();
    assert_eq!(
        receipts.len(),
        4,
        "equal outputs are distinct accepted attempts"
    );

    for target in (0..4).rev().chain(1..5) {
        let before = read_archived_document(&path).unwrap();
        let moved = navigate_archived_revision(&path, before.head_revision(), target).unwrap();
        assert_eq!(moved.restored_revision, Some(target));
        let reopened = read_archived_document(&path).unwrap();
        assert_eq!(reopened.timeline().cursor, target as usize);
        assert_eq!(
            reopened.timeline().steps.len(),
            5,
            "navigation is not another attempt"
        );
        assert_eq!(reopened.head_text(), raw);
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), raw);
    assert_eq!(
        fs::read(&audio).unwrap(),
        b"five physical occurrences, immutable PCM fixture"
    );
}

#[test]
#[serial]
fn accepted_equal_attempt_after_undo_cuts_redo_but_stale_write_does_not() {
    let root = TempDir::new().unwrap();
    let _env =
        crate::test_isolation::EnvGuard::set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
    let day = transcriptions_base_dir().join("2026-10-10");
    fs::create_dir_all(&day).unwrap();
    let path = day.join("010100_branch_raw.txt");
    fs::write(&path, "raw").unwrap();
    for index in 0..3 {
        commit_archived_revision(
            &path,
            index,
            "same",
            ArchiveRevisionProvenance::Retranscribe,
            None,
        )
        .unwrap();
    }
    let moved = navigate_archived_revision(&path, 3, 1).unwrap();
    let before = read_archived_document(&path).unwrap();
    assert!(
        commit_archived_revision(
            &path,
            3,
            "stale",
            ArchiveRevisionProvenance::Formatter,
            None,
        )
        .is_err()
    );
    assert_eq!(read_archived_document(&path).unwrap(), before);
    let accepted = commit_archived_revision(
        &path,
        moved.revision,
        "same",
        ArchiveRevisionProvenance::Formatter,
        None,
    )
    .unwrap();
    let reopened = read_archived_document(&path).unwrap();
    assert_eq!(
        reopened
            .timeline()
            .steps
            .iter()
            .map(|step| step.revision)
            .collect::<Vec<_>>(),
        [0, 1, accepted.revision]
    );
    assert_eq!(reopened.timeline().cursor, 2);
    assert!(
        navigate_archived_revision(&path, accepted.revision, 3).is_err(),
        "discarded redo branch cannot be selected"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "raw");
}
