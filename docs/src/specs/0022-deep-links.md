# 0022 - Deep links

**Status:** Implemented (2026-11-18; initial and forwarded arguments are wired; macOS bundle registration is packager-owned)

Phase 2 needs apps to accept URL-scheme launches. This spec fixes the deep-link service contract:
scheme validation, registration, parsing and a queue that waits for the app. The `auro::services`
re-export requires the facade's `services` feature.

## Scope

- `auro-services::register_scheme`: register a scheme for the current user.
- `auro-services::AppLinks`: parse accepted schemes and queue links until drained.
- `DeepLink`: scheme, host, decoded segments, decoded query and raw argument.
- `auro::services::AppLinks` and `register_scheme` re-exports.
- `App::deep_links(links)`: queues initial process arguments and forwarded `InstanceMessage`
  arguments into a shared queue.

## Out of scope

- macOS `CFBundleURLTypes` editing; the packager owns the bundle's `Info.plist`.
- Linux `.desktop` installation by the packager; the service writes a user entry and calls
  `xdg-mime`.
- Authentication, consent prompts and unknown-origin policy.
- Universal links / HTTPS associations.

## Decisions

### D1 - A closed scheme list

`AppLinks::new` takes the accepted schemes. `parse` rejects any other scheme, control characters,
links over 4096 bytes, empty hosts, `.`/`..` path segments, and hosts that are not a DNS-style label
sequence or an IPv6 literal. A bracketed host must parse as `std::net::Ipv6Addr`; `[..]`, `[C:]` and
other hex-looking strings are rejected. Path segments reject `/`, `\`, `.`, `..` and `:` (a drive
prefix or an NTFS alternate-data separator), and any segment that is a Windows prefix, root or parent
component. A segment ending in `.` or space, whose trailing-dot/space trim is empty or `.`/`..`, or
whose stem is a reserved Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`,
`LPT1`-`LPT9`) is also rejected: Windows trims those characters when it resolves a component, so
`.. ` would otherwise arrive as `..` (#95, #101). The app sees only links it asked for.
`register_scheme` validates the RFC 3986 grammar: the first character is ASCII alphabetic and the
rest are ASCII alphanumerics or `+`, `-`, `.`. A leading digit and `_` are rejected; `+` is accepted.

### D2 - Registration is per user

`register_scheme` always acts in the current user's namespace:

| Platform | Mechanism |
|---|---|
| Windows | `HKCU\Software\Classes\<scheme>` via `reg.exe` |
| Linux | XDG desktop entry plus `xdg-mime default` |
| macOS | packager's `CFBundleURLTypes`; the call is a no-op |

The Linux entry's `Exec` value quotes the executable and escapes it for the Desktop Entry spec:
`%` -> `%%` (field codes expand even inside quotes), `"`/`\`/`` ` ``/`$` are escaped, and control
characters (including `\n`) are rejected so a path cannot inject another key. Registration writes
through a sibling temporary file and rename, so a symlink at the desktop path is rejected rather
than followed. If `xdg-mime` is missing or fails, a first registration removes the file it wrote and
a re-registration restores the previous handler instead of deleting it.

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

impl App {
    #[cfg(feature = "services")]
    pub fn deep_links(self, links: AppLinks) -> Self;
}
```

Usage:

```rust,ignore
let links = auro::services::AppLinks::new(vec!["myapp".to_owned()]);
App::new()
    .deep_links(links.clone())
    .single_instance()
    .root::<View>("main")
    .run();
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
4. Cloned `AppLinks` handles share one FIFO (`clones_share_the_same_queue`); `App::deep_links`
   stores that shared handle behind the `services` feature and the run path feeds initial and
   forwarded `InstanceMessage` arguments.
5. Bracketed hosts must be IPv6 literals and drive-prefixed segments are rejected
   (`rejects_non_ipv6_bracketed_hosts`, `accepts_ipv6_literal_hosts`,
   `rejects_drive_prefixed_and_encoded_separator_segments`, `accepts_ordinary_path_segments`).
6. Segments whose trailing dot/space would normalize to `.`/`..`, and reserved Windows device
   names, are rejected while an internal dot stays valid
   (`rejects_segments_that_windows_normalizes_to_traversal`,
   `rejects_windows_reserved_segments`, `accepts_segments_with_internal_dots`).
7. The Linux `applications` directory is created `0755` under `umask 002`, a group-writable or
   symlinked directory is refused, and the managed data base itself is validated
   (`applications_directory_is_created_private_under_umask`,
   `registration_refuses_a_group_writable_applications_directory`,
   `registration_refuses_a_symlinked_applications_directory`,
   `registration_refuses_a_group_writable_data_base`).
8. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
9. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Universal links / HTTPS associations | Deep-link follow-up |
| macOS bundle registration automation | Packager follow-up |
| Unknown-origin consent policy | Deep-link UX spec |
