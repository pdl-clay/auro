# 0002 - Configuration

**Status:** Approved - implementation in progress

`Auro.toml` is the only file an app author edits to describe their app. This spec fixes its
location, format, every Phase 0 key, the merge rules, and what makes it invalid.

## Decisions

### D1 - TOML, kebab-case, next to the crate manifest

- File: `Auro.toml` at the app crate root, beside `Cargo.toml`.
- Keys: `kebab-case` (TOML idiom). Section and field *names* mirror Tauri's `tauri.conf.json`, so a
  Tauri user recognises the shape; the case convention is the only translation. Mapping is
  mechanical and lives in `auro-bundle` when it lands.
- Unknown keys are **errors**, not warnings. A typo in a config file must not silently do nothing.

*Rejected:* JSON. It is the format Tauri uses, but it costs comments and trailing commas for no
benefit in a file humans maintain by hand. *Rejected:* TOML with camelCase keys. It satisfies
neither idiom.

### D2 - Load order and overrides

1. `AURO_CONFIG` (a path) if set, otherwise `Auro.toml` in the app directory (current directory,
   or the one `--dir` names), falling back to the executable's directory.
2. `Auro.<platform>.toml` where platform is `macos`, `windows` or `linux`, merged key by key.
3. `AURO_CONFIG_OVERRIDES`, a JSON Merge Patch set by `auro dev` and `auro build` from `--config
   key=value` flags, merged last. This environment variable is the only CLI-to-app channel.

Merge is recursive for tables and **replacing** for arrays: a platform file that declares
`app.windows` replaces the list; it does not append to it. Merging arrays positionally is how
config systems become unreadable. The rule is JSON Merge Patch (RFC 7396) minus the `null`-deletes
case, which TOML cannot express.

In Phase 0 the file is read at runtime. Bundled apps embed it in Phase 2.

### D3 - Windows are a list

`app.windows` is an array of tables; the first one is the default unless `app.default-window`
names another. Labels are unique and stable: they are what a capability targets later.

## Schema

Required: `app.identifier`, `app.product-name`, `app.version`, and at least one window.

Top level also accepts `"$schema"` (string): an editor hint pointing at the published JSON Schema.
It is never read at runtime and never affects behaviour.

### `[app]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `identifier` | string | required | reverse-DNS, `[A-Za-z0-9.-]`, no leading/trailing dot; becomes the bundle id |
| `product-name` | string | required | window-title default and bundle display name |
| `version` | string | required | SemVer; must match `Cargo.toml`'s `package.version` or the build fails |
| `description` | string | `""` | one line, used by the bundle metadata |
| `authors` | array of string | `[]` | `"Name <email>"` |
| `homepage` | string | `""` | URL |
| `license` | string | `""` | SPDX expression; must be compatible with `MIT OR Apache-2.0` |
| `default-window` | string | first window's label | must match a declared label |

### `[[app.windows]]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `label` | string | required | unique, `[A-Za-z0-9_-]` |
| `title` | string | `app.product-name` | |
| `width` | integer | `960` | logical pixels |
| `height` | integer | `640` | logical pixels |
| `min-width` | integer | none | |
| `min-height` | integer | none | |
| `max-width` | integer | none | |
| `max-height` | integer | none | |
| `resizable` | bool | `true` | |
| `decorations` | bool | `true` | |
| `transparent` | bool | `false` | |
| `always-on-top` | bool | `false` | |
| `visible` | bool | `true` | |
| `center` | bool | `true` | |
| `fullscreen` | bool | `false` | |
| `theme` | `"light"` \| `"dark"` \| `"system"` | `"system"` | |

### `[dev]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `watch` | array of string | `["src", "Auro.toml"]` | paths relative to the app dir |
| `run-args` | array of string | `[]` | passed to the spawned binary |
| `build-args` | array of string | `[]` | passed to `cargo build` |

### `[bundle]`

Validated in Phase 0, consumed from Phase 2. Nothing here changes `auro build` yet.

| Key | Type | Default | Notes |
|---|---|---|---|
| `active` | bool | `true` | |
| `targets` | array of string \| `"all"` | `"all"` | `app`, `dmg`, `msi`, `nsis`, `appimage`, `deb`, `rpm`; unsupported per host is an error, not a skip |
| `icon` | array of string | `[]` | first existing format wins per platform |
| `resources` | array of string | `[]` | glob patterns, relative to the app dir |
| `category` | string | `"DeveloperTool"` | Apple/XDG category name |
| `short-description` | string | `""` | installer metadata |
| `long-description` | string | `""` | |
| `copyright` | string | `""` | |
| `license-file` | string | `""` | shipped in the bundle when set |

## Validation

Failures are `AuroError::Config { path, key, message }` and abort before a window opens. Rules:

1. Unknown key: reported with the key path and the closest known key, if any.
2. Wrong type or missing required key: reported with the key path.
3. `app.version` differing from `Cargo.toml`: reported by `auro build` and `auro dev`, naming both
   values. The running app cannot see the manifest, so it does not perform this check.
4. Duplicate window labels, or `default-window` naming nothing: error.
5. `bundle.targets` naming a package format the host cannot produce: error listing the valid set
   for that host.
6. Relative `watch` or `resources` paths escaping the app directory: error.

## Interfaces

```toml
# Auro.toml - the smallest valid file
[app]
identifier = "com.example.demo"
product-name = "Demo"
version = "0.1.0"

[[app.windows]]
label = "main"
title = "Demo"
width = 960
height = 640
```

```rust,ignore
pub struct Config { pub app: AppConfig, pub dev: DevConfig, pub bundle: BundleConfig }
impl Config {
    /// Reads `Auro.toml`, merges the platform file and overrides, validates, returns.
    pub fn load(dir: &Path, overrides: &[String]) -> Result<Config, AuroError>;
}
```

## Acceptance

- The smallest valid file parses; every default in the tables above is the value the app uses where
  the platform can express it. `auro doctor` lists the keys the current platform cannot honor.
- A missing `Auro.toml` fails with exit code 2 and a message naming the paths searched.
- An unknown key under `[app]` fails with a message naming it.
- `Auro.linux.toml` overriding `app.windows` replaces the list; overriding `app.product-name`
  changes only that key.
- A `version` mismatch against `Cargo.toml` fails `auro build` and `auro dev`.
- Round-trip test: every documented key is set to a non-default value, serialized back, and equals
  the original.
