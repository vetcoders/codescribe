# Restart an idle application

After the Founder authorizes a restart, run:

```sh
make restart-if-idle
# Equivalent from a script:
./scripts/restart-if-idle
```

This command restarts the installed desktop application. `make install-app`
still installs the bundle and does not restart its running process.

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

Exit 0 means the requested operation was verified. Exit 75 means refusal or
an unverified transition; inspect the JSON reason. There is no force option,
automatic retry after a take, raw Quit or signal-based termination path. Do
not delete bus records, change channel bindings or override lifecycle state
to turn a refusal into success.

An older running process without this protocol refuses before any restart
request is sent. Installing a newer bundle does not give an already running
older process a capability. The Founder must close that older process at a
chosen idle moment once, then open the newly installed version.

`--quit-only` is for an installer that must perform a copy after the old process
exits; it uses the same native reservation and does not launch another process.
`--app PATH` selects a bundle and still requires its running capability.
`--timeout N` accepts 0 < N <= 60 seconds (default 30).

Per-process capability and request/receipt files live in `restart-if-idle/`
beside the canonical runtime lease. PID, random process nonce and request UUID
must match. Requests expire after 30 seconds; replay into another generation
cannot authorize shutdown. These files contain lifecycle metadata, never audio
or transcript text. Request/receipt files remain as diagnostic evidence.
