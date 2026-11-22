# 0025 - In-process plugin dylibs are an explicit exception

**Status:** Accepted (2026-11-22)

## Context

RFC 0002 requires both an out-of-process RPC backend (crash isolation, any language) and an
in-process `dylib` backend behind a feature flag. Auro also has a strict rule about dynamic view
code: [ADR 0008](0008-hot-reload-mechanism-revision.md) rejected `cdylib` swapping of GPUI view code
after the feasibility spike, and the hot-reload architecture keeps host state outside patched code.

Those two decisions are easy to confuse. A dylib plugin is not a view patch, but it is native code in
the app's address space: a panic or abort can take down the app, ABI drift can cause undefined
behaviour, and no capability grant limits what the library can do directly.

## Decision

Add a `plugins-dylib` feature, off by default. With the feature off, attempting to load a dylib is a
named error. With it on:

- the loader checks exported ABI and SDK version symbols before calling the entry point;
- the entry point uses raw bytes only — no `Entity`, `Window`, or any GPUI type crosses the C ABI;
- the call is wrapped in `catch_unwind` so a Rust panic becomes a typed error where possible;
- no plugin or UI hot reload is provided; the library is loaded at startup and never patched.

The capability checks remain at the effect sites in the host. A dylib plugin is trusted native code,
not a sandboxed extension. [Spec 0036](../specs/0036-plugin-supervision.md) records the contract and
tests.

## Consequences

- A plugin author can ship a Rust dylib without an IPC hop, at the cost of losing crash isolation.
- An ABI or SDK mismatch fails before the entry call with a typed error instead of undefined
  behaviour from a wrong struct layout.
- App bundles that enable the feature carry a dynamic loader and must pin their plugin SDK.
- The distinction from ADR 0008 must be kept explicit in docs and review: no view code, no patching,
  no host-state swap. A future proposal that blurs that line needs a superseding ADR.

## Alternatives

- **Only out-of-process plugins.** Rejected: RFC 0002 explicitly requires the in-process option for
  low-latency, same-toolchain extensions.
- **WASM/WASI sandbox.** Rejected for this stage: a larger host and ABI story; deferred to future
  possibilities in RFC 0002.
- **Allow dylibs by default.** Rejected: the default build must not inherit ABI risk or a dynamic
  loader that most apps do not use.

## Open questions

- Full ABI stability for third-party dylibs requires a published SDK contract, not only a version
  symbol (maintainers).
- Whether availability events should automatically restart a failed dylib is a later policy decision
  (maintainers).
