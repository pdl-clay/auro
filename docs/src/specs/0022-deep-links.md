# 0022 - Deep links

**Status:** Implemented (2026-11-17; macOS bundle registration is packager-owned)

Phase 2 needs apps to accept URL-scheme launches. This spec fixes the deep-link service contract:
scheme validation, registration, parsing and a queue that waits for the app.

## Scope

- `auro-services::register_scheme`: register a scheme for the current user.
- `auro-services::AppLinks`: parse accepted schemes and queue links until drained.
- `DeepLink`: scheme, host, decoded segments, decoded query and raw argument.
- `auro::services::AppLinks` and `register_scheme` re-exports.

## Out of scope

- macOS `CFBundleURLTypes` editing; the packager owns the bundle's `Info.plist`.
- Linux `.desktop` installation by the packager; the service writes a user entry and calls
  `xdg-mime`.
- Universal links / HTTPS associations.
- Authentication, consent prompts and unknown-origin policy.
- Forwarding to an already-running instance; single-instance forwarding is a separate slice.

## Decisions

### D1 - A closed scheme list

`AppLinks::new` takes the accepted schemes. `parse` rejects any other scheme, control characters,
links over 4096 bytes, empty hosts and `.`/`..` path segments. The app sees only links it asked for.

### D2 - Registration is per user

`register_scheme` always acts in the current user's namespace:

| Platform | Mechanism |
|---|---|
| Windows | `HKCU\Software\Classes\<scheme>` via `reg.exe` |
| Linux | XDG desktop entry plus `xdg-mime default` |
| macOS | packager's `CFBundleURLTypes`; the call is a no-op |

*Rejected:* writing machine-wide registry keys or system desktop files. They need elevation and are
not appropriate for an app-level link.

### D3 - Queue until the app asks

Links arrive from launch arguments and may arrive before the UI is ready. `AppLinks` stores parsed
links in a mutex-protected FIFO and returns them in arrival order on `drain`. Callers can ignore
duplicates themselves; the queue does not deduplicate.

### D4 - Parsing is defensive

Percent decoding is UTF-8-aware. Control characters are rejected after decoding. Path traversal is
rejected before the app sees a segment. Invalid links are dropped, not turned into partial links.

*Rejected:* handing raw strings to every app. Every consumer would re-implement the same checks.

## Interfaces

```rust
pub enum DeepLinkError { Register(String), InvalidScheme(String) }

pub struct DeepLink {
    pub scheme: String,
    pub host: String,
    pub segments: Vec<String>,
    pub query: BTreeMap<String, String>,
    pub raw: String,
}

pub fn register_scheme(
    scheme: &str,
    executable: &std::path::Path,
) -> Result<(), DeepLinkError>;

pub struct AppLinks { /* queue, schemes */ }

impl AppLinks {
    pub fn new(schemes: Vec<String>) -> Self;
    pub fn push(&self, raw: &str);
    pub fn push_raw(&self, raw: &[String]);
    pub fn drain(&self) -> Vec<DeepLink>;
    pub fn schemes(&self) -> &[String];
}
```

Usage:

```rust,ignore
let links = auro::services::AppLinks::new(vec!["myapp".to_owned()]);
links.push_raw(&std::env::args().collect::<Vec<_>>());
for link in links.drain() {
    // route `link.host`, `link.segments`, `link.query`
}
```

## Acceptance

1. A valid `scheme://host/path?key=value` parses into scheme, host, segments and query
   (`parses_a_deep_link`).
2. Unknown schemes and traversal paths are dropped (`rejects_an_unknown_scheme`,
   `rejects_a_traversal_path`).
3. Empty or malformed schemes are rejected before registration
   (`rejects_an_empty_or_bad_scheme`).
4. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
5. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Forwarding links to a running instance | Single-instance IPC spec |
| Universal links / HTTPS associations | Deep-link follow-up |
| macOS bundle registration automation | Packager follow-up |
| Unknown-origin consent policy | Deep-link UX spec |
