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
