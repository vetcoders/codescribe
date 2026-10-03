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
