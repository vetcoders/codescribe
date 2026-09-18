# Configuration repair at launch

The running app repairs supported `settings.json` defects under the existing
settings transaction lock. An invalid `ui.chat_zoom` becomes 1.0; an unknown
`speech.formatting.level` becomes `off`. Other JSON fields, including unknown
extension fields, are retained in the repair write. Truncated/unparseable JSON
is backed up byte-for-byte before a fresh schema-3 default document is written.

Every replacement of an existing file first creates an exclusive, private
`settings.json.bak-<UTC timestamp>-<unique id>` beside it and syncs the backup.
The replacement uses the existing atomic write. Failed backup or validation
prevents replacement. Unknown schema versions and unsupported field types are
reported as `ConfigUnrepairable`, leaving the original file untouched.

The launch snapshot carries a `repair_receipt()` and Settings → User displays
its summary. The bridge also exposes the full structured receipt as JSON.
These reads do not reload settings or run repair. A process logs one
`config_repair actions=<n> backup=<path|none>` line. Unresolved launch errors
keep the mandatory capture lane disarmed, so Settings remains accessible for
recovery; fix the source and restart. An invalid formatting override disables
formatting and is reported without echoing its value.

## Precedence and the optional env file

Existing precedence is unchanged. Supported explicit process overrides win.
The loader intentionally excludes promoted GUI settings from optional `.env`
injection, so stale `.env` entries cannot overwrite `settings.json`. File env
keys still managed by the runtime retain their existing loading behavior.

Deprecated and unknown `.env` keys are **report-only** in this cut. The registry
still names retired LLM endpoint/model replacements with no active readers.
Automatically copying those values would claim a migration that does not work.
The old destructive migration (which dropped legacy values and rewrote the
whole file without a backup) has been removed. The app preserves every byte;
receipts name keys only, never their values. Persistent diagnostic notes recur
on later launches until the file is corrected. These notes are not counted as
successful file changes in Settings.

## Operator pack

The build embeds only `speech.engine.cloud_transcription_endpoint` and
`asr_mode` from the selected Voice Lab pack into
`Contents/Resources/operator-pack/settings.json`. It never embeds the keys
folder or the rest of the profile. The app fills these fields only when empty;
existing choices win. Development can select a checkout with the existing
`CODESCRIBE_VOICE_LAB_SRC` variable. The Voice Lab installer delegates runtime
and public-key setup with `INSTALL_SETTINGS=0`; configuration merging belongs
to the app. Without a pack, no cloud endpoint is invented.

A second repair pass over repaired settings performs no write and creates no
backup. The process retains its first-launch receipt so opening Settings later
still explains the repair. Keychain semantics, prompts, lexicon, audio and
transcript authority are outside this repair surface.
