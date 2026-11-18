# 0033 - Update journal trust boundary

**Status:** Implemented (2026-11-18; Unix tests pass locally, Windows tests pass on GitHub Actions
`windows-latest`; metadata freshness, rotation and revocation are proposed in
[RFC 0001](../../../rfcs/0001-update-metadata-trust.md))

Spec 0026 introduced a durable journal and a restart helper. The helper re-verifies the signed
manifest and artifact, but the journal itself names the verification key, staged artifact, platform
target and relaunch command. This spec closes that local trust boundary. It does not introduce
TUF-style repository metadata; that design needs the RFC first.

## Scope

- `ApplyJournal` path validation before a journal is read, written or used to spawn/await the
  helper.
- The journal file, its parent directory, its ancestors (Unix) and the sibling lock file.
- Unix ownership/mode/symlink checks and Windows reparse/owner/DACL checks.
- A typed failure for an untrusted path, tested on Unix and on Windows CI.

## Out of scope

- Authenticity of the journal's contents beyond path trust. A journal in a trusted directory is
  written by the current user; that user can already run code as themselves.
- Root/Administrator compromise, a compromised same-user session, kernel attacks or physical
  access.
- Full ancestor ACL reconstruction on Windows, and a general TOCTOU-proof installer handoff.
- Freshness, downgrade, replay, freeze, key rotation, revocation and mirror policy. Those are
  metadata properties, modelled in [RFC 0001](../../../rfcs/0001-update-metadata-trust.md).
- A new journal format or a migration to TUF metadata.

## Decisions

### D1 - The journal path is checked fail-closed, on the opened object

`ApplyJournal::load` opens the journal read-only, validates the opened file, and reads the same
handle. `save` validates the parent before creating the temporary file and validates the persisted
journal afterwards. `open_lock` validates the parent and the opened lock handle before the helper
takes the lock. A path that cannot be proven private is `UpdateError::JournalTrust`; there is no
"warn and continue" mode and no opt-out in the public API.

Path checks are not lexical. They use `symlink_metadata`/`fstat`-style metadata on opened handles,
`O_NOFOLLOW` on Unix and `FILE_FLAG_OPEN_REPARSE_POINT` on Windows.

*Rejected:* a string check for `..` or a well-known directory prefix. It cannot distinguish a
private directory from a shared one with the same spelling, and it does not survive a rename.
*Rejected:* trusting the journal because its contents are signed. The signature key travels in the
same file; internal consistency does not establish who wrote it.

### D2 - Unix: owner, mode, symlinks and ancestors

The journal file and its immediate parent must be owned by the effective user. The immediate
parent must not have a group/other write bit. A journal file must be a regular file and must not be
group/other writable. A lock file follows the same rule. The journal and lock files are opened
with `O_NOFOLLOW`, so a symlink at the final path is refused. Parent directories are opened with
`O_DIRECTORY` and the resolved directory is validated; a private symlinked parent (for example the
macOS `/var` symlink or a user-owned link to a private data directory) is accepted only when its
resolved owner/mode pass the same checks.

The walk from the immediate parent to the filesystem root rejects an ancestor that is not owned by
the effective user or root, or that is group/other writable without the sticky bit. A root-owned
sticky ancestor such as `/tmp` is allowed for an ancestor, because another user cannot remove or
rename an entry owned by this user there; the immediate journal directory still has to be private.
New directory components are created with mode `0700`; new files are created with mode `0600`.

This is a path-replacement check, not a general filesystem immutability claim. The same-user
principal can rename its own files; that is out of scope.

### D3 - Windows: reparse points, owner and DACL

The journal, lock and parent are opened with `FILE_FLAG_OPEN_REPARSE_POINT` (and
`FILE_FLAG_BACKUP_SEMANTICS` for the directory). The handle must not carry
`FILE_ATTRIBUTE_REPARSE_POINT`; a symlink, junction or other reparse point is refused.

`GetSecurityInfo` supplies the owner and DACL. The owner must be the current user, `SYSTEM`,
`Administrators`, `CREATOR OWNER` or `CREATOR OWNER RIGHTS`. Every allowed ACE that carries a
write/delete/ACL right (`FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `FILE_WRITE_EA`,
`FILE_WRITE_ATTRIBUTES`, `FILE_ADD_FILE`, `FILE_ADD_SUBDIRECTORY`, `FILE_DELETE_CHILD`, `DELETE`,
`WRITE_DAC`, `WRITE_OWNER`, `GENERIC_WRITE`, `GENERIC_ALL`) must name one of those principals. A
missing DACL, an allowed object ACE or any other ACE type is refused rather than interpreted.
Creator-owner ACEs are allowed because the object's owner is checked separately and only the
owner receives them.

The Windows tests in this change ran on GitHub Actions `windows-latest` and passed: a private
journal directory is accepted, a junction directory is refused, a permissive `icacls` DACL is
refused, and a symlinked journal is refused (the symlink test skips when the runner lacks the
symlink privilege). This is Windows runtime evidence from CI, not from the Linux development host.
The Windows path does not walk ancestors; a shared ancestor that can rename the private directory
is a residual gap. See D4.

### D4 - Remaining limits are stated, not papered over

- A local Administrator/root or the same user can replace the files; the threat model is another
  local user, not privilege escalation.
- Windows ACL semantics are broader than the subset above: callback/object ACEs, central access
  policies and inherited ACEs that name a group are refused even when they would be harmless. That
  is deliberate fail-closed behaviour.
- Windows does not validate ancestors. The app must use a per-user data directory; an ancestor that
  grants delete-child to another user can still replace the directory.
- The helper verifies the artifact before applying it, but a writeable staging directory still
  leaves a verify-then-open window. Moving the verified bytes through a held descriptor and asking
  the installer to consume that descriptor is platform-specific and remains open.
- The journal may point at a staged artifact or relaunch executable outside the journal directory.
  Those paths are user-chosen and are not re-homed; the journal's directory is the trust anchor,
  not a sandbox for every path it names.

## Interfaces

`UpdateError` adds:

```rust
#[error("update journal path `{}` is not trusted: {reason}", .path.display())]
JournalTrust { path: PathBuf, reason: &'static str },
```

`crates/auro-update/src/trust.rs` is private to the crate:

```rust
pub(crate) fn open_journal(path: &Path) -> Result<File, UpdateError>;
pub(crate) fn validate_parent(path: &Path) -> Result<(), UpdateError>;
pub(crate) fn open_lock(path: &Path, create: bool) -> Result<File, UpdateError>;
pub(crate) fn validate_lock_path(path: &Path) -> Result<(), UpdateError>;
```

`ApplyJournal::load` maps a missing journal to `JournalMissing` before the path check can turn it
into an I/O error. The lock path must still be the journal's sibling; that rule is checked before
the lock path itself is opened.

## Acceptance

Unix tests in `crates/auro-update/src/journal.rs`:

- `accepts_a_private_journal_directory_and_file`
- `rejects_a_world_writable_journal_directory`
- `rejects_a_world_writable_journal_file`
- `rejects_a_symlinked_journal`
- `rejects_a_shared_ancestor_directory`
- `refuses_a_symlinked_lock_file`
- `accepts_a_journal_in_a_private_symlinked_directory`
- `refuses_to_create_a_journal_in_a_shared_directory`

The existing journal/helper tests continue to pass (100 tests in `auro-update` on Linux). The
Windows job runs the same crate tests; the `windows-latest` CI log shows 76 `auro-update` tests
passing, including `windows_rejects_a_junction_journal_directory`,
`windows_rejects_a_permissive_journal_dacl` and `windows_rejects_a_symlinked_journal_file`. The
local Linux host cannot run the Windows code; the full-crate Windows cross-check is also blocked
here by `ring`'s C build requiring `lib.exe`.

`cargo fmt --all --check`, `cargo clippy -p auro-update --all-targets -- -D warnings`,
`cargo test -p auro-update`, and `mdbook build docs` must pass.

## Open questions

| Question | Where it settles |
|---|---|
| Windows ancestor ACL policy | Follow-up; needs a concrete DACL model and tests |
| Windows symlink privilege coverage on runners without developer mode | CI follow-up; the test skips rather than failing |
| Staging-directory and installer-handoff TOCTOU | [RFC 0001](../../../rfcs/0001-update-metadata-trust.md) |
| Metadata expiry, generation, key rotation, revocation and mirror policy | [RFC 0001](../../../rfcs/0001-update-metadata-trust.md) |
