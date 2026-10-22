# Restart an idle application

```sh
make restart-if-idle
```

This arms one background waiter and returns at once. The waiter restarts the
installed desktop application at the first idle moment, and only when the
running process did not load the installed main and FFI binaries, which means
a newer bundle was installed after it started. It plays a sound and says
"Zrobione" after a verified restart, or "Restart się nie udał" after a
refusal that waiting cannot change or when the wait runs out. With nothing newer installed it exits
silently with `"status": "current"`.

`make install-app` arms the same waiter as its last step, after the
post-install STT probe passes. `make install-app RESTART_AFTER_INSTALL=0`
installs without arming it. Maciej asked for this automation on 2026-10-22
("żeby robił restart apki, kiedy jest nieaktywna, i tylko wtedy, kiedy jest
zainstalowana nowa wersja… Zautomatyzujmy to").

The waiter retries every 30 seconds for up to six hours
(`RESTART_IDLE_WAIT`, `RESTART_IDLE_INTERVAL`), only while the refusal can
end on its own: a take is active or settling, an agent turn holds the runtime
lease, a request expired or the application never answered it. Any other
refusal ends it at once. A refusal after the application accepted shutdown is
never retried. Only one waiter runs at a time; a second one, for example
after another install, exits with `deferred_to_active_waiter`, because every
attempt compares against the bundle installed at that moment. The waiter
appends its JSON lines to `waiter.log` in the handshake directory.

`make restart` still stops and starts the application without asking it.
Use it only when the application does not answer.

## The native protocol

Run once, immediately, without the newer-version check:

```sh
./scripts/restart-if-idle
```

The script sends a generation-bound request through the existing Codescribe
URL handler. The application acquires the existing exclusive agent-turn lease,
checks the bus, then asks the recording controller to reserve shutdown under
the same lock used by capture start. A recording, open channel, pending start,
unsettled capture tail, inaccessible state or controller that is not ready
refuses. Refusal leaves capture admission open. Acceptance closes new capture
admission before normal application termination begins. An independently read
bus snapshot alone cannot authorize Quit.

The script waits for that exact native receipt and old PID exit, then launches
the installed application. Success requires the new process capability,
strict installed signature and loaded main/FFI inodes matching the installed
bundle. It prints a JSON receipt with version, build, commit and process IDs.
Each wait is bounded; timeout never escalates to forced termination.

Exit 0 means the requested operation was verified, or that an unattended run
(`--newer-only` or `--wait`) found nothing to do. Exit 75 means refusal or an
unverified transition; inspect the JSON reason. There is no force option, raw
Quit or signal-based termination path. Do not delete bus records, change
channel bindings or override lifecycle state to turn a refusal into success.

An older running process without this protocol refuses before any restart
request is sent. Installing a newer bundle does not give an already running
older process a capability. The Founder must close that older process at a
chosen idle moment once, then open the newly installed version.

Options:

- `--newer-only` restarts only a process that did not load the installed
  binaries.
- `--wait N` retries the refusals listed above for up to N seconds
  (at most 86400); `--interval N` sets the pause between attempts.
- `--announce` plays the outcome and speaks it with the `Zosia` voice
  (`RESTART_ANNOUNCE_VOICE` overrides it).
- `--detach` runs in the background and prints the waiter's PID and log.
- `--quit-only` is for an installer that must perform a copy after the old
  process exits; it uses the same native reservation, does not launch another
  process and takes no `--newer-only` or `--wait`.
- `--app PATH` selects a bundle and still requires its running capability.
- `--timeout N` accepts 0 < N <= 60 seconds per wait (default 30).

Per-process capability and request/receipt files live in `restart-if-idle/`
beside the canonical runtime lease. PID, random process nonce and request UUID
must match. Requests expire after 30 seconds; replay into another generation
cannot authorize shutdown. These files contain lifecycle metadata, never audio
or transcript text. Request/receipt files remain as diagnostic evidence.
