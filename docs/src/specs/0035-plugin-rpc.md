# 0035 - Out-of-process plugin RPC: fail-closed negotiation and effect boundary

**Status:** Implemented (2026-11-22; stage 2 of [RFC 0002](../../../rfcs/0002-plugin-host.md)).
Stage 1 trust anchor is in [spec 0034](0034-plugin-manifest-index.md).

Stage 1 can prove who a plugin is. It cannot yet let an app call it. This spec defines the first
host boundary: a private child channel, an exact handshake, typed messages, an explicit command
allowlist and the minimum file effects needed to prove that capability checks happen where the
effect is performed. It intentionally stops before crash isolation, bounded queues, backpressure,
child reaping and in-process dylibs; those belong to stage 3.

## Scope

- A private per-user/per-installation channel whose name embeds a secret stored in a private
  directory. A predictable global name is a typed error.
- Spawning the child from the entrypoint of a pinned, verified manifest with no runtime fetch.
- Exact protocol/SDK/schema/capability negotiation with fail-closed behavior.
- Typed, versioned command/reply/event messages; no subject on the wire and no
  `serde_json::Value` escape.
- An explicit, typed allowlist of app commands exposed to plugins.
- Minimal `fs` read/write effects with the capability check at the call site that opens the handle.

## Out of scope (stage 3 or open decisions)

- Timeouts, bounded queues, backpressure, plugin death handling and child reaping. The host stores
  the child handle but does not yet reap it.
- In-process `dylib` plugins and any hot reload of plugin or host code.
- `net` and `shell` effect sites: their normalization/argument policy is still an open decision.
- Creating a file that does not exist yet. Stage 2 rejects `EffectError::UnresolvedPath` rather than
  inventing a create-open contract; spec 0034 keeps that decision open.
- Dynamic registry, runtime downloads, implicit upgrades and plugin signing-key rotation.

## Decisions

### D1 - Private channel, secret in a private directory

The host creates a local socket/pipe endpoint under a per-user directory (`$XDG_RUNTIME_DIR` or a
mode-0700 `auro-<uid>` directory on Unix; `%LOCALAPPDATA%\auro\plugin` on Windows). The endpoint
name embeds a 32-hex secret generated per instance and stored in a mode-0600 token file; the
directory is validated as owned by the current user, non-symlink and not group/other-writable, with
Windows reparse points rejected. A short or predictable name, an occupied name or a denied bind is
a typed host error. The plugin receives the endpoint through a private environment variable, not
through a globally predictable name.

### D2 - The child comes from the pinned artifact

The host hashes the entrypoint file under the plugin root and requires the hash to equal the
artifact hash pinned by the signed index before `PluginRegistry::resolve` derives the subject. The
child is spawned with `std::process::Command` on that canonical entrypoint. There is no download,
package manager call or shell string in the spawn path. Stage 2 does not reap the child; the
structure keeps the `Child` handle so stage 3 can add supervision.

### D3 - Exact, fail-closed handshake

`Hello` carries protocol version, SDK version, a digest per command/event schema (including the
`command.request` envelope) and a capabilities hash. The host regenerates all expected values from
its own typed schemas and the loaded manifest. Any mismatch, missing or extra digest key, unknown
field, old or future version, or capability hash divergence is a typed `NegotiationError`; no
command is dispatched. Compatibility is exact equality, not semver "compatible enough", and there
is no downgrade mode.

### D4 - Typed messages, no subject on the wire

Commands, replies and events are `serde(deny_unknown_fields)` enums with `schemars`-derived schemas
and a protocol version. The host sends a typed `Request { request_id, command }` envelope so the
plugin can echo the correlation id without the host trusting a caller-supplied value. No message
field can carry a subject, a scope or a capability grant: the wire format has no such field, so an
attempt to send one is rejected during deserialization. The host injects its own `VerifiedSubject`
from the loaded instance into every effect check. Capability grants come only from the verified
manifest and the app policy.

### D5 - Explicit typed allowlist, no string dispatch

`CommandPolicy` is a typed set of booleans for the commands the app exposes to plugins (`read_file`,
`write_file`, `ping`). Dispatch matches typed enum variants; a command name supplied by the plugin
is never used to select a handler, and no case/unicode normalization is performed.

### D6 - Capability check at the effect site

`EffectGateway::read_file`/`write_file` validate the path lexically (`..`, prefix and unexpected
root components rejected), canonicalize an existing path, require it to stay under the canonical
grant root, and call `PluginPolicyExt::check_plugin(&VerifiedSubject, Capability::Fs, canonical)`
immediately before opening the handle. Unix opens with `O_NOFOLLOW`; Windows opens with
`FILE_FLAG_OPEN_REPARSE_POINT` and rejects reparse points. The canonical path is compared again
after the open to bound the check/open race. A denial, symlink escape or traversal is a typed error
and no bytes are read or written.

## Security limits

- A child process is isolated from crashes, not privileges: it is native code running as the same
  user. Capabilities only constrain effects that the plugin performs through the host.
- Stage 2 has no timeout or queue bound. A hostile plugin that never answers can stall a call; this
  is explicitly deferred to stage 3 and documented in the host API.
- The check/open re-canonicalization narrows but does not eliminate TOCTOU. A fully handle-based
  `openat` chain is a later hardening step; the current contract fails closed on mismatch.

## Acceptance

1. A canonical hello is accepted; old/future protocol and SDK versions, divergent/missing/extra
   schema digests and a mismatched capability hash each produce a typed error and no dispatch.
2. A Hello or command with an unknown field (including any `subject` field) is rejected before a
   handler runs.
3. A typed command outside `CommandPolicy` is rejected without reaching an effect.
4. A plugin cannot inherit another plugin's grant: the host ignores wire subjects and uses only its
   loaded `VerifiedSubject`.
5. File effects inside a grant succeed; a denial, `..`, a symlink escape and a Windows reparse point
   fail with typed errors and no write.
6. A predictable endpoint name and an occupied/unsafe private directory fail before spawn.
7. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`
   (default and `--all-features`), `cargo test --workspace --locked`, the Windows-target check and
   `mdbook build docs` pass.

## Open questions

| Question | Owner | Closing condition |
|---|---|---|
| Timeout, queue bound and child reaping policy | maintainers | Stage 3 design before the host is exposed by default |
| `net` normalization and redirect semantics | maintainers | Before the first `net` effect site exists |
| `shell` executable + argument policy | maintainers | Before the first `shell` effect site exists |
| Create-open sequence for a missing file inside a grant | implementers | Before the `fs` write of a new file is enabled |
| Plugin signing key rotation and revocation | maintainers + release engineering | Before a third-party index entry is added |

## Relationship to other documents

- [RFC 0002](../../../rfcs/0002-plugin-host.md) defines the five-stage sequence.
- [Spec 0034](0034-plugin-manifest-index.md) defines the manifest, verified subject and index.
- [ADR 0024](../decisions/0024-plugin-host-stage-1.md) records the layering and trust decision.
- [Spec 0016](0016-capabilities.md) D2/D5 delegates canonicalization to the effect caller.
