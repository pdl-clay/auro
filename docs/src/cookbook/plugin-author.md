# Plugin author cookbook

This chapter turns the plugin host specifications and decisions into an author path. It describes
the contract implemented by `auro-plugins` in `0.1.0-alpha.13`. The sources are
[spec 0034](../specs/0034-plugin-manifest-index.md),
[spec 0035](../specs/0035-plugin-rpc.md),
[spec 0036](../specs/0036-plugin-supervision.md),
[ADR 0024](../decisions/0024-plugin-host-stage-1.md) and
[ADR 0025](../decisions/0025-in-process-plugin-dylib.md).

## What a plugin is today

An out-of-process plugin is a native child process. The host spawns the canonical entrypoint of a
pinned artifact with `std::process::Command`; it does not download, invoke a package manager or
run a shell string. The host passes the channel endpoint through `AURO_PLUGIN_ENDPOINT`.

The isolation is from crashes, not privileges. The child is native code running as the same user.
Capabilities constrain only effects performed through the host; they do not constrain the process
itself.

> **Security warning.** A plugin is trusted native code with the user's authority. A plugin that
> bypasses the host can read the user's files, open sockets and spawn processes with the user's
> privileges. The signed index and verified subject are identity checks, not an operating-system
> sandbox. Do not treat a capability grant as containment of plugin code.

The `plugins-dylib` feature adds a different, explicit exception: an in-process library that shares
the app's address space. A panic or abort can take down the app, ABI drift can cause undefined
behaviour, and no capability grant limits what the library does directly. It is trusted native code,
not a sandboxed extension. The feature is off by default.

## Host prerequisites

The app opts in with the default-off `plugins` feature on the `auro` crate. The `plugins-dylib`
feature implies `plugins` and is separately opt-in. Without `plugins`, the facade does not compile
`auro-plugins`.

The app config uses `[plugins]` with an allowlist. `enabled` defaults to `false` and `allow` to an
empty list; unknown keys are rejected. An empty allowlist loads no plugin even when `enabled` is
`true`.

```toml
[plugins]
enabled = true

[[plugins.allow]]
id = "com.example.notes"
version = "0.1.0"
```

The index is static, signed and pinned into the build. The shipped file
`crates/auro-plugins/index/plugins.json` is signed over `[]`; the matching public key is pinned in
source and the secret is not in the repository. `PluginIndex::parse_signed` verifies the Ed25519
signature and rejects oversized indexes, duplicate ids and invalid versions or hashes. Nothing is
fetched, refreshed or upgraded at runtime; an entry becomes available only through a reviewed change
to the signed index. The shipped index is empty, so no third-party plugin loads today.

## Author lifecycle

1. **Create a plugin crate.** There is no author SDK crate and no plugin scaffold emitted by
   `auro new`. The plugin side must currently speak the documented wire protocol itself. The
   following block records the missing API; none of the lines is runnable and no plugin-side
   helper name is defined by this repository.

   ```rust,ignore
   // AUTHOR SDK DOES NOT EXIST YET.
   // There is no plugin-side crate, no registration function and no plugin main helper here.
   // The supported contract today is the length-prefixed JSON protocol in spec 0035.
   ```

2. **Write the versioned manifest.** The wire struct uses kebab-case names and
   `deny_unknown_fields`; all fields are required. The shape below is the format only, not a
   manifest that would load with the shipped empty index.

   ```json
   {
     "format-version": 1,
     "id": "com.example.notes",
     "version": "0.1.0",
     "sdk-version": "0.1.0-alpha.13",
     "declared-subject": "metadata only",
     "artifact-sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
     "entrypoint": "bin/notes-plugin",
     "commands": ["read_file", "write_file", "ping"],
     "capabilities": [
       { "capability": "fs", "scope": "/notes" }
     ]
   }
   ```

   `format-version` must equal `1`. `sdk-version` must equal `auro_plugins::sdk_version()` exactly;
   comparison is string equality, not semver compatibility. `declared-subject` is metadata and is
   never used for policy. `id` needs at least two dot-separated labels, `commands` are ASCII
   identifiers without duplicates, and `artifact-sha256` is 64 lowercase hex characters. Limits are
   64 KiB for the manifest, 1024 bytes per string, 256 commands and 64 capabilities. Unknown
   fields are rejected.

3. **Derive the subject from verified identity.** The host builds
   `plugin:<id>@<version>.<artifact-hash-prefix>.<instance-id>` only after the index entry,
   artifact hash, manifest digest, SDK version and app allowlist pass. `VerifiedSubject` has a
   private field and no public constructor; `PluginRegistry::resolve` is its only producer. A
   restart needs a new instance id and inherits no grants.

4. **Propose the index entry.** Add a reviewed change to the signed static index with the entry's
   `id`, `version`, `crate_name`, `artifact_sha256` and `manifest_sha256`, and re-sign it. Spec 0034
   calls this a reviewed repository change; it is not an author-callable API. The signing key,
   rotation and revocation are open decisions, so the maintainers must settle them before a
   third-party entry is published.

5. **Test against host policy.** Build a fixture plugin and run it against the host acceptance
   cases in `crates/auro-plugins/tests/`. There is no author-facing test harness; the tests
   exercise host-side resolution, negotiation, effect checks and supervision.

## RPC contract

`Hello` carries `protocol_version`, `sdk_version`, `schema_digests` and `capabilities_hash`.
`PROTOCOL_VERSION` is `1`, `MAX_FRAME_BYTES` is 64 KiB, and frames are length-prefixed JSON with a
little-endian `u32` length. The digest keys produced by `schema_digests()` are `command.request`,
`command.read_file`, `command.write_file`, `command.ping` and `event.file_changed`.

The host regenerates the expected protocol version, SDK version, per-command/event digests and
capabilities hash from its typed schemas and the loaded manifest. Comparison is exact equality:
no semver-compatible-enough rule and no downgrade mode. A mismatch, missing or extra digest, old or
future version, divergent capabilities hash or unknown field produces a typed `NegotiationError`
and no dispatch. Negotiation is terminal per instance: `NotConnected` to `Handshaking`, then
`Ready` or `Failed`; a later call on a failed instance returns `HandshakeRequired`.

Wire types use `deny_unknown_fields`. `Request` carries `request_id` and a typed command;
`PluginCommand` has `read_file`, `write_file` and `ping`, and `CommandPolicy` exposes only those
variants. Plugin-supplied strings never select handlers. No wire field can carry a subject, scope
or grant; such a field is rejected during deserialization.

The host applies a finite per-call timeout, a bounded request queue with typed backpressure, and
child reaping. `DEFAULT_CALL_TIMEOUT` is 5 seconds and `REQUEST_QUEUE_CAPACITY` is 64 in the host;
these are implementation values, not wire-negotiated fields. Handshake, allowlist and effect checks
happen before a request is queued. EOF, `abort`, `exit`, child death, panic and timeout are typed
errors.

## Effects and capabilities

Capability checks happen at the effect site that opens the handle, not when a command is parsed.
`auro-perms` stays pure: it returns a typed `Denied` verdict and performs no I/O. The host injects
its own `VerifiedSubject` into `PluginPolicyExt::allow_plugin` and `check_plugin`; grants come from
the verified manifest and app policy, never from the wire.

`EffectGateway::read_file` and `write_file` lexically reject `..`, prefix and unexpected root
components, canonicalize an existing path, require it under the canonical grant root, and check
`Capability::Fs` with the grant's `scope` immediately before opening. On Unix the open uses
`O_NOFOLLOW`; on Windows it uses `FILE_FLAG_OPEN_REPARSE_POINT` plus reparse-point rejection. The
canonical path is compared again after opening. This narrows the check/open race but does not
eliminate it; a fully handle-based `openat` chain is later hardening. Denial, escape and mismatch
are typed errors with no bytes read or written.

The capability carries a scope string; there is no public `Scopes` type. `shell` grants are an
executable plus a host-side argument policy, never a shell string. Open decisions remain: `net`
normalization and redirect re-checks, the `shell` executable/argument policy, and creating a missing
file inside a grant. Create-open currently fails with `EffectError::UnresolvedPath`.

## Tests and failures

Use the host policy, not the plugin's own claims, as the oracle. The acceptance cases in specs
0034-0036 cover these checks:

- **Fail-closed negotiation:** a canonical `Hello` is accepted; old or future protocol and SDK
  versions, divergent, missing or extra schema digests, and a mismatched capabilities hash each
  produce a typed error and no dispatch.
- **Forged subject:** a field carrying `subject`, `scope` or `grant` on the wire is rejected before
  a handler runs. A manifest-declared subject such as `host:...` is ignored for policy.
- **Impersonation:** grants attach only to `PluginRegistry::resolve` outputs. A different verified
  subject cannot use another instance's grant.
- **Effect escapes:** denial, `..`, symlink escape and Windows reparse points fail with typed
  errors and no write.
- **Dead plugin:** `PluginHost::take_events()` reports `PluginAvailability` with
  `PluginState::Failed` and a typed `PluginFailure` cause. The host stops the worker, reaps or
  kills the child, releases grants and marks the instance unusable. A new instance gets a new
  subject. Flood and silence cases produce `QueueFull` and `Timeout`.

## Explicit limitations

- No operating-system sandbox: the child runs as the same user and the dylib shares the address
  space.
- No remote registry, marketplace, runtime download, implicit upgrade or silent downgrade.
- No plugin or host hot reload. The dylib backend loads once at startup and has no reload or unload
  method.
- No stable third-party ABI in 0.x. The loader checks an exact `ABI_VERSION` and SDK version, which
  rejects drift but is not a compatibility promise.
- Signing key rotation and revocation are an open decision; no third-party index entry is published
  until it is settled.
- There is no author SDK crate, registration helper, remote index client or plugin scaffold. The
  only config keys are `enabled` and `[[plugins.allow]] id`/`version`.

## Invitation

Discuss the contract in [RFC 0002 discussion #104](https://github.com/pdl-clay/auro/discussions/104)
and read [spec 0034](../specs/0034-plugin-manifest-index.md),
[spec 0035](../specs/0035-plugin-rpc.md) and
[spec 0036](../specs/0036-plugin-supervision.md) before proposing an index entry. A plugin written
by an author outside the core team is what remains for the phase 3 exit.
