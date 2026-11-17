# 0009 - Toolkits are the application's choice

**Status:** Accepted (2026-11-17)

## Context

Phase 1 planned an `auro-ui` component kit of about twenty widgets. Before implementing it, we
evaluated adopting [`gpui-component`](https://crates.io/crates/gpui-component) instead, because it
already ships 60+ widgets and would remove a large chunk of work.

The decision has to hold three constraints:

1. **Auro is the application layer, not a GUI toolkit.** The non-goal table says widgets are GPUI's
   and Auro is the application framework. A component kit is a toolkit, whoever writes it.
2. **The GPUI line is pinned on purpose.** `gpui = "=0.2.2"` is the line that carries the
   hot-reload evidence (`subsecond`/`dx 0.7.10`, function-level patching, state preservation).
   Moving the renderer line is a separate, expensive decision.
3. **Size and startup are part of the pitch.** The 1.0 target is a sub-6 MB hello world and
   sub-80 ms cold start.

A throwaway spike on 2026-11-17 (Linux x86_64, Wayland/niri, rustc 1.99.1, release, stripped,
`panic = "abort"`, one window with a label and one button) measured:

| Probe | Stripped binary | spawn → window requested (median, N=5) |
|---|---:|---:|
| Auro with its own `Button` | **15.5 MB** | 139.8 ms |
| GPUI + `gpui-component 0.5.0` (`Root` + `Button`) | **22.0 MB** | 147.3 ms |

The delta is **+6.5 MB (+42%)**; startup is effectively unchanged. The component probe compiled
and opened a window against the pinned `gpui 0.2.2`. It added 46 packages, including non-optional
`tree-sitter`, `html5ever`, `ropey`, `markdown`, `lsp-types` and `rust-i18n`, and 58k lines of
source. It is not only a widget crate: it requires `gpui_component::init(cx)` and wrapping every
window root in `Root::new(view, window, cx)`, and it owns a global theme, dialog/notification state
and input focus.

Version compatibility matters as much as size:

| `gpui-component` | GPUI line | Consequence |
|---|---|---|
| 0.5.1 (2026-02-05) | `gpui ^0.2.2` | last line that compiles against Auro's pinned renderer |
| 0.6.0 and later | `gpui-pre ^0.3.1` | requires a GPUI line migration; current 0.10.0 pins `gpui-pre =0.3.12` |

Adopting the current component line therefore means moving the GPUI pin and revalidating the
hot-reload bet. Adopting the compatible line means freezing nine-month-old widgets with no upstream
fixes on our pin.

Both measured binaries are far above the 6 MB target, so size work is needed regardless; adding
6.5 MB makes that target harder, not easier. Writing the kit ourselves is not free either: the
useful subset for Phase 1 (`Input`, `List`, `Table`, `Tabs`, dialogs, menus) is an estimated
5-8k lines, and input/table are the hard parts.

## Decision

Auro ships **no component toolkit**. Toolkit choice belongs to the application.

- `auro-ui` stays the renderer seam: GPUI re-exports, Auro tokens, and the minimal primitives the
  scaffold and tests need. It does not gain a component kit in 1.0.
- An app adds the toolkit it wants as an application dependency, initializes it in
  [`App::setup`](../specs/0005-toolkit-seam.md), and wraps its root in the toolkit's shell with
  [`App::root_with`](../specs/0005-toolkit-seam.md) when the toolkit needs one.
- `App::setup` receives the parsed `Config`, so the app can apply `app.windows[].theme` to the
  toolkit it chose. Auro does not own a theme system.
- Auro documents one tested integration — `gpui-component 0.5.x` while GPUI 0.2.2 is pinned — and
  says plainly that the current component line requires a GPUI migration. No toolkit is a
  framework dependency.
- The Phase 1 proof (log viewer plus settings) uses an app-level toolkit; it does not become part
  of the framework.

*Alternative rejected:* own the twenty widgets. It preserves size and API ownership but spends the
phase on a toolkit Auro says it is not, and duplicates a mature library.

*Alternative rejected:* adopt `gpui-component` into `auro-ui`. It wins schedule but costs 6.5 MB on
every app, freezes a stale line or forces a GPUI migration that invalidates the hot-reload
evidence, and makes a pre-1.0 third-party API Auro's public component API.

## Consequences

**Easier**

- No component kit in the critical path; Phase 1 focuses on the app model, typed IPC, lifecycle,
  events, and the dev loop.
- The GPUI pin and hot-reload research stay untouched.
- Apps choose the toolkit that fits them, and Auro does not have to compete with upstream widget
  work.
- Auro's hello-world binary does not pay for widgets it does not use.

**Harder**

- Out-of-the-box UI requires a toolkit decision; the scaffold stays deliberately minimal.
- Docs and examples must name exact toolkit versions, because compatibility tracks the GPUI line.
- The root/setup seam is now a public contract and cannot be removed casually.
- Dialogs, notifications, focus rings, per-window themes and accessibility become the app's or the
  toolkit's responsibility, not Auro's.

**Forbidden**

- Adding a toolkit to `auro-core` or `auro-ui` as a framework dependency.
- Claiming toolkit compatibility without a pinned, tested version and a note about the GPUI line.
- Presenting Auro's minimal `Button` as a component kit.

## Revisit when

- Auro plans its next GPUI line migration (after the hot-reload mechanism is settled), or
- a toolkit reaches a stable 1.0 line that can be pinned without moving GPUI, or
- the Phase 1 proof shows the seam is insufficient for a real application.

An optional integration crate (for example `auro-gpui-component`) can wrap the current toolkit
line without changing this decision: the framework core still ships no toolkit.
