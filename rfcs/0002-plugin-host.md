# 0002 - Plugin host: out-of-process RPC and opt-in in-process dylibs

- **Status:** Accepted
- **Author(s):** @pdl-clay
- **Created:** 2026-11-22
- **Supersedes:** none

## Summary

Add a plugin host as a new `auro-plugins` crate. The default backend runs each plugin as a child
process and talks to it through typed RPC, so a plugin crash does not take down the app. A second
backend loads native `dylib` plugins in-process behind an explicit feature flag, with ABI and panic
risks stated up front. Plugins are identified by a verified subject derived from the artifact,
receive deny-by-default capability grants enforced at the call site that performs the effect, and
are discovered through a signed, static index before any dynamic registry exists.

This changes crate layering, the IPC contract and the capability model, so it is an RFC before it is
an implementation. Problem discussion:
[#104](https://github.com/pdl-clay/auro/discussions/104). Adversarial design review:
`/tmp/auro-phase3-risk.md` (also `native-gui/docs/auro-phase3-risk-2026-11-22.md`, commit
`83b5d00`); its eleven invariants and gates are reflected below.

## Motivation

An app that needs an integration it does not own has two bad choices today: link arbitrary Rust code
into the app process, where a panic or `abort` takes the app down and there is no permission
boundary, or keep the integration in first-party code. Phase 3 exists to make extension safe enough
to ask a third party to publish one.

`auro-perms` already has typed capabilities, deny-by-default grants and compiled glob scopes, but
spec 0016 defers the plugin-host wiring: the policy has no consumer that can be granted to code the
app did not compile. Spec 0016 also explicitly excludes OS sandboxing and first-party code from its
threat model. The most important consequence is stated as a non-goal below: **the host is not a
sandbox**. A capability grant mediates effects that go through the host; it cannot stop a plugin
process from calling `std::fs`, sockets or `exec` directly.

Spec 0015 already solved an analogous trust problem for single-instance pipes; this RFC must not
repeat the predictable-name or shared-directory mistakes there. The roadmap exit for phase 3 is a
plugin published by someone outside the core team; this document does not meet it.

## Non-goals

- **No sandbox and no privilege isolation.** A child plugin process runs as the same OS user as the
  app and can perform arbitrary filesystem, network and process operations outside the host.
  Capabilities are *host-mediated effect scoping*, not a security boundary against native code.
  This wording must be copied into `docs/SECURITY.md` when the host lands (see below).
- **No in-process isolation.** A `dylib` plugin has full access to the app process by construction:
  same address space, same user, all statics and all FDs. The feature flag is an opt-in risk
  acknowledgement, not a boundary.
- **No OS sandbox in phase 3.** Landlock, seccomp, AppContainer and similar mechanisms are out of
  scope; introducing one would need a separate ADR before this design can claim more.
- **No hot reload of plugin code or of the out-of-process host.** Spec 0004 excludes both; process
  restart with a versioned snapshot remains the documented fallback.
- **No per-window grants.** Issue #26 (window show/hide) is still open, so this RFC decides that a
  plugin is *not window-scoped* and receives no window API.
- **No allow-all grant, no interactive upgrade prompt, no dynamic/marketplace fetch in phase 3.**
  Expanding a grant requires a new app release and a new plugin version.

The copy for `docs/SECURITY.md`, to land with the implementation PR:

> Auro capabilities scope the effects that go through the host. Plugins are native code: an
> out-of-process plugin runs as the same OS user and can use the filesystem, network and processes
> directly, and an in-process dylib plugin shares the app's address space. A grant is not a sandbox
> and does not constrain code that bypasses the host.

## Guide-level explanation

An app opts into plugins and names the ones it trusts in `Auro.toml`:

```toml,ignore
[plugins]
enabled = true

[[plugins.allow]]
id = "dev.example.notes-sync"
version = "1"
capabilities = ["net:notes.example.org", "clipboard:read"]
```

The app registers the host during setup and calls plugin commands through typed signatures. The
plugin is a separate crate implementing the published SDK and carrying a manifest (id, semantic
version, exact SDK version, commands and events, requested scopes, entry point, artifact hash).
An external author publishes it and adds it to a signed static index; the app resolves an id to one
pinned version from that index. Nothing is fetched at runtime in phase 3.

The default backend starts the plugin as a child process. The app owns the host; the plugin receives
a private channel and no ambient authority over host effects. If the plugin exits, pending calls get
a typed error, an event is emitted and the child is reaped; the app keeps running. Opting into
`plugins-dylib` loads the same plugin in-process. That backend requires an ABI/version check and is
explicitly unsafe with respect to panics and duplicated statics.

## Reference-level explanation

### Crate boundaries and ownership

```
auro-core  ── composes ──> auro-plugins ── uses ──> auro-ipc, auro-perms
                                  │
                     spawns/manages child processes
```

`auro-plugins` sits above `auro-ipc` and `auro-perms` and is composed by `auro-core`. It must not
depend on `auro-ui` or GPUI, and no lower crate may depend on it. The facade exposes it behind a
feature; the default build does not pay for the host. The architecture diagram in
`docs/src/architecture/` is updated when the RFC is accepted and implemented, not in this PR.

The app process owns the host, the child handles, the `StateStore`, the `EventBus` and the policy.
The plugin process is a child of the app, with a private channel whose name contains a
per-user-per-installation secret, in the same spirit as the single-instance token of spec 0015. A
predictable global name is rejected; the channel uses the private directory rules (mode/DACL,
reparse-point rejection) already established there.

### Discovery, identity and trust anchor

- **Index:** a signed, pinned, static list in the repository maps a plugin id to a crate, version,
  artifact hash and manifest digest. It is embedded at build time. No runtime fetch, no implicit
  update and no silent downgrade. Manifest and index parsing uses `deny_unknown_fields`, explicit
  size limits and subject validation.
- **Registration:** the app names allowed ids and versions in `Auro.toml` and calls
  `PluginHost::register` during setup. An unindexed id, an unlisted version, a digest mismatch or a
  bad subject is a named load error; the plugin is not started.
- **Subject:** the effective subject is derived from the *verified* identity: index entry plus
  artifact hash/signature plus the instance id. It is never a free-form field from the manifest.
  A manifest-declared subject is only metadata and is ignored for policy decisions. Two loaded
  plugins with the same effective subject, or a subject in the reserved host namespace, are load
  errors. Restarted instances get an instance-suffixed subject and do not inherit a stale grant.
- **OS-visible identity:** a plugin cannot choose a window title, notification sender, keychain
  service, pipe name or lock file from its manifest; the host derives those from the verified
  subject.

### Command and event contract

- Commands and events are declared in the SDK as Rust signatures; `schemars` derives the wire
  schema, matching the `auro-ipc` model. There is no first-party string dispatch and no untyped JSON
  payload at the call site.
- The handshake exchanges the **exact** protocol version, the **exact** SDK version, a
  `schema_digest` per command and event and a capabilities hash. A future version, a different
  digest, an unknown field (`deny_unknown_fields`), an unknown command or a missing required field
  is a typed `UnsupportedPlugin` error and the plugin is not loaded. No best-effort downgrade, no
  `serde_json::Value` escape hatch, no case/unicode-normalizing name comparison.
- The host keeps an explicit allowlist of app commands exposed to each plugin. The set is not the
  app's full command registry and is not derived from plugin arguments. A request carries plugin
  subject, instance id, request id and typed payload; cross-plugin dispatch is denied unless an
  explicit grant names the target subject.
- Events are typed and versioned. Publishing an event requires permission for that event *type*;
  delivery is to subjects with an explicit grant, never by string topic. Subscriber queues are
  bounded and apply backpressure, so a plugin cannot flood the `EventBus` or the GPUI main thread.

### Capabilities and effect sites

The host consumes `auro-perms::Policy`, which stays pure and performs no I/O. The check happens at
the call site that performs the effect, never only at message ingress: every file open, network
connection, process spawn, clipboard write, notification and keychain access made on behalf of a
plugin calls `policy.check_scope(subject, cap, canonical_value)` in the same function that performs
it. Checking earlier would let a later host change or helper bypass the grant.

Spec 0016 D2/D5 delegates path canonicalization and symlink-escape rejection to the caller. The
host therefore resolves every path argument before the check, rejects `..`, root and prefix
components, uses `O_NOFOLLOW`/`openat` on Unix and reparse-point checks with validated ACLs on
Windows, and revalidates after opening the handle so a symlink swapped between check and open
cannot change the target. For a file that does not exist yet, the contract is to resolve through an
already-opened, grant-rooted directory handle and reject any target that does not stay inside the
granted tree; the exact create-open sequence is an open question with the implementers. A `net`
grant is normalized (scheme, host, port) by the host; a `shell` grant names an executable plus an
argument policy enforced by the host with controlled cwd/env — never a shell string.

`fs`, `net`, `shell`, `clipboard`, `notification` and `keychain` are the scopes in this iteration.

### RPC limits and failure

- Calls are asynchronous off the GPUI main thread. The main thread never blocks on a plugin.
- Per-call timeouts and a bounded request queue with backpressure are mandatory; a plugin that never
  answers cannot stall the app or grow memory without bound.
- `abort`, `exit`, panic in the plugin thread, EOF and timeout all produce typed errors. The host
  reaps the child, releases its grants, fails pending calls and emits an availability event.
  Automatic restart is not in the first iteration; an app may create a fresh instance explicitly.

### Dylib as an explicit exception

The in-process backend is gated behind `plugins-dylib`, off by default. Load requires a checked ABI
and version tuple (`rustc` version, SDK version, dependency versions); a mismatch is a named error.
The boundary uses `catch_unwind` where a panic can be caught, and no GPUI `Entity`, `Window` or
other framework type crosses the C ABI. This does not contradict ADR 0008: that ADR bans `cdylib`
swapping of *GPUI view code* for hot reload, while a dylib plugin is a non-view extension loaded at
startup and never patched. The distinction is documented where both features are described, and a
superseding ADR is written if implementation shows they interact. No plugin or UI hot reload is
provided by this backend.

### Hot reload boundary

The long-lived dev host keeps `Application`, runtime, `StateStore`, `EventBus`, the plugin manager
and snapshots (spec 0004 D1). Plugin processes and dylib instances are external to the snapshot;
patches never swap plugin code in flight. Snapshots carry the plugin schema version and invalidate
when it changes. A patch failure leaves the previous plugin instance running, as it leaves the
previous app code running today.

## Drawbacks

- One child process per plugin adds startup latency, memory and serialization cost; documentation
  must say plugin calls are not free.
- Typed schemas, exact versions and digests add release-engineering work for every SDK release.
- The signed static index and the review process slow third-party adoption.
- In-process dylibs remain fragile: ABI mismatches, duplicated statics and panics can take down the
  app. The feature flag limits the blast radius but does not remove the risk.
- Two backends mean two implementations of the plugin API to test, with deliberately different
  failure semantics.

## Alternatives

- **Do nothing.** Phase 3 has no exit and apps keep linking untrusted code in-process. Rejected.
- **In-process plugins only.** Faster, but a plugin crash is an app crash. Available as the opt-in
  dylib backend, not as the default.
- **OS sandbox (Landlock/seccomp/AppContainer).** Would turn the capability story into a real
  privilege boundary, but needs a separate ADR and platform work; deferred, not promised.
- **WASM/WASI sandbox.** Language-neutral, but a bigger host and ABI story than phase 3 can carry;
  a future possibility, not this proposal.
- **Dynamic runtime registry.** Rejected: unsigned downloads and mutable artifacts undermine trust
  before the static index has been exercised.
- **String-based IPC.** Rejected: it would discard the compile-time safety that defines Auro's IPC.

## Security and verification

Invariants and mandatory tests before the host can be called implemented:

1. **Capability at the effect site** — plugin asks for `/etc/passwd` and for `<grant>/ok`; only the
   second succeeds, and the effect handler is the only caller of `Policy`.
2. **Canonicalization and symlink escape** — `<grant>/link -> /etc`, a symlink swapped between check
   and open, Windows junctions and reparse points, absolute paths and `..`; all denied on
   Linux/macOS/Windows in CI.
3. **Subject bound to identity** — two plugins with the same manifest subject: the second fails to
   load; `subject="host"` and a path-traversal subject are refused.
4. **No RPC amplification** — property tests with unknown fields, wrong types, null required
   fields, case/unicode variants of command names and colliding `$defs`; nothing reaches a handler.
5. **Fail-closed negotiation** — old/future SDK version, divergent `schema_digest`, unknown command
   and mismatched capabilities hash all yield `UnsupportedPlugin` without dispatch.
6. **Crash isolation and limits** — a child calls `abort()`, `exit(1)` and panics; the host stays
   responsive, pending calls fail with typed errors, fds are reaped and RSS/threads stay stable.
   A 10k-call flood and a silent plugin exercise the bounded queue, timeout and backpressure.
7. **Dylib gated** — refused without the feature, ABI mismatch refused with it, panic at the entry
   point caught where possible, no GPUI type in exported symbols (compile-time check plus review).
8. **Events** — plugin A publishing B's event type without a grant is denied, an old event schema
   is denied, and a flood is bounded.
9. **Index and manifest** — path traversal, hostile globs, wrong hash and bad signature fail with
   named errors; no remote fetch occurs.
10. **CI on three operating systems** — fmt, clippy `-D warnings`, tests, scaffold smoke, and both
    feature configurations of the dylib backend on Linux, macOS and Windows.

## Adoption strategy and sequence

Recommended order, with the dependencies made explicit:

1. Manifest format, verified subject and signed static index, plus the `auro-perms` extension for
   plugin scopes.
2. The host RPC with exact-schema negotiation and effect-site capability checks.
3. Crash isolation, timeouts, bounded queues and child reaping.
4. A reference plugin written by the core but published as an external package through the public
   index.
5. Only then hot reload GA, and after that phase 4's webview.

A third-party author publishes a crate implementing the SDK with a versioned manifest, opens a PR
to add it to the signed index, and an app opts into its id and version in `Auro.toml`. The
documentation needs an authoring walkthrough (build, manifest, index entry, grant, failure
handling) before the exit can be claimed. The reference plugin and the Inspector app (issue #13)
are internal validation steps, not the external exit.

This RFC does not satisfy the phase 3 exit. A plugin published by someone outside the core team is
still required. Hot reload GA also cannot claim its `< 500 ms` target today: spec 0004 records
Windows at 813 ms (N=9) and macOS at 509-956 ms, against a 500 ms target. Those numbers are
recorded, not presented as met; the phase 3 criterion must be "measured and documented per OS with
an honest restart-with-snapshot fallback" rather than "target achieved".

## Unresolved questions

Every item below needs an owner and a closing condition; none is assumed silently.

- **Plugin/index signing key, rotation, revocation and downgrade policy** — maintainers plus release
  engineering; close before the manifest spec is written. The index needs a signature/pin format and
  a revocation path before a third-party artifact can be trusted.
- **`net` normalization and redirect semantics** — maintainers; close before the host effect site
  exists. Define whether a redirect to a different host re-checks the grant.
- **Exact create-open contract for a path that does not exist** — implementers; close before the
  `fs` effect site is written. Must work through a grant-rooted directory handle.
- **Dylib ABI gate details** — implementers; close before the feature flag is exposed. A clear error
  is mandatory even if full detection is not feasible.
- **Issue #13 as reference consumer and issue #26 window grants** — maintainers; close before the
  reference plugin. This RFC decides "no window-scoped grants"; #26 stays open for app windows only.
- **Hot reload GA criterion per OS** — maintainers; close before claiming phase 3 completion.
  Record the measured values above; do not restate the target as achieved.

## Future possibilities

A signed plugin registry with revocation, a WASM/WASI backend for non-Rust plugins, OS sandboxing
as a separate ADR, hot reload of plugin code if spec 0004 is explicitly expanded, and per-window
grants or upgrade prompts if the capability model later needs them. None is promised by this RFC.
