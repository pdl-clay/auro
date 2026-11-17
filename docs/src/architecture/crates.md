# Crates and layering

The Phase 0 workspace is in the tree. This chapter is the contract it satisfies and the map for
what comes next; it is deliberately more specific than the README.

## Crates

Six crates are materialized now; the rest arrive with the phase that needs them.

| Crate | Kind | Responsibility | Status |
|---|---|---|---|
| `auro` | library | facade re-exporting the supported API surface (`auro::prelude`) | Phase 0, in tree |
| `auro-cli` | binary | `new`, `dev`, `build`, `bundle`, `add`, `doctor` | Phase 0, in tree (`new`, `dev`, `build`, `doctor`) |
| `auro-core` | library | config schema, state store, runtime bridge; lifecycle, command registry and app-wide events arrive in Phase 1 | Phase 0, in tree (config, state) |
| `auro-ui` | library | GPUI re-export, tokens, minimal primitives; **no component toolkit** ([ADR 0009](../decisions/0009-toolkits-are-app-level.md)) | Phase 0, in tree (re-exports, `Button`, tokens) |
| `auro-ipc` | library | typed transport, schema derivation, event delivery | Phase 0, in tree (commands, runtime bridge) |
| `auro-macros` | proc-macro | `#[command]`, `generate_handler!` | Phase 0, in tree |
| `auro-perms` | library | capability manifests and runtime enforcement | planned, Phase 2 |
| `auro-plugins` | library | plugin manifest and host (in-process and out-of-process) | planned, Phase 3 |
| `auro-bundle` | library | platform packaging | planned, Phase 2 |
| `auro-update` | library | signed manifests, deltas, rollout, rollback | planned, Phase 2 |
| `auro-sign` | library | signing and key handling shared by bundle and update | planned, Phase 2 |

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
- Whether `auro-bundle`, `auro-update` and `auro-sign` are three crates or one crate with
  features. They are named separately in the roadmap and only `auro-cli` consumes them, so this
  can be settled when Phase 2 starts.
