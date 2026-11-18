# 0023 - The update journal is a local trust boundary

**Status:** Accepted (2026-11-18)

## Context

The update journal is a small JSON file that names a verification key, a staged artifact, a
platform target, a backup and a relaunch command. The restart helper reads it after the old process
exits and applies the plan. Re-verifying the artifact with the key inside the journal proves that
the two agree; it does not prove who wrote the journal.

A journal planted by another local user can therefore choose its own key, artifact and executable.
On a package apply that plan reaches an elevation prompt with an attacker-supplied package. The
journal and its lock therefore need a directory that only the current user can write.

Rust's standard library exposes ownership and modes on Unix, but not Windows ACLs. A check that is
only `cfg(unix)` would leave the Windows helper with no protection and would make the API's safety
property platform-dependent in an undocumented way.

## Decision

`ApplyJournal` validates the journal's local trust boundary fail-closed on every load, save and
helper lock open. Unix checks the effective owner, group/other write bits, symlinks and ancestors
on opened handles. Windows opens the reparse point itself, rejects reparse attributes, and checks
the owner and DACL for write/delete/ACL grants to principals other than the current user, SYSTEM,
Administrators and the creator-owner SIDs. A path that cannot be proven private is
`UpdateError::JournalTrust`; there is no bypass in the public API.

The library does not claim more than it proves: Windows is compile-verified but not yet
runtime-verified, Windows ancestors are not walked, and a verify-then-open window remains for
staged artifacts consumed by an external installer. [Spec 0033](../specs/0033-update-journal-trust.md)
records the tests and limits; metadata freshness, rotation and revocation are not solved by this
decision and are modelled in [RFC 0001](../../../rfcs/0001-update-metadata-trust.md).

## Consequences

- A shared or attacker-writable journal directory is a typed error instead of a silent risk.
- Tests that use a private temporary directory still pass; the generated app's data directory is
  the intended location.
- Windows CI needs a real execution test. The code is not "done" because it cross-compiles.
- A Windows ancestor that can rename the private directory remains a known gap; the contract is a
  per-user data directory, not any path.
- New directory components are created `0700` and files `0600` on Unix, so an umask of `000` does
  not silently widen the journal.
- The updater remains opt-in. This decision is a prerequisite for a release that enables the
  updater, not a claim that the whole update channel is production-ready.
