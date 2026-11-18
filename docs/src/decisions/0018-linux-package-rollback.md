# 0018 - Package rollback requires an explicit older artifact

**Status:** Accepted (2026-11-18)

## Context

The Linux package path can install `.deb`/`.rpm` files and elevate through `pkexec`. A bad update
still needs a way back. The operating system package manager has no generic previous-version store:
`dpkg` and `rpm` own their databases, caches are optional local state, and a downgrade can break
dependencies. An automatic rollback would have to invent a safe old package and decide when it is
valid.

## Decision

`LinuxPackageInstaller::rollback` accepts a caller-supplied, already-staged older package. It runs
`dpkg --force-downgrade -i` or `rpm -U --oldpackage`, reusing the same elevation and held-descriptor
semantics as install. Auro does not download, discover or repack the old package, and does not
automatically invoke rollback on failure.

The caller remains responsible for authenticating the rollback artifact through the same manifest
path and for deciding whether the downgrade is safe. `ApplyJournal::mark_rolled_back` is available
but is not wired to a failure hook.

## Consequences

**Easier**

- An app that retains an old package has a tested, elevated rollback path.
- Intent is explicit: no normal upgrade is silently reinterpreted as a downgrade.
- The privileged command and error taxonomy stay identical to install.

**Harder**

- Rollback is only possible when the caller has the older package.
- Dependency repair and downgrade safety remain package-manager policy.
- A failed update does not self-heal automatically; an operator or app must invoke rollback.
- A future journal-driven rollback needs artifact retention and a policy decision.

**Forbidden**

- Claiming automatic rollback when no older artifact exists.
- Querying an unauthenticated package cache and feeding it to root.
- Hiding a downgrade behind a normal `-U`/`-i` invocation.

## Revisit when

- Release artifact retention stores a signed previous package.
- A package transaction policy (holds, channels, dependency repair) is specified.
- The apply journal gains an explicit rollback plan and failure hook.
