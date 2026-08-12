//! Per-fd SIGPIPE suppression for pipes we write to child processes.
//!
//! # Why this is not optional here
//!
//! Rust's runtime sets `SIGPIPE` to `SIG_IGN` before `main`, so a broken-pipe
//! write in an ordinary Rust binary surfaces as an `EPIPE` error. That setup
//! **never runs** in this product: the core ships as `crate-type = ["staticlib",
//! "cdylib"]` and is loaded into a Swift host, which keeps the default
//! disposition. A write to a dead child's stdin therefore kills the entire
//! application — and Darwin does not file a ReportCrash entry for it, so the app
//! simply vanishes with nothing in the logs to explain it.
//!
//! This was first diagnosed for the MCP stdio client (U14, `a35a64b`), where a
//! server that died at exec took the app down with it on the farewell write. The
//! same hazard exists for every child we pipe into; observed again 2026-08-12
//! when killing the Apple STT bridge terminated the host app.
//!
//! `F_SETNOSIGPIPE` is deliberately per-fd rather than a process-wide
//! `signal(SIGPIPE, SIG_IGN)`: the core is a guest inside someone else's
//! process and must not mutate the host's signal table.

/// Mark a pipe so writes to a dead peer return `EPIPE` instead of raising
/// `SIGPIPE`.
///
/// Best-effort by design: a failure leaves the previous behaviour in place, so
/// callers keep whatever liveness check they already had. Accepts anything with
/// a raw fd, so both `std::process::ChildStdin` and its async equivalents work.
#[cfg(target_os = "macos")]
pub fn disable_sigpipe<F: std::os::fd::AsRawFd>(pipe: &F) {
    // Darwin `sys/fcntl.h`: `#define F_SETNOSIGPIPE 73`. The libc crate does not
    // export this per-fd fcntl command (only the socket-level `SO_NOSIGPIPE`),
    // so the value is pinned here.
    /// Darwin fcntl command: mark a fd so broken-pipe writes return EPIPE, not SIGPIPE.
    const F_SETNOSIGPIPE: libc::c_int = 73;

    // SAFETY: fcntl on a fd the caller owns; `F_SETNOSIGPIPE` only flips a
    // per-fd flag and cannot invalidate the descriptor.
    let _ = unsafe { libc::fcntl(pipe.as_raw_fd(), F_SETNOSIGPIPE, 1) };
}

/// No-op outside macOS: `F_SETNOSIGPIPE` is a Darwin-specific fcntl.
#[cfg(not(target_os = "macos"))]
pub fn disable_sigpipe<F>(_pipe: &F) {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// The flag must actually be set on the pipe.
    ///
    /// A Rust test cannot reproduce the failure itself — the harness is an
    /// ordinary Rust binary, so its runtime already ignores SIGPIPE and a
    /// broken-pipe write returns `EPIPE` with or without this call. The part
    /// that can silently rot is the pinned command number: `F_SETNOSIGPIPE` is
    /// not exported by the libc crate, so `73` is a hand-copied constant. Read
    /// it back with `F_GETNOSIGPIPE` (74) and the constant is pinned by
    /// evidence rather than by comment.
    #[test]
    fn disable_sigpipe_sets_the_flag_the_swift_host_depends_on() {
        use std::os::fd::AsRawFd;

        /// Darwin fcntl command: read back the per-fd no-SIGPIPE flag.
        const F_GETNOSIGPIPE: libc::c_int = 74;

        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn /bin/cat");
        let stdin = child.stdin.as_ref().expect("child stdin");

        // SAFETY: reading a per-fd flag on a descriptor owned by this test.
        let before = unsafe { libc::fcntl(stdin.as_raw_fd(), F_GETNOSIGPIPE) };
        assert_eq!(
            before, 0,
            "pipes start with SIGPIPE live — that is the hazard"
        );

        disable_sigpipe(stdin);

        // SAFETY: same descriptor, same read-only command.
        let after = unsafe { libc::fcntl(stdin.as_raw_fd(), F_GETNOSIGPIPE) };
        assert_eq!(
            after, 1,
            "F_SETNOSIGPIPE must take effect; a wrong command number fails silently \
             and only shows up as the whole app vanishing without a crash report"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
