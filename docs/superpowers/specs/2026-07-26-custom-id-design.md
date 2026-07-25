# Official Client Custom ID Design

## Goal

Allow an installed, unmodified RustDesk client connected to the RCGK OSS server to rename its existing numeric ID to an official-format custom ID such as `farm-pc01` or `workshop-01`.

## Scope

The change is limited to `hbbs`. It implements the existing RustDesk TCP `RegisterPk` rename request that the OSS server currently answers with `NOT_SUPPORT`. It does not add accounts, an API server, address books, aliases, a custom client, configuration hosting, new ports, or support for `@` or IDs outside the official format.

## Validation and ownership

The requested ID must pass `hbb_common::is_valid_custom_id`, which requires 6-16 characters, an ASCII letter first, and then letters, digits, underscore, or hyphen. The request must include the current ID and non-empty machine UUID. The server renames only when the stored UUID for the current ID matches the request. A name already owned by another peer returns `ID_EXISTS`; an invalid name returns `INVALID_ID_FORMAT`; a missing or mismatched owner returns `UUID_MISMATCH`.

## Persistence and runtime state

SQLite performs an ID-only update inside a transaction, preserving the peer GUID, UUID, public key, creation time, user, status, note, and info. The peer map moves the same `Arc<RwLock<Peer>>` from the old key to the new key while holding the map write lock, preserving online state. A repeated successful request is idempotent when the new ID already belongs to the same UUID.

## Protocol flow

1. The stock client validates the requested custom ID and sends TCP `RegisterPk` with `old_id`, `id`, and `uuid`.
2. `hbbs` validates the request and applies existing IP rate limiting.
3. `PeerMap` verifies in-memory conflicts and calls the transactional database rename.
4. `hbbs` returns the matching `RegisterPkResponse` result.
5. On `OK`, the client persists the new ID and resumes ordinary registration. `hbbr` is unchanged.

## Verification

Automated tests cover invalid input, missing ownership, UUID mismatch, duplicates, idempotent retry, field preservation, and peer-map movement. Repository tests and release builds run before an immutable ARM64 image is published. A disposable or backed-up official client is renamed from a numeric ID to `test-pc01`; connection by the new name, restart persistence, password/approval, and relay behavior are verified before closeout.

## Rollback

Before deployment, preserve the current Compose file, full RustDesk state, and current image digest. Rollback restores the old digest and complete state. No private key is regenerated or exposed.
