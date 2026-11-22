# 0036 - Plugin supervision, RPC limits and the dylib backend

**Status:** Implemented (2026-11-22; stage 3 of [RFC 0002](../../../rfcs/0002-plugin-host.md)).
Stages 1-2 are in [spec 0034](0034-plugin-manifest-index.md) and [spec 0035](0035-plugin-rpc.md).

Stage 2 proved the trust boundary but a silent or dead plugin could still stall a call, and only
out-of-process plugins existed. This spec adds bounded RPC, child supervision with typed failure and
availability events, plus a strictly opt-in in-process `dylib` backend.

## Scope

- Per-call timeout, bounded request queue with backpressure, and no unbounded wait on the GPUI main
  thread.
- Reaping the stored child; typed errors for abort, exit, panic, EOF and timeout; grant release and
  a typed availability event.
- `plugins-dylib` feature (off by default): ABI/version gate, raw-byte `extern "C"` entry,
  `catch_unwind` and no GPUI types across the boundary.
- Tests for flood, silence, child death, feature-off refusal, ABI mismatch and a caught panic.

## Out of scope

- Remote index, marketplace, `net`/`shell` effect sites, signing-key rotation and hot reload GA.
- Sandboxing: an out-of-process plugin is a same-user child; a dylib shares the app's address space.
- Hot reload of plugin code or UI; spec 0004 excludes both and the dylib backend is loaded once.

## Decisions

### D1 - Bounded calls, typed backpressure

Every plugin call has a finite timeout and a bounded request queue. A plugin that never answers
produces a typed timeout error; a full queue produces typed backpressure instead of unbounded
growth. The host never relies on an indefinitely blocking read from a GPUI handler; callers remain
free to hop to a background runtime before calling. The existing typed allowlist, effect-site
checks and fail-closed handshake from spec 0035 run before a request is queued.

### D2 - Child death is a typed state transition

The host owns the `Child` handle from stage 2. EOF, `abort`, `exit`, a panic in the child or a call
timeout are typed errors. The host reaps the child, drops the stream, marks the instance failed,
releases its capability grants and emits a typed availability event. A dead instance cannot be
reused; a new instance gets a new verified subject (spec 0034). Reaping is best-effort on platforms
where the child cannot be waited synchronously without blocking; no zombie thread is left behind.

### D3 - Dylibs are an explicit, feature-gated exception

`plugins-dylib` is off by default; with the feature off, a dylib load is a named error. With it on,
the loader checks an exported ABI version and SDK version before calling anything. The entry point
uses raw bytes only; no `Entity`, `Window` or other GPUI type crosses the C ABI. `catch_unwind`
contains Rust panics at the boundary and reports a typed error. There is no plugin or UI hot reload.


The loader checks an exported `u32` ABI-version symbol and an exported function returning a NUL-
terminated SDK-version string before it resolves the raw-byte entry point. Both versions must match
exactly; a mismatch is a named error before any plugin code is called.

This does not revisit [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md): that record
bans swapping `cdylib` view code for hot reload. A dylib plugin is a non-view extension loaded at
startup, never patched; [ADR 0025](../decisions/0025-in-process-plugin-dylib.md) records the
distinction and its risks.

### D4 - Failure is observable without GPUI in the crate

Availability events are plain typed values exposed by the host; `auro-plugins` still does not depend
on `auro-ui` or GPUI. An app can bridge them into its own `EventSender` on the main thread.

## Security limits

- Limits protect the host from an unresponsive plugin, not from a same-user malicious plugin: native
  code still has the user's authority.
- A panic that unwinds across an FFI boundary is undefined behaviour in general; `catch_unwind`
  reduces the exposure but an aborting dylib still takes down the app. The feature is opt-in for that
  reason. The regression test exercises `call_entry` with a same-runtime `C-unwind` entry: a
  separately compiled cdylib carries its own panic runtime and cannot reliably unwind into the
  host's `catch_unwind` on every platform.
- TOCTOU at capability effect sites remains bounded by post-open revalidation (spec 0035).

## Acceptance

1. A 10k-call flood is admitted only up to the configured queue bound; excess calls fail with typed
   backpressure and the calling thread stays responsive.
2. A plugin that never replies fails with the typed timeout error within the configured bound.
3. Abort, exit and EOF produce typed errors, the child is reaped, grants are released and a typed
   availability event is emitted; thread/fd counts stay stable where measurable.
4. Without `plugins-dylib`, a dylib load returns the named `FeatureDisabled` error. With it on,
   wrong ABI/SDK versions are rejected before the entry call and a panicking entry is reported as a
   typed panic error.
5. Stage 1/2 tests remain green: verified subjects, fail-closed handshake, effect-site checks and
   safe private directories.
6. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`
   (default and `--all-features`), `cargo test --workspace --locked`, the Windows-target check and
   `mdbook build docs` pass. The default facade build must not compile `auro-plugins`.

## Open questions

| Question | Owner | Closing condition |
|---|---|---|
| Default timeout and queue depth per platform | maintainers | Measured with the reference plugin before the values are called final |
| Whether availability events initiate automatic restarts | maintainers | Stage 4 policy decision; stage 3 reports only |
| Full ABI stability for third-party dylibs | maintainers | Requires a published SDK contract, not just a version symbol |

## Relationship to other documents

- [RFC 0002](../../../rfcs/0002-plugin-host.md) defines the stage sequence.
- [Spec 0034](0034-plugin-manifest-index.md) is the identity/trust-anchor contract.
- [Spec 0035](0035-plugin-rpc.md) is the RPC and effect-site contract this spec extends.
- [ADR 0025](../decisions/0025-in-process-plugin-dylib.md) records the dylib decision.
