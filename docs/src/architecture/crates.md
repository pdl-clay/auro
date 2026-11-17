# Crates and layering

The Phase 0 workspace is in the tree. This chapter is the contract it satisfies and the map for
what comes next; it is deliberately more specific than the README.

## Crates

Eleven crates are materialized now; the rest arrive with the phase that needs them.

| Crate | Kind | Responsibility | Status |
|---|---|---|---|
| `auro` | library | facade re-exporting the supported API surface (`auro::prelude`) | Phase 0, in tree |
| `auro-cli` | binary | `new`, `dev`, `build`, `bundle`, `generate-key`, `manifest`, `doctor` | Phase 0-2, in tree |
| `auro-core` | library | config schema, state store, lifecycle events, single-instance guard; window manager and event delivery live in the facade | Phase 1, in tree (config, state, lifecycle, instance) |
| `auro-ui` | library | GPUI re-export, tokens, minimal primitives; **no component toolkit** ([ADR 0009](../decisions/0009-toolkits-are-app-level.md)) | Phase 0, in tree (re-exports, `Button`, tokens) |
| `auro-ipc` | library | typed transport, command schemas, runtime bridge | Phase 1, in tree |
| `auro-macros` | proc-macro | `#[command]`, `generate_handler!` | Phase 0, in tree |
| `auro-perms` | library | capability manifests and runtime enforcement (spec 0016) | Phase 2, in tree |
| `auro-plugins` | library | plugin manifest and host (in-process and out-of-process) | planned, Phase 3 |
| `auro-services` | library | platform services: autostart (spec 0017) | Phase 2, in tree |
| `auro-bundle` | library | platform packaging via `tauri-bundler` (spec 0010) | Phase 2, in tree |
| `auro-sign` | library | Ed25519 keys, signatures, SHA-256 (spec 0011) | Phase 2, in tree |
| `auro-update` | library | signed manifests, check/download/stage runtime, Linux AppImage apply (specs 0012-0014) | Phase 2, in tree |

## Dependency rules

1. **One direction.** A crate may depend on the layer below it. Cycles are a design error, not a
   `Cargo.toml` problem.
2. **`auro-core` composes; it is not a dumping ground.** Anything that only one platform or one
   subcommand needs belongs in that crate.
3. **`auro-ui` has no policy.** No permission checks, no filesystem access, no config reads.
   A primitive that needs data takes it as an argument.
4. **`auro-perms` has no I/O.** It answers "is this capability granted for this subject?" and
   returns a verdict. Enforcement points live at the call sites that perform the effect.
5. **GPUI is not re-exported wholesale.** `auro-ui` re-exports only the GPUI surface Auro's
   primitives use, so upstream churn is absorbed in one place.
6. **No toolkit in the framework.** `gpui-component` and similar crates are application
   dependencies, initialized through `App::setup` and wrapped with `App::root_with`
   ([ADR 0009](../decisions/0009-toolkits-are-app-level.md)).

## Rules for tracking GPUI upstream

- Pin GPUI by exact version. Floating a git dependency is how the compat layer becomes a fork.
- Vendor a patch only when the upstream bug blocks a release; keep the patch in one module with a
  link to the upstream issue.
- Nothing outside `auro-ui` imports `gpui` directly. That includes examples unless they are
  explicitly demonstrating the escape hatch.

## Naming

- Crates: `auro-*` in the manifest, `auro_*` in code, `auro` as the user-facing facade.
- Public API is added to `auro::prelude` only when it is stable enough to promise.
- Internal modules may be reorganised freely before 1.0; public items may not move without a
  changelog entry.

## Open questions

- Whether `auro` is a separate facade crate or `auro-core` re-exported under a shorter name.
- Whether `auro-update` and `auro-sign` are two crates or one crate with features. They are named
  separately in the roadmap, and their consumers are narrow (`auro-cli` and `auro-update`), so this
  can be settled when the updater API stabilizes.
