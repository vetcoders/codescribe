# Provider registry and migration

Codescribe binds each request lane to a provider and model. Vendor endpoints
are fixed by the vendor contracts in this directory. Custom providers retain
their own endpoint, wire protocol, and Keychain account.

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
