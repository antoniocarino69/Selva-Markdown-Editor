# Sync and macOS proposal

Status: design only, 2026-09-28. No sync service has been connected, no vault uploaded, and no macOS build has been verified.

## Confirmed requirements

- Windows and an Apple Silicon M1 Mac.
- The vault is not currently synchronized.
- The user wants dedicated synchronization integrated into Selva, not an external folder-sync product.
- This is architecture work; no deployment or live-vault migration is authorized by this proposal.

## Direction: Selva Sync

Keep Markdown and attachments as the local source of truth. Share the Rust core and egui interface between Windows and macOS. Build a dedicated sync client and a small independently deployable Rust service. Desktop editing must work with the server unavailable, and startup must not wait for network or whole-vault content hashing.

The initial service is a revision store and relay for one user's devices, not a collaborative document editor. Use a Rust HTTP API, transactional SQLite metadata for a single-server deployment, and an immutable encrypted-blob directory behind a storage abstraction. Object storage and a larger database are scaling choices, not initial requirements. Do not deploy until the hosting location, credentials and backup destination are chosen.

## Protocol and data model

- Stable random IDs for vaults, devices and notes. A rename changes encrypted path metadata rather than creating an unrelated note.
- Each revision references its base revision. The server conditionally accepts the expected base inside a database transaction. Divergent offline edits form an explicit conflict with both versions retained. No timestamp-based last-writer-wins.
- A monotonically increasing server sequence feeds a per-device change cursor. The client keeps a durable outbox and uses idempotency keys, so retries after a crash do not duplicate operations.
- Upload immutable blobs before publishing a revision that references them. Keep orphan cleanup separate from revision acceptance; never expose revisions with missing attachments.
- Deletions are versioned tombstones. Retain them and previous revisions under an explicit retention policy; a stale device must not silently resurrect deleted notes. Expired devices must reconcile a fresh snapshot.
- Separate encrypted content blobs from revision/control metadata. The server necessarily sees device identifiers, revision relationships, sizes and traffic timing; do not claim complete metadata secrecy.
- Maintain a local SQLite journal/cache outside the vault. Detect local changes with a filesystem watcher and reconciliation; read/hash changed files, not the whole vault on startup. First upload is a separate resumable background task.

## Encryption, pairing and recovery

Proposed default: end-to-end encryption using maintained cryptographic libraries and authenticated encryption, not custom primitives. Plain Markdown remains on the user's devices; the sync service stores encrypted note bodies, filenames and attachments. Protect the local vault key using platform credential storage.

First device creates the vault key. A new device displays a short-lived pairing request; an already trusted device approves its public key after fingerprint verification and wraps the vault key for it. A pairing code alone must not allow the service to substitute an unverified device key. Device authorization tokens must be scoped and revocable. Removing a device stops future access; it cannot erase data that device already downloaded, and key rotation is needed for future-content separation.

Provide an explicit recovery key flow. Losing every device and the recovery key means the service cannot recover encrypted notes. Keep encryption and pairing as a reviewed implementation milestone before uploading the real vault.

## Product behavior

A compact **Sync** control belongs with the vault controls in the sidebar, respecting the user's request to preserve editor height. Show Offline / Uploading / Downloading / Up to date / Conflicts based on acknowledged server state. Do not equate server acceptance with delivery to every device; expose the last device acknowledgement separately.

First-run flow: create vault sync, show recovery key, pair the Mac, then perform a resumable initial upload. Conflicts open a comparison with Keep both / Resolve. No merge should destroy the divergent originals. Automatic merging and live multi-cursor collaboration are outside v1.

## Current gaps in this repository

- `save_current_file` checks the previous text, then writes directly with `fs::write`. The check/write gap does not protect against an external concurrent writer; direct writes can expose partial files.
- External updates require Refresh. There is no filesystem watcher or incremental reconciliation.
- Conflict handling blocks navigation and asks the user to copy text. There is no conflict comparison/recovery interface.
- Image lookup uses filenames rather than unambiguous vault-relative paths; duplicate attachment names can resolve incorrectly.
- Clipboard image paste explicitly checks Ctrl+V rather than the platform command modifier.
- The initial vault falls back to the process working directory, which is unsuitable for a Finder-launched app bundle.
- There is no macOS packaging or verified macOS build. Cargo.lock is ignored; application releases should pin and track it.

## Storage foundation for the dedicated client

1. Extract filesystem operations from the UI into a vault module. Keep machine-specific preferences and rebuildable indexes outside the synchronized vault.
2. Write via a unique temporary file in the same directory, flush it, then perform a platform-tested replacement. Preserve recovery versions. This prevents partial-file exposure; it does not solve distributed conflicts by itself.
3. Retain the revision read by the editor. If disk content changes, reload automatically only when the editor is clean. With unsaved local edits, retain both versions and offer Compare / Keep both / Resolve. Never silently select the latest timestamp as the user's intent.
4. Watch for create/change/rename/delete events with debounce, handle atomic replacement events, and reconcile after missed events or resume. Update only affected rows and cached content; do not re-read the whole vault on each event.
5. Make paths relative to the vault; normalize comparisons carefully for Windows/macOS case and Unicode differences without silently renaming existing files. Represent dedicated-service conflict branches in the conflict UI.
6. Keep network downloads separate from startup; make initial vault download progress explicit. Keep local startup filename-only and content indexing on demand.
7. Treat attachments and recoverable deletions as part of the sync design. Document which recovery folders are synchronized. Synchronization does not replace a separate backup.

## macOS milestone

- Compile and test on a Mac or macOS CI runner; target `aarch64-apple-darwin` for the confirmed M1 Mac. Intel is outside the initial scope.
- Keep eframe/egui initially. Check the existing dependency versions on macOS rather than combining the port with a large dependency upgrade.
- Use Command shortcuts consistently, test text/image clipboard, file dialogs, focus, undo, keyboard layouts, Retina rendering and window restore.
- Use a first-run vault picker, platform application-data directories and a stable application identifier. Do not synchronize absolute Windows paths.
- Produce a proper `.app` bundle with icon and metadata, then a distributable archive or DMG. Plan Developer ID signing and notarization for normal external distribution. Credentials remain outside the repository.
- Test startup and note editing on real macOS hardware. A passing cross-target compile is not a usability test.

## Delivery order and acceptance

1. Filesystem/save/conflict refactor and local revision journal, preserving Windows responsiveness.
2. M1 macOS build and a testable `.app` bundle; validate on the actual Mac.
3. Dedicated protocol prototype with two simulated clients and disposable vaults: offline edits, retries, conflicts, renames and deletions.
4. Encryption, verified device pairing and recovery, tested before using real notes.
5. Deploy to the selected server, configure encrypted-data backups and retention, then perform a two-device pilot on a copy of the vault.

Acceptance: offline edits on both devices survive reconnection; divergent edits preserve both versions; edit-versus-delete and rename conflicts are recoverable; attachments survive transfer; remote edits never erase a dirty editor buffer; Unicode and case collisions are surfaced; restart during a save/upload leaves a recoverable complete revision; vault startup does not depend on network availability or full content indexing; an unauthorized/revoked device cannot fetch new revisions; restoring a server backup and replaying a client outbox is idempotent.

Open decisions: server hosting (user infrastructure or a dedicated hosted instance), availability/budget, retention limits and the recovery experience. No cloud provider or subscription has been selected.

## Sources

- egui/eframe platform support: https://github.com/emilk/egui
- Rust macOS targets: https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html
- Syncthing synchronization and conflict semantics: https://docs.syncthing.net/users/syncing.html
- Apple distribution/notarization documentation: https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution
