//! Bounded archive-only conversion. The child owns only private staging paths.

#[cfg(any(target_os = "macos", all(test, unix)))]
use anyhow::Context;
use anyhow::Result;
use std::fs::File;
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::io::{Seek, SeekFrom};
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::process::{Child, Command, Stdio};
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::time::{Duration, Instant};

/// Existing Stop budget is 120s; archive conversion gets at most 15s of it.
#[cfg(target_os = "macos")]
const ENCODE_TIMEOUT: Duration = Duration::from_secs(15);

/// Own the child until it has been waited for, including error/unwind paths.
#[cfg(any(target_os = "macos", all(test, unix)))]
struct EncoderChild(Child);

#[cfg(any(target_os = "macos", all(test, unix)))]
impl Drop for EncoderChild {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            // An already reaped child needs no kill; wait below remains harmless.
            tracing::debug!("archive child kill: {error}");
        }
        if let Err(error) = self.0.wait() {
            tracing::warn!("archive child reap failed: {error}");
        }
    }
}

/// afconvert replaces its output leaf, so inherited /dev/fd output is invalid.
/// Only this 0700 directory is exposed to the child, never a caller pathname or
/// caller descriptor. The held directory pins output admission across renames.
/// This is not a sandbox against an arbitrary compromised same-UID process.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn run_encoder(
    command: &mut Command,
    input: &mut File,
    output: &mut File,
    timeout: Duration,
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let input_meta = input.metadata()?;
    let output_meta = output.metadata()?;
    anyhow::ensure!(
        input_meta.is_file(),
        "archive encoder input must be regular"
    );
    anyhow::ensure!(
        output_meta.is_file(),
        "archive encoder destination must be regular"
    );
    anyhow::ensure!(
        (input_meta.dev(), input_meta.ino()) != (output_meta.dev(), output_meta.ino()),
        "archive encoder input aliases destination"
    );
    let staging = tempfile::Builder::new()
        .prefix("codescribe-encoder-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(staging.path())?;
    anyhow::ensure!(
        dir.metadata()?.permissions().mode() & 0o777 == 0o700,
        "archive encoder staging must be private"
    );
    let mut staged_input = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(staging.path().join("input.wav"))?;
    input.seek(SeekFrom::Start(0))?;
    std::io::copy(input, &mut staged_input)?;
    drop(staged_input);
    let child = command
        .current_dir(staging.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn archive encoder")?;
    // The child is reaped before admission and before the directory is dropped.
    supervise(child, timeout)?;
    let mut encoded = admit_encoder_output(&dir)?;
    output.set_len(0)?;
    output.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut encoded, output)?;
    output.seek(SeekFrom::Start(0))?;
    Ok(())
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn admit_encoder_output(dir: &File) -> Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    // SAFETY: held directory and fixed NUL-terminated leaf; no-follow rejects
    // symlinks, nonblocking prevents a FIFO from hanging before the type check.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c"output.m4a".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open archive encoder output");
    }
    // SAFETY: successful openat transfers one owned descriptor.
    let encoded = unsafe { File::from_raw_fd(fd) };
    let metadata = encoded.metadata()?;
    anyhow::ensure!(metadata.is_file(), "archive encoder output must be regular");
    anyhow::ensure!(
        metadata.nlink() == 1,
        "archive encoder output must not be hardlinked"
    );
    anyhow::ensure!(metadata.len() > 0, "archive encoder returned empty success");
    Ok(encoded)
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn supervise(child: Child, timeout: Duration) -> Result<()> {
    let mut child = EncoderChild(child);
    let started = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().context("poll archive encoder")? {
            anyhow::ensure!(status.success(), "archive encoder failed: {status}");
            return Ok(());
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "archive encoder deadline exceeded"
        );
        // No detached waiter: this thread owns polling, termination and reaping.
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

/// Encode a held WAV into a held destination through encoder-private staging.
pub(crate) fn encode_wav_to_m4a(input: &mut File, output: &mut File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_encoder(
            Command::new("/usr/bin/afconvert").args([
                "-f",
                "m4af",
                "-d",
                "aac@44100",
                "-b",
                "64000",
                "input.wav",
                "output.m4a",
            ]),
            input,
            output,
            ENCODE_TIMEOUT,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (input, output);
        anyhow::bail!("m4a archive encoding requires macOS afconvert")
    }
}

/// Closed test scenarios: no caller-supplied shell program is accepted.
#[cfg(all(test, unix))]
pub(crate) enum TestEncoderOutcome {
    Failed,
    Empty,
    Hanging,
    Symlink,
    Directory,
    Fifo,
    Hardlink,
}

/// Inject the actual child owner into history failure tests (never afconvert).
#[cfg(all(test, unix))]
pub(crate) fn encode_test_child(
    input: &mut File,
    output: &mut File,
    outcome: TestEncoderOutcome,
) -> Result<()> {
    let script = match outcome {
        TestEncoderOutcome::Failed => "printf partial > output.m4a; exit 9",
        TestEncoderOutcome::Empty => ": > output.m4a",
        TestEncoderOutcome::Hanging => "while :; do :; done",
        TestEncoderOutcome::Symlink => "ln -s input.wav output.m4a",
        TestEncoderOutcome::Directory => "mkdir output.m4a",
        TestEncoderOutcome::Fifo => "mkfifo output.m4a",
        TestEncoderOutcome::Hardlink => "ln input.wav output.m4a",
    };
    run_encoder(
        Command::new("/bin/sh").args(["-c", script]),
        input,
        output,
        Duration::from_millis(100),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn bytes(file: &mut File) -> Vec<u8> {
        file.rewind().expect("rewind");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read");
        bytes
    }

    #[test]
    fn rejected_children_preserve_input_and_held_destination_bytes() {
        for (outcome, reason) in [
            (TestEncoderOutcome::Failed, "archive encoder failed"),
            (TestEncoderOutcome::Empty, "empty success"),
            (TestEncoderOutcome::Hanging, "deadline exceeded"),
            (TestEncoderOutcome::Symlink, "open archive encoder output"),
            (TestEncoderOutcome::Directory, "output must be regular"),
            (TestEncoderOutcome::Fifo, "output must be regular"),
            (TestEncoderOutcome::Hardlink, "must not be hardlinked"),
        ] {
            let mut input = tempfile::tempfile().expect("input");
            input.write_all(b"admitted WAV").expect("fixture");
            let mut output = tempfile::tempfile().expect("output");
            output.write_all(b"held destination").expect("sentinel");
            let error = encode_test_child(&mut input, &mut output, outcome)
                .expect_err("encoder output refused");
            assert!(error.to_string().contains(reason), "{error:#}");
            assert_eq!(bytes(&mut input), b"admitted WAV");
            assert_eq!(bytes(&mut output), b"held destination");
        }
    }

    fn assert_reaped(pid: libc::pid_t) {
        let mut status = 0;
        // SAFETY: check only the recorded child; the status pointer is valid.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn success_failure_and_deadline_return_only_after_child_is_reaped() {
        for (script, timeout, expected) in [
            ("exit 0", Duration::from_secs(2), None),
            (
                "exit 9",
                Duration::from_secs(2),
                Some("archive encoder failed"),
            ),
            (
                "while :; do :; done",
                Duration::ZERO,
                Some("deadline exceeded"),
            ),
        ] {
            let child = Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("child");
            let pid = child.id() as libc::pid_t;
            let result = supervise(child, timeout);
            match expected {
                Some(message) => {
                    assert!(result.expect_err("refused").to_string().contains(message))
                }
                None => result.expect("success"),
            }
            assert_reaped(pid);
        }
    }

    #[test]
    fn unwind_reaps_owned_child() {
        let child = Command::new("/bin/sh")
            .args(["-c", "while :; do :; done"])
            .spawn()
            .expect("child");
        let pid = child.id() as libc::pid_t;
        let result = std::panic::catch_unwind(move || {
            let _owner = EncoderChild(child);
            panic!("injected encoder unwind");
        });
        assert!(result.is_err());
        assert_reaped(pid);
    }

    #[test]
    fn child_observes_private_directory_and_input_before_encoder_work() {
        let mut input = tempfile::tempfile().expect("input");
        input.write_all(b"admitted audio").expect("fixture");
        let mut output = tempfile::tempfile().expect("output");
        output.write_all(b"held destination").expect("sentinel");
        // Observe modes in the actual encoder child, before creating output.
        // The returned bytes are a transport receipt, not an M4A codec claim.
        let stat_args = if cfg!(target_os = "macos") {
            "-f %Lp"
        } else {
            "-c %a"
        };
        run_encoder(
            Command::new("/bin/sh").args([
                "-c",
                r#"set -eu
directory_mode=$(/usr/bin/stat $1 .)
input_mode=$(/usr/bin/stat $1 input.wav)
test "$directory_mode" = 700
test "$input_mode" = 600
test ! -e output.m4a
test "$(cat input.wav)" = 'admitted audio'
printf '%s/%s\n' "$directory_mode" "$input_mode" > output.m4a
cat input.wav >> output.m4a
"#,
                "mode-witness",
                stat_args,
            ]),
            &mut input,
            &mut output,
            Duration::from_secs(2),
        )
        .expect("child verifies private staging before encoder work");
        assert_eq!(bytes(&mut output), b"700/600\nadmitted audio");
        assert_eq!(bytes(&mut input), b"admitted audio");
    }

    #[test]
    fn child_can_replace_private_output_and_mutate_only_staged_input() {
        let mut input = tempfile::tempfile().expect("input");
        input.write_all(b"admitted audio").expect("fixture");
        let mut output = tempfile::tempfile().expect("output");
        // Transport witness only: these bytes are not asserted to be M4A.
        run_encoder(
            Command::new("/bin/sh").args([
                "-c",
                "printf old > output.m4a; rm output.m4a; cat input.wav > output.m4a; printf changed > input.wav",
            ]),
            &mut input,
            &mut output,
            Duration::from_secs(2),
        )
        .expect("private replacement");
        assert_eq!(bytes(&mut output), b"admitted audio");
        assert_eq!(bytes(&mut input), b"admitted audio");
    }

    #[test]
    fn output_admission_uses_held_directory_and_keeps_held_leaf_after_replacement() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().expect("root");
        let stage = root.path().join("stage");
        std::fs::create_dir(&stage).expect("stage");
        let dir = File::open(&stage).expect("held directory");
        std::fs::write(stage.join("output.m4a"), b"admitted result").expect("result");
        let moved = root.path().join("moved");
        std::fs::rename(&stage, &moved).expect("move");
        std::fs::create_dir(&stage).expect("replacement dir");
        std::fs::write(stage.join("output.m4a"), b"foreign result").expect("foreign");
        let mut admitted = admit_encoder_output(&dir).expect("original directory");
        std::fs::remove_file(moved.join("output.m4a")).expect("remove leaf");
        symlink(stage.join("output.m4a"), moved.join("output.m4a")).expect("replace leaf");
        assert!(admit_encoder_output(&dir).is_err());
        assert_eq!(bytes(&mut admitted), b"admitted result");
        assert_eq!(
            std::fs::read(stage.join("output.m4a")).expect("foreign"),
            b"foreign result"
        );
    }

    #[test]
    fn aliased_destination_is_rejected_without_changing_input() {
        let mut input = tempfile::tempfile().expect("input");
        input.write_all(b"admitted WAV").expect("fixture");
        let mut output = input.try_clone().expect("alias");
        let error = run_encoder(
            &mut Command::new("/usr/bin/false"),
            &mut input,
            &mut output,
            Duration::from_secs(2),
        )
        .expect_err("alias rejected before spawn");
        assert!(error.to_string().contains("aliases destination"));
        assert_eq!(bytes(&mut input), b"admitted WAV");
    }
}
