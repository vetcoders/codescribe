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
end on its own: a take is active or settling, the Founder is writing to an
agent (a focused non-empty composer, keys within the last 15 seconds or an
unsent draft refuse with `founder_composing`; a resting caret in an empty
field after the typing horizon does not block restart), an agent turn holds the runtime
lease, a request expired or the application never answered it. Any other
refusal ends it at once. A refusal after the application accepted shutdown is
never retried. Only one waiter runs at a time; a second one, for example
after another install, exits with `deferred_to_active_waiter`, because every
attempt compares against the bundle installed at that moment. The waiter
appends its JSON lines to `waiter.log` in the handshake directory.

`make install-if-idle` builds through `make install-app`, so the same waiter
owns the swap there too. With `INSTALL_APP_SOURCE` it copies that signed
bundle over the installed one and then arms the same waiter itself, when an
application is running. The installer sends no Quit of its own on either
path, so a Founder composing in the overlay keeps the running generation
until the waiter's `founder_composing` refusal ends. After the
bundle is replaced it watches the running process for up to 90 seconds
(`CODESCRIBE_INSTALL_SWAP_WAIT`) and ends with one of these lines:

- `old generation exited; new generation running pid=…`: the waiter swapped it.
- `restart required; old generation still running pid=…`: the application is
  busy or refused; the waiter keeps trying and `waiter.log` holds the reason.
- `old generation exited without a successor; started the installed app pid=…`:
  the old process left and nothing replaced it, so the installer opened the
  installed bundle. If that launch fails too, the install exits 1.
- `installed app is ready to launch`: nothing was running, nothing is started.

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
- `--announce` plays `Glass` after a verified restart or `Basso` after a
  refusal, then speaks the corresponding text through `cs-say`. It resolves
  `cs-say` on `PATH`, falling back to `~/.local/bin/cs-say`. Set a nonempty
  `RESTART_ANNOUNCE_VOICE` to pass that voice with `--voice`; the configured
  Codescribe speaker is used otherwise.
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

After installation, `make install-app` reads the installed bundle version and
asks `~/.local/bin/cs-say` to announce `Build <version> zainstalowany`. Speech
failure does not change the installation result or print the helper receipt.

To install and arm the same idle waiter without either spoken announcement:

```sh
make install-app INSTALL_ANNOUNCE=0 \
  RESTART_IF_IDLE='./scripts/restart-if-idle --newer-only --wait 21600 --interval 30 --detach'
```

The native shutdown reservation, signature and loaded-binary checks still apply.
