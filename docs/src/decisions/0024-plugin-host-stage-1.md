# 0024 - Plugin host stage 1: trust anchor before RPC

**Status:** Accepted (2026-11-22)

## Context

[RFC 0002](../../rfcs/0002-plugin-host.md) was accepted with a five-step sequence. Implementation
must start with the trust anchor: if the manifest, subject and index are weak, a later RPC host can
only relay hostile identity and scope decisions. The capability model in spec 0016 is deny-by-default
and pure, but it has no consumer for code the app did not compile, and its D2/D5 rules delegate path
canonicalization to the caller. An earlier attempt to treat plugins as sandboxed would be false: a
child process is isolated from crashes, not privileges, and an in-process dylib has full access.

The framework already has `auro-sign` for Ed25519 signatures and SHA-256, `auro-perms` for policy
verdicts, and `auro-ipc` for typed schemas. Reusing them is cheaper and safer than a second trust
stack.

## Decision

1. Add `auro-plugins` above `auro-ipc` and `auro-perms`, composed by `auro-core` behind a
   default-off feature and re-exported by the facade behind the same `plugins` feature. The crate
   does not depend on `auro-ui` or GPUI, and no lower crate depends on it.
2. Stage 1 implements only the manifest, verified subject, signed static index, app allowlist and
   the `auro-perms` extension. No process spawning, RPC, dylib loading or hot reload is added.
3. The subject is derived from the verified index entry plus artifact hash plus instance id. A
   manifest-declared subject is metadata only. Duplicates and reserved `host`/`app` namespaces are
   errors.
4. The index is embedded at build time, signed with Ed25519 and pinned to a public key in source.
   The shipped index is empty and signed over `[]`; no private key is committed. Runtime fetch,
   implicit upgrades and silent downgrades are absent.
5. `auro-perms` remains pure and string-based. The verified subject is an opaque `VerifiedSubject`
   owned by `auro-plugins`; `PluginRegistry::resolve` is its only constructor and plugin policy
   helpers are an extension trait over `&VerifiedSubject`. No public constructor can turn a text
   value into a subject usable by a grant. The effect site performs canonicalization and
   revalidation. Net normalization and the create-open sequence stay open decisions with owners and
   closing conditions ([spec 0034](../specs/0034-plugin-manifest-index.md)).

## Consequences

- A third-party plugin cannot be loaded in stage 1: the shipped index is empty. That is deliberate;
  adding an entry requires a reviewed signature and a signing-key policy that is still open.
- Apps that do not enable the `plugins` feature compile exactly as before.
- The next stage can build the RPC host on verified identity and exact schema digests instead of
  string names; it cannot claim sandboxing.
- The architecture chapter and the workspace crate table must change in the same code PR, because a
  planned crate now exists. The published version and README status copy do not change in stage 1.

## Alternatives

- **Start with the RPC host.** Rejected: it would bind command names and subjects before identity
  is verified, and a later hardening pass would be breaking.
- **Plugin declares its own subject.** Rejected: lateral movement between plugins by copying a
  manifest subject.
- **Unsigned static index or runtime fetch.** Rejected: both let an attacker choose the artifact
  without changing the app.
- **Dependency on `auro-ui` for plugin views.** Rejected in RFC 0002; layering forbids it here.

## Relationship to ADR 0008

[ADR 0008](0008-hot-reload-mechanism-revision.md) rejects swapping `cdylib` view code for hot
reload. This decision does not revisit it: a future `dylib` plugin is a non-view extension loaded at
startup, never patched in flight, and does not carry GPUI entities across the C ABI. The distinction
is documented in RFC 0002 and must be repeated in the later ADR that enables the dylib backend.

## Open questions

- Signing-key rotation and revocation (maintainers + release engineering) before a third-party entry
  is added to the index.
- `net` normalization/redirect policy (maintainers) before a `net` effect site exists.
- Create-open sequence for not-yet-existing files (implementers) before the `fs` effect site exists.

These are tracked in [spec 0034](../specs/0034-plugin-manifest-index.md) with closing conditions.
