//! One-time move of high-churn data into [`Config::churn_dir`].
//!
//! Spotlight re-imports a file on every write unless it sits in a `*.noindex`
//! folder. The entries listed in [`CHURN_ENTRIES`] are rewritten or appended
//! constantly and have no path-identity role: nothing persists or compares
//! their path string, and no other process or language finds them by path.
//! Their owners now write under `<data root>/data.noindex`; this module moves
//! what an earlier build left at the old location.
//!
//! Each entry is one same-volume `rename(2)` that refuses an existing
//! destination. The move never copies, never deletes, and never overwrites:
//!
//! | old location | new location | outcome |
//! | --- | --- | --- |
//! | absent | absent | [`ChurnMoveOutcome::NothingToMove`] |
//! | absent | present | [`ChurnMoveOutcome::AlreadyMigrated`] |
//! | present | absent | renamed: [`ChurnMoveOutcome::Moved`] |
//! | present | present | [`ChurnMoveOutcome::RefusedBothPresent`], both untouched |
//! | present, other volume | absent | [`ChurnMoveOutcome::RefusedCrossVolume`], untouched |
//!
//! Running it again is a no-op. Path-identity data (buses and their `events/`
//! archives, leases, bindings, `sessions/`, `takes/`, `transcriptions/`, the
//! files `bus-demux` reads by path) is never in the table.

use std::io;
use std::path::{Path, PathBuf};

use super::Config;

/// Where an entry's old location is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChurnAnchor {
    /// Directly under the data root (`Config::config_dir`).
    DataRoot,
    /// Under the agent-bridge home's `runtime/` directory.
    BridgeRuntime,
}

/// One moved entry: its old anchor and the name it keeps in both places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChurnEntry {
    pub anchor: ChurnAnchor,
    pub name: &'static str,
}

/// Every entry the churn folder owns. Owners resolve their path as
/// `Config::churn_dir().join(name)`:
///
/// - `logs`: the Rust tracing sink `codescribe.log` and the Swift
///   `diagnostic.log`, appended constantly;
/// - `overlay-delivery-cursor`: the overlay delivery observer's per-bus cache
///   units, rewritten on every checkpoint;
/// - `overlay-delivery-cursor.v1.json`: that cache's earlier single file. It
///   is never read; the first successful cache save deletes it from here;
/// - `overlay-read-state.v1.json`: the overlay's reply read marks.
pub const CHURN_ENTRIES: &[ChurnEntry] = &[
    ChurnEntry {
        anchor: ChurnAnchor::DataRoot,
        name: "logs",
    },
    ChurnEntry {
        anchor: ChurnAnchor::BridgeRuntime,
        name: "overlay-delivery-cursor",
    },
    ChurnEntry {
        anchor: ChurnAnchor::BridgeRuntime,
        name: "overlay-delivery-cursor.v1.json",
    },
    ChurnEntry {
        anchor: ChurnAnchor::BridgeRuntime,
        name: "overlay-read-state.v1.json",
    },
];

/// What happened to one entry.
#[derive(Debug)]
pub enum ChurnMoveOutcome {
    /// Neither location exists.
    NothingToMove,
    /// Only the new location exists.
    AlreadyMigrated,
    /// The old entry was renamed to the new location.
    Moved,
    /// Both locations exist; neither was touched.
    RefusedBothPresent,
    /// The old entry is on another volume; it was not copied or touched.
    RefusedCrossVolume,
    /// Inspecting or renaming failed; nothing was removed.
    Failed(io::Error),
}

impl ChurnMoveOutcome {
    /// A refusal or failure the caller must report loudly.
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::RefusedBothPresent | Self::RefusedCrossVolume | Self::Failed(_)
        )
    }
}

/// One entry's result with both of its paths.
#[derive(Debug)]
pub struct ChurnMove {
    pub entry: ChurnEntry,
    pub from: PathBuf,
    pub to: PathBuf,
    pub outcome: ChurnMoveOutcome,
}

/// Move owned [`CHURN_ENTRIES`] items from their old locations into `churn`
/// (`Config::churn_dir` in production). `data_root` anchors data-root entries;
/// `bridge_home` admits bridge entries only when that runtime owns the bridge.
/// Call before any owner opens these paths.
pub fn migrate_churn_data(
    data_root: &Path,
    churn: &Path,
    bridge_home: Option<&Path>,
) -> Vec<ChurnMove> {
    CHURN_ENTRIES
        .iter()
        .filter_map(|entry| {
            let from = match entry.anchor {
                ChurnAnchor::DataRoot => data_root.join(entry.name),
                ChurnAnchor::BridgeRuntime => bridge_home?.join("runtime").join(entry.name),
            };
            let to = churn.join(entry.name);
            let outcome = move_entry(&from, &to, churn);
            Some(ChurnMove {
                entry: *entry,
                from,
                to,
                outcome,
            })
        })
        .collect()
}

/// The production migration: the resolved data root, its churn folder and,
/// for standalone Codescribe only, the resolved agent-bridge home. An embedding
/// host owns its data root but does not own the standalone bridge. The app runs
/// this once per process, before the log sink opens (`codescribe::logging`).
pub fn migrate_churn_data_at_startup() -> Vec<ChurnMove> {
    let bridge_home = super::runtime_host::selected()
        .is_none()
        .then(crate::stt::active_names::bridge_home);
    migrate_churn_data(
        &Config::config_dir(),
        &Config::churn_dir(),
        bridge_home.as_deref(),
    )
}

fn move_entry(from: &Path, to: &Path, churn: &Path) -> ChurnMoveOutcome {
    let old = match present(from) {
        Ok(old) => old,
        Err(error) => return ChurnMoveOutcome::Failed(error),
    };
    let new = match present(to) {
        Ok(new) => new,
        Err(error) => return ChurnMoveOutcome::Failed(error),
    };
    match (old, new) {
        (false, false) => ChurnMoveOutcome::NothingToMove,
        (false, true) => ChurnMoveOutcome::AlreadyMigrated,
        (true, true) => ChurnMoveOutcome::RefusedBothPresent,
        (true, false) => {
            if let Err(error) = std::fs::create_dir_all(churn) {
                return ChurnMoveOutcome::Failed(error);
            }
            match rename_exclusive(from, to) {
                Ok(()) => ChurnMoveOutcome::Moved,
                Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                    ChurnMoveOutcome::RefusedCrossVolume
                }
                // A destination created after the check: the exclusive rename
                // refused it, so both entries stay as they are.
                Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {
                    ChurnMoveOutcome::RefusedBothPresent
                }
                Err(error) => ChurnMoveOutcome::Failed(error),
            }
        }
    }
}

/// Whether a directory entry exists, without following a final symlink.
fn present(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// `rename(2)` that refuses an existing destination, also one raced in after
/// the presence check, so no file or non-empty folder is ever replaced.
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both pointers are NUL-terminated paths that outlive the call.
    #[cfg(target_vendor = "apple")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    // SAFETY: as above.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    let result: libc::c_int = {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "churn migration requires an exclusive rename",
        ));
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        bridge: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let bridge = root.join("agent-bridge");
            fs::create_dir_all(bridge.join("runtime")).unwrap();
            Self {
                _temp: temp,
                root,
                bridge,
            }
        }

        fn churn(&self) -> PathBuf {
            self.root.join("data.noindex")
        }

        fn old(&self, name: &str) -> PathBuf {
            let entry = CHURN_ENTRIES
                .iter()
                .find(|entry| entry.name == name)
                .unwrap();
            match entry.anchor {
                ChurnAnchor::DataRoot => self.root.join(name),
                ChurnAnchor::BridgeRuntime => self.bridge.join("runtime").join(name),
            }
        }

        fn run(&self) -> Vec<ChurnMove> {
            migrate_churn_data(&self.root, &self.churn(), Some(&self.bridge))
        }
    }

    fn outcome<'a>(moves: &'a [ChurnMove], name: &str) -> &'a ChurnMoveOutcome {
        &moves
            .iter()
            .find(|item| item.entry.name == name)
            .unwrap()
            .outcome
    }

    fn seed_old_layout(fixture: &Fixture) {
        let logs = fixture.old("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("codescribe.log"), b"old log\n").unwrap();
        fs::write(logs.join("diagnostic.log"), b"old diagnostic\n").unwrap();
        let cursor = fixture.old("overlay-delivery-cursor");
        fs::create_dir_all(&cursor).unwrap();
        fs::write(cursor.join("0123456789abcdef.json"), b"{}").unwrap();
        fs::write(fixture.old("overlay-delivery-cursor.v1.json"), b"{}").unwrap();
        fs::write(fixture.old("overlay-read-state.v1.json"), b"{\"marks\":1}").unwrap();
    }

    /// Fresh install: nothing at either location, nothing is created.
    #[test]
    fn fresh_data_root_moves_nothing_and_creates_nothing() {
        let fixture = Fixture::new();
        let moves = fixture.run();
        assert_eq!(moves.len(), CHURN_ENTRIES.len());
        for item in &moves {
            assert!(matches!(item.outcome, ChurnMoveOutcome::NothingToMove));
            assert!(!item.outcome.is_refusal());
        }
        assert!(!fixture.churn().exists());
    }

    /// An existing install: every entry is renamed with its bytes, and the
    /// old locations are gone without anything being deleted or copied.
    #[test]
    fn existing_layout_is_renamed_into_data_noindex_with_its_bytes() {
        let fixture = Fixture::new();
        seed_old_layout(&fixture);
        let log_inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(fixture.old("logs").join("codescribe.log"))
                .unwrap()
                .ino()
        };

        let moves = fixture.run();
        for item in &moves {
            assert!(
                matches!(item.outcome, ChurnMoveOutcome::Moved),
                "{}: {:?}",
                item.entry.name,
                item.outcome
            );
            assert_eq!(item.to, fixture.churn().join(item.entry.name));
            assert!(!item.from.exists(), "{} left behind", item.from.display());
        }
        let churn = fixture.churn();
        assert_eq!(
            fs::read(churn.join("logs/codescribe.log")).unwrap(),
            b"old log\n"
        );
        assert_eq!(
            fs::read(churn.join("logs/diagnostic.log")).unwrap(),
            b"old diagnostic\n"
        );
        assert!(
            churn
                .join("overlay-delivery-cursor/0123456789abcdef.json")
                .is_file()
        );
        assert!(churn.join("overlay-delivery-cursor.v1.json").is_file());
        assert_eq!(
            fs::read(churn.join("overlay-read-state.v1.json")).unwrap(),
            b"{\"marks\":1}"
        );
        {
            use std::os::unix::fs::MetadataExt;
            let moved = fs::metadata(churn.join("logs/codescribe.log")).unwrap();
            assert_eq!(moved.ino(), log_inode, "a rename keeps the inode; no copy");
        }
        // Nothing else under the data root or the bridge runtime moved.
        assert!(fixture.bridge.join("runtime").is_dir());
    }

    /// A second run, and a run after migration, are no-ops.
    #[test]
    fn already_migrated_layout_is_left_as_it_is() {
        let fixture = Fixture::new();
        seed_old_layout(&fixture);
        fixture.run();
        let moves = fixture.run();
        for item in &moves {
            assert!(
                matches!(item.outcome, ChurnMoveOutcome::AlreadyMigrated),
                "{}: {:?}",
                item.entry.name,
                item.outcome
            );
        }
        assert_eq!(
            fs::read(fixture.churn().join("logs/codescribe.log")).unwrap(),
            b"old log\n"
        );
    }

    /// Both locations present: refused loudly, and neither side changes.
    #[test]
    fn both_present_is_refused_and_both_sides_stay_untouched() {
        let fixture = Fixture::new();
        seed_old_layout(&fixture);
        let churn = fixture.churn();
        fs::create_dir_all(churn.join("logs")).unwrap();
        fs::write(churn.join("logs/codescribe.log"), b"new log\n").unwrap();
        fs::write(churn.join("overlay-read-state.v1.json"), b"new marks").unwrap();

        let moves = fixture.run();
        for name in ["logs", "overlay-read-state.v1.json"] {
            let result = outcome(&moves, name);
            assert!(
                matches!(result, ChurnMoveOutcome::RefusedBothPresent),
                "{name}: {result:?}"
            );
            assert!(result.is_refusal());
        }
        assert_eq!(
            fs::read(fixture.old("logs").join("codescribe.log")).unwrap(),
            b"old log\n"
        );
        assert_eq!(
            fs::read(churn.join("logs/codescribe.log")).unwrap(),
            b"new log\n"
        );
        assert_eq!(
            fs::read(fixture.old("overlay-read-state.v1.json")).unwrap(),
            b"{\"marks\":1}"
        );
        assert_eq!(
            fs::read(churn.join("overlay-read-state.v1.json")).unwrap(),
            b"new marks"
        );
        // Entries without a conflict still move.
        assert!(matches!(
            outcome(&moves, "overlay-delivery-cursor"),
            ChurnMoveOutcome::Moved
        ));
    }

    /// Even an empty destination folder is not replaced: the exclusive rename
    /// never overwrites, so an empty `data.noindex/logs` also refuses.
    #[test]
    fn empty_destination_folder_is_not_replaced() {
        let fixture = Fixture::new();
        seed_old_layout(&fixture);
        fs::create_dir_all(fixture.churn().join("logs")).unwrap();
        let moves = fixture.run();
        assert!(matches!(
            outcome(&moves, "logs"),
            ChurnMoveOutcome::RefusedBothPresent
        ));
        assert!(fixture.old("logs").join("codescribe.log").is_file());
    }

    /// Path-identity data under the data root and the bridge runtime is never
    /// part of the move.
    #[test]
    fn identity_bearing_data_is_never_moved() {
        let fixture = Fixture::new();
        for name in ["sessions", "takes", "events", "transcriptions", "models"] {
            fs::create_dir_all(fixture.root.join(name)).unwrap();
        }
        for name in ["followers", "reply-playback", "native-wake", "plugins"] {
            fs::create_dir_all(fixture.bridge.join("runtime").join(name)).unwrap();
        }
        fs::write(fixture.root.join("transcript-events.jsonl"), b"").unwrap();
        fixture.run();
        for name in ["sessions", "takes", "events", "transcriptions", "models"] {
            assert!(fixture.root.join(name).is_dir(), "{name} moved");
        }
        for name in ["followers", "reply-playback", "native-wake", "plugins"] {
            assert!(fixture.bridge.join("runtime").join(name).is_dir());
        }
        assert!(fixture.root.join("transcript-events.jsonl").is_file());
        assert!(
            CHURN_ENTRIES
                .iter()
                .all(|entry| !["sessions", "takes", "events", "transcriptions"]
                    .contains(&entry.name))
        );
    }
}
