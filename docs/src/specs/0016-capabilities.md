# 0016 - Capability manifests and enforcement

**Status:** Implemented (2026-11-17)

Auro does not sandbox first-party app code. This spec fixes the smallest runtime contract for the
real boundaries: plugins, untrusted content and the webview escape hatch. It is deny-by-default,
compiled into the host, and checked at the call site that performs the effect.

## Scope

- `auro-perms`: a `Policy` with deny-by-default grants for `fs`, `net`, `shell`, `clipboard`,
  `notification` and `keychain`.
- Scoped capabilities (`fs`, `net`, `shell`) match a glob pattern against the value being acted on.
- Typed `Denied` errors naming subject, capability and value.
- `auro::perms` re-export for app and host code.

## Out of scope

- Sandboxing first-party app code. If the app author writes it, it runs.
- OS-level sandboxing (App Sandbox, seccomp, AppContainer).
- A manifest file format; the host constructs `Policy` in code today.
- Per-window grants, plugin upgrade prompts and an allow-all mode.
- Runtime I/O: the crate answers verdicts only; the effect site performs the effect.

## Decisions

### D1 - Deny by default

`Policy::deny_all()` has no grants. Every check without a matching grant is a typed denial. There is
no global allow-all mode, because the boundary exists precisely so a subject can be constrained.

*Rejected:* an implicit allow for first-party code. The policy is used at call sites that perform an
effect on behalf of a subject, so a missing grant must be an error, not a shortcut.

### D2 - Scoped capabilities carry a glob pattern

`fs`, `net` and `shell` are scoped. `Grant::scoped(subject, capability, pattern)` compiles a
`globset` pattern once and rejects malformed patterns at grant time. `Policy::check_scope(subject,
capability, value)` matches the value against the compiled form; it never re-parses the pattern per
call.

A filesystem value must be rooted (absolute, or a leading separator) with no `.` or `..`
components. The match is lexical, so `/var/lib/notes/../secret` would otherwise match
`/var/lib/notes/**` and escape the grant. A shell value must have no `.`/`..` components either,
but may be a bare executable name; a full command string must be parsed and validated by the
caller. The policy does not resolve symlinks; the call site that performs the effect must
canonicalize and re-check when a symlink could leave the granted tree. `net` keeps its
caller-defined value shape.

*Rejected:* substring or prefix matching. It is too coarse for paths and hosts. *Rejected:* regular
expressions. Globs are easier to audit and sufficient for the current boundaries.

### D3 - Unscoped capabilities carry no pattern

`clipboard`, `notification` and `keychain` are not scoped. `Grant::new(subject, capability)` is
checked with [`Policy::check`], and `Policy::check_scope` on an unscoped capability falls back to the
same check. This keeps one call site shape without inventing fake scope values.

### D4 - The denial names what failed

`Denied` carries `subject`, `capability` and the `value` that failed to match, when the check was
scoped. This is enough for a plugin author to fix a manifest without reading framework source.

### D5 - No I/O in the crate

`auro-perms` performs no file, network or process I/O. It is a pure policy engine. The call site
that opens the file, connects to the host or spawns the process is the code that asks.

## Interfaces

```rust
pub enum Capability {
    Fs,
    Net,
    Shell,
    Clipboard,
    Notification,
    Keychain,
}

impl Capability {
    pub fn as_str(&self) -> &'static str;
    pub fn is_scoped(&self) -> bool;
}

pub struct Grant { /* subject, capability, pattern */ }

impl Grant {
    pub fn new(subject: impl Into<String>, capability: Capability) -> Result<Self, PolicyError>;
    pub fn scoped(
        subject: impl Into<String>,
        capability: Capability,
        pattern: impl Into<String>,
    ) -> Result<Self, PolicyError>;
    pub fn subject(&self) -> &str;
    pub fn capability(&self) -> Capability;
    pub fn pattern(&self) -> Option<&str>;
}

pub struct Policy { /* grants */ }

impl Policy {
    pub fn deny_all() -> Self;
    pub fn allow(self, grant: Grant) -> Self;
    pub fn allow_scope(
        self,
        subject: impl Into<String>,
        capability: Capability,
        pattern: impl Into<String>,
    ) -> Result<Self, PolicyError>;
    pub fn grants(&self) -> &[Grant];
    pub fn check(&self, subject: &str, capability: Capability) -> Result<(), Denied>;
    pub fn check_scope(
        &self,
        subject: &str,
        capability: Capability,
        value: &str,
    ) -> Result<(), Denied>;
}
```

Errors: `PolicyError::InvalidPattern`, `PolicyError::ScopeRequired`,
`PolicyError::ScopeNotSupported` and `UnknownCapability` for manifest parsing.

Usage:

```rust,ignore
let policy = auro::perms::Policy::deny_all()
    .allow(
        auro::perms::Grant::new("app", auro::perms::Capability::Clipboard)
            .expect("unscoped capability"),
    )
    .allow_scope("plugin:notes", auro::perms::Capability::Fs, "/var/lib/notes/**")?;

policy.check("app", auro::perms::Capability::Clipboard)?;
policy.check_scope("plugin:notes", auro::perms::Capability::Fs, "/var/lib/notes/a.txt")?;
```

## Acceptance

1. `Policy::deny_all()` denies every check (`denies_by_default`).
2. Unscoped capabilities are granted and denied independently of other subjects
   (`allows_an_unscoped_capability`, `scopes_are_subject_scoped`).
3. Scoped grants match their glob and reject out-of-scope values, including subjects outside the
   grant (`matches_a_scoped_grant`, `rejects_an_out_of_scope_value`).
4. Capability strings round-trip and an invalid pattern or scope mismatch is rejected
   (`capability_strings_round_trip`, `rejects_an_invalid_pattern`, `rejects_a_scope_mismatch`).
5. A filesystem value that is relative or contains `.`/`..`, and a shell value containing `.`/`..`,
   are denied before glob matching (`rejects_a_traversal_or_relative_filesystem_value`).
6. `auro::perms` re-exports the same API surface (`crates/auro/src/lib.rs`).
7. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Manifest file format and load path | Plugin host spec |
| Per-window grants for the webview escape hatch | Webview hatch spec |
| Plugin upgrade that widens scopes | Plugin host spec |
| Systemd-style units for scoped shell/notifications | Platform services spec |
