# 0003 - Capabilities at the real boundary

**Status:** Accepted

## Context

Tauri's capability system exists because a web page is untrusted code sharing a process with
privileged commands. Auro's first-party frontend is Rust compiled into the app: calling
`std::fs::write` is not a privilege escalation, it is the app doing its job.

Copying Tauri's model wholesale would produce a permission system with no adversary, which trains
users to click "allow" and reviewers to ignore the manifest.

The genuine runtime boundaries are narrower: third-party plugin code, untrusted content, and the
future webview escape hatch.

## Decision

Capabilities guard **plugins, content, and the webview escape hatch** - not first-party app code.
Grants are deny-by-default, declared in a manifest with scopes (`fs`, `net`, `shell`, `clipboard`,
`notification`, `keychain`), compiled into the host, and checked at the call site that performs the
effect.

## Consequences

**Easier**

- The permission surface is small enough to audit and to document honestly.
- Compile-time type safety covers what a runtime permission model would otherwise paper over.
- Plugin authors get a precise denial (scope, subject, pattern) instead of a generic refusal.

**Harder**

- The security story depends on the plugin host being correct, so it needs its own threat model
  and tests.
- Users will ask "why can't I grant the app access to X"; the answer is "it is your code, it
  already has it", which must be explained rather than implied.
- In-process plugins (`cdylib`) share the address space, so a capability check cannot contain a
  malicious library. Defaulting to out-of-process is a consequence, not a preference.

**Forbidden**

- Adding a capability for something first-party code can do directly.
- A global allow-all mode that ships enabled.
- Treating an in-process plugin as sandboxed.
