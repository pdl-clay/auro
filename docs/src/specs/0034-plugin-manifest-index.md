# 0034 - Plugin manifest, verified subject and signed index

**Status:** Implemented (2026-11-22; stage 1 of [RFC 0002](../../../rfcs/0002-plugin-host.md),
accepted 2026-11-22). No plugin RPC host or process spawning is part of this spec.

Phase 3 adds code the app did not compile. Before any plugin process exists, the host must be able
to answer three questions without guessing: what did the author declare, which artifact was pinned,
and which subject owns the effects that pass through the host. This spec closes that trust anchor.
It does not claim sandboxing: native plugins run as the same user and can bypass host-mediated
capabilities.

## Scope

- The versioned plugin manifest: fields, `deny_unknown_fields`, size and collection limits, command
  and capability declarations, artifact hash and manifest digest.
- The verified subject: derived from a verified index entry, the artifact hash and an instance id.
  A subject declared in the manifest is metadata only.
- The static index: signed, pinned, embedded at build time, with no runtime fetch, implicit update
  or silent downgrade.
- `Auro.toml`: which plugin ids and exact versions an app allows.
- The extension of `auro-perms` for plugin subjects, keeping that crate pure (no I/O).

## Out of scope

- Command/event RPC, process spawning, crash isolation, bounded queues and child reaping (stage 2).
- In-process `dylib` loading (later in phase 3) and hot reload of plugins or of the host
  ([spec 0004](0004-hot-reload.md) excludes both).
- A dynamic registry or marketplace, remote fetch, implicit version selection and grant upgrades.
- `net` normalization/redirect policy and the exact create-open sequence for a file that does not
  exist yet: both are open decisions with owners below.
- Plugin signing key rotation, revocation and downgrade policy: open decision with owners below.
- OS sandboxing, per-window grants, allow-all and upgrade prompts (RFC 0002 non-goals).

## Decisions

### D1 - A versioned, closed manifest

`PluginManifest` is JSON with `deny_unknown_fields` and kebab-case names. It carries
`format-version` (exact `1`), `id`, `version` (semver), `sdk-version` (exact match against the
host SDK), optional `declared-subject` (metadata only), `artifact-sha256`, `entrypoint`,
`commands` and `capabilities`. `commands` use ASCII identifiers; a capability is one of the six
`auro-perms` capabilities with an optional scope string, required for `fs`/`net`/`shell` and
rejected for `clipboard`/`notification`/`keychain`. Limits: 64 KiB manifest, 1024 bytes per string,
256 commands, 64 capabilities. The manifest digest is lowercase SHA-256 of the canonical serialized
manifest and is pinned by the index entry.

### D2 - The subject comes from the artifact, not from the manifest

A verified subject has the shape `plugin:<id>@<version>.<artifact-hash-prefix>.<instance-id>` and is
constructed only after the index entry, artifact hash, manifest digest, SDK version and app
allowlist have been checked. `declared-subject` is kept for author metadata and never used for a
policy decision. `auro-perms::PluginSubject` validates the shape, rejects traversal and control
characters and reserves the `host:`/`app:` namespaces. A second loaded plugin resolving to the same
subject, or any attempt to use a reserved namespace, is a named load error. Restarting an instance
requires a new instance id and therefore a new subject; no grant state is inherited.

### D3 - A signed, pinned, embedded index

The index is a JSON document `{"entries":[...],"signature":"..."}` embedded with `include_str!` at
build time. Its Ed25519 signature covers the compact serialization of the `entries` array.
`PluginIndex::parse_signed` verifies the signature against a pinned public key and rejects an
oversized index, duplicate ids, invalid versions or invalid hashes. The shipped index is empty,
signed over `[]`, and its public key is pinned in source; no private key is committed. Adding an
entry is a reviewed repository change with a new signature. Runtime code never fetches an index,
chooses a newer version implicitly or accepts a downgrade silently.

### D4 - The app names exact ids and versions

`Auro.toml` gains an optional `[plugins]` section with `enabled` and `[[plugins.allow]]` entries
containing `id` and `version`. Parsing uses `deny_unknown_fields`; validation rejects duplicate ids
and empty values. An empty allowlist is legal and means no plugin is available. The facade exposes
the stage-1 API only through the default-off `plugins` feature.

### D5 - `auro-perms` stays pure and checks at the effect site

`auro-perms` gains `PluginSubject`, `Policy::allow_plugin` and `Policy::check_plugin`. They perform
no I/O: no filesystem reads, no time, no randomness. The call site that performs the effect must
canonicalize the value, reject `..`/prefix/root components and symlink escapes, revalidate after
opening the handle, and only then call `check_plugin` with the canonical value. Spec 0016 D2/D5
delegates exactly that responsibility to the caller; the policy matches strings and returns a typed
`Denied` verdict, nothing more. A `shell` grant is an executable plus a host-side argument policy,
never a shell string.

## Security limits

- Neither a signed index nor a verified subject is an OS sandbox. A plugin that bypasses the host
  can read files, open sockets and spawn processes with the user's authority.
- A snapshot of the index is pinned at build time; freshness, revocation and key rotation need the
  open decision below before a third-party plugin can be published.
- No RPC payload exists in stage 1, so argument amplification is not yet testable. RFC 0002
  requires exact schema digests and an app-command allowlist before stage 2 ships.

## Acceptance

1. Hostile manifest fixtures (unknown field, >64 KiB, oversized command list, traversal in
   `entrypoint`, bad hash, wrong SDK version, bogus capability) fail with named errors.
2. A manifest-declared `host` subject is ignored for policy; the resolved subject is always
   `plugin:...`.
3. Signed-index fixtures generated with `auro_sign::SigningKey` prove valid signatures pass and
   bad signatures, duplicate ids and oversized indexes fail.
4. The registry rejects an unindexed id, an unlisted version, a downgrade to an older version, an
   artifact hash mismatch, a manifest digest mismatch, a duplicate subject and a reserved subject.
5. Two instances with different instance ids get different subjects; no grant state is inherited.
6. `auro-perms` tests cover subject validation, reserved namespaces, deny-by-default and lexical
   `..` rejection without any filesystem access.
7. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, the Windows-target check and `mdbook build docs` pass.

## Open questions

| Question | Owner | Closing condition |
|---|---|---|
| How to normalize `net` values and whether a redirect re-checks the grant | maintainers | Written rule in this spec before the first `net` effect site exists |
| Exact create-open sequence for a path that does not exist inside a granted directory | implementers | Documented helper using a grant-rooted directory handle before the `fs` effect site exists |
| Plugin/index signing key, rotation and revocation, including whether the bootstrap key is replaced | maintainers + release engineering | Decision record before any third-party entry is added to the index |

## Relationship to other documents

- [RFC 0002](../../../rfcs/0002-plugin-host.md) is the design this implements; stage 2 adds the
  fail-closed RPC host.
- [Spec 0016](0016-capabilities.md) defines capabilities and delegates path canonicalization to the
  effect caller.
- [ADR 0024](../decisions/0024-plugin-host-stage-1.md) records the layering and trust decision.
