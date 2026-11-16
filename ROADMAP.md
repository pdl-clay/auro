# Auro - Roadmap

> Tauri's app model. GPUI's renderer. Zero webview.

Living document. Phases ship when their exit criteria are met, not when the calendar says so.

Architecture detail lives in the [docs book](https://pdl-clay.github.io/auro/); decisions and
their trade-offs are recorded in [`docs/src/decisions/`](docs/src/decisions/README.md).

---

## 1. The thesis

Tauri got the *shell* right and the *engine* wrong.

- **Right:** the application model. Config, capabilities/permissions, plugin system, typed IPC,
  bundling (`.app`/DMG, MSI/NSIS, AppImage/deb/rpm), signed updater, dev server, single-instance,
  multi-window. Nobody wants to rebuild that.
- **Wrong (for people who want native):** the frontend is a browser. 80-150 MB of runtime,
  inconsistent rendering, second-hand accessibility, and the frontend is JavaScript.

GPUI is Zed's GPU UI toolkit: pure Rust, a Tailwind-flavoured builder API, 60-120 fps, no DOM, no
GC, no CSS engine. The entire Zed editor runs on it, so text input, IME, selection and font
rendering arrived solved, because it was a text editor first.

**Auro = Tauri's shell + GPUI as the renderer.** The frontend is Rust. The binary is the app.

**Initial audience:** Rust developers shipping dev tools, editors, dashboards, internal tools and
kiosk apps - people who want a sub-10 MB binary (the 1.0 target is under 6 MB) and
tens-of-milliseconds startup, and who already know Rust, so they do not need HTML.

---

## 2. What Auro is not

Positioning, not limitation:

| Not | Why |
|---|---|
| A webview wrapper | The opposite. The webview is a scoped escape hatch (phase 4), not a foundation. |
| A GUI toolkit | That is GPUI. Auro is an *application* framework. |
| A GPUI fork | Tracking upstream. A fork dies by merge conflict. |
| "CSS for Rust" | Hard rule: no styling language in 1.0. If you need CSS, use Tauri. |
| Electron with extra steps | Electron packs Chromium. Auro packs no browser at all. |

---

## 3. Architecture

```
+-----------------------------------------------------------+
| auro-cli   new . dev . build . bundle . add . doctor      |
+-----------------------------------------------------------+
| auro-core                                                 |
|   App . WindowManager . Config . Lifecycle .              |
|   Command registry . State store . Event bus              |
+--------------+-------------------+------------------------+
| auro-ui      | auro-ipc          | auro-perms             |
| GPUI + kit   | typed bus         | capabilities           |
+--------------+-------------------+------------------------+
| auro-plugins   in-process (cdylib) | out-of-process RPC   |
+-----------------------------------------------------------+
| auro-bundle . auro-update . auro-sign                     |
+-----------------------------------------------------------+
```

### Decision 1 - Threading model

GPUI owns the main thread (`Application::run`). All UI mutation happens on the main thread via
`cx.update` / `cx.spawn`. Long work goes to a `tokio` runtime on background threads.

Consequence: **`auro-ipc` is the boundary.** Commands run off the main thread and return typed
results; events travel main-ward by updating an `Entity`.

### Decision 2 - Actually typed IPC

Tauri's `invoke("cmd", { args })` is stringly-typed by default: a wrong argument is a runtime
error. Auro has no strings. The schema is derived from the Rust signature (`schemars`), and the
call site is a plain async function.

That is the largest DX advantage over Tauri, and it comes free by removing JavaScript from the
middle.

### Decision 3 - Capabilities for a Rust frontend

When the frontend is your own Rust code, permission checking is mostly *compile time*. What
remains as a genuine runtime boundary:

1. **Plugins** - third-party code. Scoped per plugin: fs, net, shell, clipboard, notification,
   keychain.
2. **Content** - documents opened, scripts evaluated, remote data.
3. **The webview escape hatch** (phase 4).

So `auro-perms` does not copy Tauri's model blindly. It guards plugin calls and sensitive service
invocations, which is where the risk actually is.

---

## 4. The hard problems

### Hot reload - existential

Rust compiles slowly. If the loop is a 20-second `cargo build`, nobody leaves the web. This is
*the* bet of the project.

- The UI compiles as a `cdylib`, loaded via `libloading`, hot-swapped on save.
- App state lives in the host process and is preserved across swaps via a `serde` snapshot.
- Function-level patching along the lines of Zed's `subsecond` crate.
- Target: **edit to see in < 500 ms.**

Fallback if that fails: process restart with state restoration plus incremental rebuild.

### Text, IME and accessibility

GPUI is strong here but not yet at parity: AccessKit, screen readers, Linux IME and complex
composition need an audit platform by platform.

### Native platform integration

File dialogs, tray, menu bar, deep links, notifications, single-instance, autostart, keychain,
drag-and-drop, print/PDF, spellcheck. Each one is a schedule line item, not a detail.

### Layout and styling

Web developers will arrive expecting a stylesheet. GPUI is `div().flex().gap_2().p_4()`. Plan: a
`style!` macro, design tokens, TOML themes, and a docs page titled "thinking without CSS".

### Windows and Linux parity

GPUI is most mature on macOS. Windows and Linux exist (Zed ships on both) but the support matrix
must be **published and honest from day one**. This is the biggest schedule risk.

---

## 5. Phases

### Phase 0 - Foundations (weeks 1-3)

- Repo, CI matrix (macOS / Windows / Linux), `rfcs/` folder, docs skeleton, license, site.
- Cargo workspace with the six Phase 0 crates, and the specs they implement (`docs/src/specs/`).
- `auro new`: generates `src/main.rs`, `Auro.toml`, `views/`, `services/`.
- **Vertical slice:** a window with a GPUI view, a button calling an async command, state
  updating. Get the *feel* right before anything else.

**Exit:** after `cargo install --path crates/auro-cli --locked`, `auro new demo && cd demo &&
auro dev` opens a window in under two minutes with a warm cargo cache. Publishing to crates.io is
not part of the phase.

**Progress (2026-11-15):** workspace, specs, CLI, typed command/state/config plumbing, scaffold,
relaunch-based `auro dev`, CI on macOS/Windows/Linux, and headless GPUI tests (render and button
click dispatching an async command with view update) are in place and green in CI/local gates. Spec
0001 acceptance is mostly met; interactive window-close-to-exit-0 verification and the < 500 ms
save-to-visible target remain open (current relaunch ~60 s, measured). The phase is still in
progress until the exit criterion and acceptance are fully closed.

### Phase 1 - v0.1 "Hello, native" (weeks 4-8)

- `auro-core`: config schema, multi-window, lifecycle, command registry with schema, state store.
- `auro-ui`: GPUI re-export plus ~20 components (Button, Input, List, Table, Tabs, Dialog, Toast,
  Menu, Tooltip, Sidebar, Command palette).
- Theming: tokens, light/dark, three built-in themes.
- `auro dev`: watch + rebuild + relaunch. Experimental hot reload.
- CLI: `new`, `dev`, `build`, `doctor`.

**Exit:** a real internal tool can be built with it (log viewer plus a settings window).

### Phase 2 - v0.3 "Ship it" (weeks 9-16)

- `auro-bundle`: `.app`/`.dmg` with codesign and notarization; MSI/NSIS with signtool;
  AppImage/deb/rpm.
- `auro-update`: signed manifests, deltas, staged rollout, rollback.
- Platform services: fs, dialogs, clipboard, notifications, keychain, tray, global shortcuts,
  deep links, single-instance, autostart.
- `auro-perms`: manifest plus runtime enforcement.
- Accessibility (AccessKit), IME and keyboard navigation pass.

**Exit:** three real apps shipped to real users. Startup/RAM/size benchmarks published against
Tauri and Electron.

### Phase 3 - v0.5 "Extensible" (weeks 17-24)

- `auro-plugins`: out-of-process RPC plugins (crash isolation, any language) plus in-process
  `dylib` plugins behind a feature flag. Manifest, versioning, permission scopes.
- Plugin index (static first, marketplace later).
- `auro-ui` grows: virtualized data table, charts, markdown/rich text, syntax highlighting
  (reusing Zed crates).
- Hot reload GA with state preservation.

**Exit:** a third-party plugin published by someone outside the core team.

### Phase 4 - v0.8 "Ecosystem" (weeks 25-34)

- **Embedded web view** via `wry`, explicitly scoped: OAuth, docs, payment iframes. Not a
  general-purpose frontend.
- Optional declarative UI (`.auro` to GPUI tree) for simple settings/about screens. Explicitly
  not a DOM.
- Template gallery, `auro add`, `auro migrate`.
- Docs: guided tour, cookbook, "coming from Tauri" guide.

**Exit:** 1.0 readiness review; API freeze candidates identified.

### Phase 5 - v1.0 (weeks 35-42)

- API freeze, semver from here on, LTS policy.
- Security policy, external audit, contributor guide, RFC process.
- **Performance budget in CI:** startup, first paint, RAM, binary size. Regressions fail the
  build.
- Launch: docs, three flagship example apps, benchmarks page, announcement.

---

## 6. Success metrics

| Metric | Target at 1.0 |
|---|---|
| Cold start | < 80 ms |
| Idle RAM | < 40 MB |
| Hello-world binary | < 6 MB (stripped) |
| Edit to see | < 500 ms |
| Platforms | macOS 12+, Windows 10+, Ubuntu 22.04+ (Wayland & X11) |

These numbers *are* the marketing. A screenshot of 4 MB of RAM next to Electron's 300 MB in a
typical app is the post that travels.

---

## 7. Risks

| Risk | Mitigation |
|---|---|
| GPUI upstream churn | Pin versions; vendor patches only if critical; thin compat layer |
| Immature Windows/Linux support | Publish the support matrix from phase 0 |
| Hot reload misses the target | Fallback to state-preserving restart |
| Scope drift into "CSS for Rust" | Hard rule in the README and in the RFC process |
| Name squatting | Register org, crates, domain and handles before announcing |

---

## 8. Open questions

1. **Scripting:** Rust-only frontend, or a layer (Rhai/Lua) for plugin UIs?
2. **Plugin isolation:** out-of-process first (safer, slower) or in-process first (faster)?
3. **Opinionated or not:** ship our own component kit, or let crates like `gpui-component` fill
   in?
4. **Brand:** name, logo, domain, org.

Resolved: **license** — dual MIT/Apache-2.0 (see [ADR 0006](docs/src/decisions/0006-dual-license.md)
and `LICENSE-MIT`, `LICENSE-APACHE`).
