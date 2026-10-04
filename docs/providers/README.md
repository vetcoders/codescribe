# Provider registry and migration

Codescribe binds each request lane to a provider and model. Vendor endpoints
are fixed by the vendor contracts in this directory. Custom providers retain
their own endpoint, wire protocol, and Keychain account.

## Custom provider form validation

Adding or editing a custom provider uses the bridge's validation and endpoint
normalization. An endpoint rejected for its scheme or missing host is not saved.
The form shows a localized explanation below Endpoint: enter an HTTP or HTTPS
URL with a host. Focus returns to Endpoint, and editing the address clears its
old validation message. A new save attempt clears previous form errors.

Other save errors use the shared `Error.userFacingMessage` presentation, without
the Rust/FFI enum representation. The detailed error is recorded only in the
`custom-provider-form` diagnostic log with private visibility; the form does
not display that representation. Since the bridge currently carries the
endpoint failure as a `CsError.Config` message, the form recognizes its existing
endpoint-reason sentence. The bridge remains the sole validation authority.

## Settings migration

Legacy settings migrate on load. Identical normalized custom endpoints on the
same wire share a provider row. Different ports, paths, or wire protocols retain
separate rows, even when the hostname is the same. Colliding host-derived IDs
receive a numeric suffix; each lane keeps its own provider and key destination.

The first migrated settings write also records secret-free key relocations in
`providers.pending_key_moves`. This intent survives a settings-only process,
a restart, unavailable Keychain access, or a failed Keychain write. The runtime
loader acknowledges these records only after the Keychain operation succeeds.
Acknowledgment reloads settings under the settings lock to preserve later edits.
Legacy V1 settings retain their backup and follow the same write ordering.

Migration never replaces an existing provider key with a different legacy key.
The differing legacy key stays in the Keychain for recovery. Identical copied
legacy keys can be removed. A successful secret write followed by a failed
settings acknowledgment is safe to retry. Neither secrets nor secret values
are stored in the pending records or migration logs.

## Credential acquisition and UI projections

`CodescribeConfig.provider_access_snapshot` is the explicit background acquisition
boundary for Settings and Setup. It refreshes the existing core bundle, lets the
existing runtime loader complete pending credential imports, and returns provider
rows, key presence, STT lanes and a revision. `provider_access_revision` is a
cache-only check. The view models reject an older generation or revision after a
credential/config mutation and request one subsequent read before publishing.

Passive `available_providers`, settings/lane loads and readiness/capability
projections use files, env and the existing cache. Account presence uses
`cached_account_status`; it does not start a Keychain read for each vendor.
The explicit snapshot distinguishes unavailable/undecodable credentials from
confirmed absence and returns the access failure to the UI.

Settings and Setup share one serial Swift credential executor. Within Rust,
one I/O mutex owns each physical bundle acquisition and read/modify/persist
sequence; the bundle cache's read/write lock is released before Security IPC.
The same cache records unloaded, confirmed missing, loaded and failed reads.
Explicit refresh reuses a completed read or write for five seconds, then can
read storage again. Missing and failed reads share that retry window, preventing
focus/readiness noise from repeatedly prompting. No timer retries on its own.

Save, delete, credential relocation and fan-out use the same I/O mutex and cache.
A denied or undecodable read cannot be treated as an empty bundle for a write.
A failed read retains the previous successful cache, while the explicit UI read
reports the failure. A successful write advances the revision only after storage
returns. Provider row changes also advance it, including a row persisted before
a later secret-write error. Explicit env overrides retain their existing priority.

Cancellation or a hidden view cannot interrupt a synchronous Security call or
release its physical I/O slot. Owned operations finish before their pending state
clears. These scheduling rules do not change Keychain service names, item access
controls, OAuth/API-key selection, or provider endpoint contracts.

### Committed settings projections and transaction leases

Passive Settings, Setup, account metadata and retranscription availability read
`UserSettings::load_projection`: the same document parser and normalization used
by the settings writer, over the atomically committed file. This read neither
acquires the settings credential transaction lease nor persists repairs or
schema/import migrations. If the document is absent, it projects the initial
`.env` values through the same import builder and normalization as the first
writer. Any projected pending rows are in-memory intent, not a persistence
receipt; only the writer commits them. An unreadable existing document does not
become permission to reimport `.env`. It observes a coherent committed document while an
explicit credential edit is waiting for Security, including durable import
cancellation published before that edit. It does not use a substitute cache or
interpret a busy lease as missing settings.

The existing settings transaction lease continues to serialize import
settlement, explicit credential intent and their persistence. Failure to persist
cancellation still forbids the secret edit; failed secret writes retain durable
cancellation and pending failed imports. First writer-capable settings loads
prepare the initial `.env` import under that same lease, using the migration
module's single builder. The existence check, promoted settings and secret-free
pending account rows are persisted together before the ordinary edit. A
concurrent authorized initial load cannot replace an already created document.
Promoted single and batch config edits refuse source or initial persistence errors
instead of creating a document that omits the pending import.
Explicit credential edits also prepare this first intent and durably cancel
their own destination before performing Security I/O. Secret settlement remains
owned by authorized acquisition; passive reads do not prepare or settle imports.
An existing document without pending rows never causes a new secret import.
The loader's file/credential work completes before taking the env
bootstrap mutex; only env publication and cache-based config capture hold that
mutex. Completed bundle values can be mirrored during the one bootstrap window
without starting Security I/O under the mutex. Warm runtime bundle reads return
from the positive cache before the physical I/O mutex; strict explicit refresh
continues to acquire that mutex and report failures. Runtime resolution still
reads the completed cache with explicit env taking priority; it does not need
another env seed after the first production bootstrap closes. A no-reseeding
witness must exercise that production bootstrap lifetime rather than the unit
harness's per-case env bootstrap permission.

`CsProviderAccessSnapshot.account_errors` isolates an unavailable OAuth account
record by provider ID. Decoder details and token JSON do not enter public error
metadata. A malformed record does not hide the registry, unrelated API keys,
custom providers or STT controls. The affected account reports unavailable and
offers Sign out to remove the stored record, then a new sign-in; it is not
presented as a confirmed signed-out account. Whole-bundle acquisition failures
remain distinct from these individual record errors. Snapshots also cover STT
endpoints, so a local endpoint edit advances the view-model generation before
publication and rejects an older in-flight endpoint projection.

Capability matrix uses `effective_agent_workspace_roots_projection`, which
feeds committed settings into the same root resolver as writer-capable callers.
Root precedence, normalization and the default workspace are unchanged. This
passive connector-health read does not acquire the settings transaction lease.

Passive projections and the repair writer share one settings analysis grammar.
Known-field normalization can occur in memory, without asserting a persisted
repair. Unsupported schema versions, malformed JSON requiring file recreation,
and non-NotFound read failures add a secret-free refusal to the existing launch
receipt before runtime capture. The sealed snapshot remains unarmed when that
receipt contains an unrepairable refusal, even without Keychain acquisition.
A missing document still uses the pure initial `.env` projection.

Executed repair actions and backup receipts require an admitted writer-capable
settings/acquiring loader. The writer alone reads an operator pack, creates a
backup, resets/recreates the document and persists its validated candidate.
Requesting a passive snapshot authorizes none of those operations. Refusals keep
the existing process-lifetime diagnostic/quarantine behavior; passive normalized
values do not erase an earlier recorded refusal.
