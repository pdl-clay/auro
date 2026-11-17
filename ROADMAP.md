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

Rust compiles slowly. If the loop stays at a multi-second/minute rebuild-and-relaunch, nobody leaves
the web. This is *the* bet of the project.

- **Target:** edit to see in **< 500 ms** for a warm view-only change, with opted-in state
  preserved (a target until measured).
- **Baseline (Phase 0):** relaunch loop, ~60 s save-to-visible on the development machine
  (relink-dominated). Not hot reload.
- **Slice 0 finding (2026-11-16):** a viability spike on GPUI 0.2.2/Linux rejected
  `cdylib`/`libloading` swap of GPUI view code as infeasible for the current stack and measured a
  realistic GPUI-view `cdylib` warm relink at ~171 s (debug). The mechanism is under revision toward
  a same-process approach.
- **Option 1 consolidation (2026-11-16, prototype evidence, not integrated):** on Linux x86_64/
  Wayland with `gpui = "=0.2.2"`, `subsecond 0.7.10`/`dioxus-devtools 0.7.10`/`dx 0.7.10`,
  in-process tip-crate function-level patching via explicit `HotFn` boundaries and host-owned GPUI
  application/window/entities/state showed warm view changes in ~271-450 ms save-to-first-new-render
  in minimal scenarios (s1 median 371.5 ms, N=10; s6 median 346 ms, N=20, 20/20 stable) with host
  state preserved. Documented constraints: tip-crate-only boundary, outside-boundary/init-captured
  work not retroactive, tip-crate statics reset on patch, `thread_local!` in a patched path can
  fail patch generation while the app stays on previous code, and tooling/layout/snapshot
  integration plus representative Auro-scaffold D6 measurement remain pending. A Windows 11 x86_64
  plain (non-GPUI) prototype follow-up in a WinBoat guest (same dx/subsecond/dioxus-devtools 0.7.10
  stack, rustc/cargo 1.99.0 MSVC, clean target, external logs,
  `dx serve --hot-patch --interactive false --platform desktop`) completed build in 30.66 s and
  applied 3/3 warm patches in 484/504/499 ms with monotonic counter continuity
  `rev0 1..361 -> rev1 421..781 -> rev2 841..1261 -> rev3 1321..1741` in the same process
  (`plain-proofwin-20261116152539.*`), showing mechanism viability on Windows for this minimal plain
  case only. A second plain Windows prototype (same guest, 2026-11-16) applied in-boundary view
  edits in 590 ms and 523 ms (`ui0`→`ui1`→`ui2` with added radius/padding/icon) while
  `session=s5500` and the host/backend counters stayed continuous; an init-captured backend
  interval change (500→1500 ms) was hot-patched but not retroactive (`active_interval_ms=500`), and
  only took effect after a restart (`session=s9516`, `init_backend_ms=1500`, 1500 ms cadence) —
  `adv-fix-20261116161242.*`. GPUI 0.2.2 on the same guest built and rendered in the interactive
  console session (206 renders, host state preserved), while `dx serve` launched from the
  SSH/session-0 context failed with DirectX `0x887A0022` before render. A `dx serve` run inside the
  interactive session then built (473.85 s), rendered, applied its first patch (1453 ms) and
  crashed with a main-thread stack overflow (`exit code: 0xc000041d`). Isolation showed the cast is
  not stack- or address-resolution-related (16 MiB stack reproduces it; jump table resolves), that a
  leaf `HotFn` patches twice without crashing, and that a `ViewData`-returning boundary hot-reloads
  UI values with state preserved (1324/1084 ms, 2/2, screenshot). So on Windows: framework-free hot
  boundaries work, element-returning view code does not. `auro dev` remains relaunch by default.
- **Experimental hot mode (2026-11-16, opt-in):** `[dev] hot = true` drives `dx serve --hot-patch`
  with a data-only hot boundary in the scaffold; on niri/Wayland one patch (3198 ms) reloaded the
  boundary in the same window, while on Windows 11 the patch applied (4679 ms) but the patched
  boundary was not observed running. `auro dev` still relaunches by default.
- **Fallback:** state-preserving process restart, labelled honestly and never presented as hot
  reload.

Contract, evidence, and staged acceptance:
[spec 0004](docs/src/specs/0004-hot-reload.md) (draft),
[ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md).

### Text, IME and accessibility

GPUI is strong here but not yet at parity: AccessKit, screen readers, Linux IME and complex
composition need an audit platform by platform.

### Native platform integration

File dialogs, tray, menu bar, deep links, notifications, single-instance, autostart, keychain,
drag-and-drop, print/PDF, spellcheck. Each one is a schedule line item, not a detail.

### Layout and styling

Web developers will arrive expecting a stylesheet. GPUI is `div().flex().gap_2().p_4()`. Plan:
Auro tokens for its own primitives, and a docs page titled "thinking without CSS". There is no
`style!` macro and no Auro-owned theme system: the app's toolkit supplies styling and themes
([ADR 0009](docs/src/decisions/0009-toolkits-are-app-level.md)).

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

**Progress (2026-11-15, updated 2026-11-17):** workspace, specs, CLI, typed command/state/config
plumbing, scaffold, relaunch-based `auro dev`, CI on macOS/Windows/Linux, and headless GPUI tests
(render and button click dispatching an async command with view update) are in place and green in
CI/local gates. The app quits when the last window closes (verified locally on niri/Wayland: demo
exits 0) and `auro dev` now detects child exit. The < 500 ms save-to-visible target remains unmet
in product (`auro dev` relaunch ~60 s measured), but Option 1 consolidation (2026-11-16) recorded
prototype evidence for a viable same-process candidate (`subsecond`/`dx` tip-crate `HotFn`
patching, Linux x86_64/GPUI 0.2.2, minimal cases ~271-450 ms, host state preserved) with
constraints and pending integration/representative D6 measurement (see
[spec 0004](docs/src/specs/0004-hot-reload.md), [ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md)).

**Phase 0 closed (2026-11-17):** spec 0001 acceptance 1-8 and spec 0002 acceptance are met; the
closing audit fixed `auro build`'s manifest parse (it rejected every app), wired the version check
into `auro dev`, added the executable-directory config fallback, validated `[bundle] targets`
against the host, made `auro doctor` report the resolved GPUI pin, and added the config round-trip
and error-conversion tests the specs required. App-wide events and lifecycle moved to Phase 1;
scaffold icons moved to Phase 2 with the bundler.

**Exit criterion measured (2026-11-16, niri/Wayland, warm cache):**
`cargo install --path crates/auro-cli --locked` took 13.3 s; `auro new /home/pdl/auro-demo` then
`auro dev` opened the window in **2 s**; closing the window through `auro dev` exited **0**. A
broken-then-fixed edit kept the previous window and app alive during the failure and produced a new
app process 3 s after the fix (spec 0004 D4, implemented the same day). The dev loop now emits a JSON
trace per save: median **2380 ms** save→build and **2730 ms** save→repaint-requested (three saves,
debug, warm cache, 50 ms resolution; spec 0004 D6). Relaunches are labelled from the app's snapshot
report (`restarted (state restored)` / `restarted (state reset: <reason>)`), verified on Linux and on
Windows 11, where the dev loop now runs the app from a copy under `target/auro-dev/` because Windows
locks a running image; the Windows traces were 10.8 s and 8.2 s save→build (debug, warm cache).
Automated process-level window-close coverage remains test debt, not an acceptance gap: acceptance
3 is a manual runnable and was verified as above. The `(unset)` window title was diagnosed and fixed
on 2026-11-16: GPUI 0.2.2 applies `WindowOptions::titlebar.title` on X11, Windows and macOS but not
on Wayland, so `App` now sets the title on the opened window; niri reports `auro-demo`, and a
headless test asserts the title reaches the window.

### Phase 1 - v0.1 "Hello, native" (weeks 4-8)

- `auro-core`: multi-window lifecycle, app-wide events, command registry with schema, state store
  (the config schema landed in Phase 0; lifecycle events landed in
  [spec 0006](docs/src/specs/0006-lifecycle-events.md), typed app events in
  [spec 0007](docs/src/specs/0007-events.md), command schemas in
  [spec 0008](docs/src/specs/0008-command-schema.md), and the window manager in
  [spec 0009](docs/src/specs/0009-window-manager.md)).
- `auro-ui`: GPUI re-export, tokens, and the minimal primitives the scaffold needs; **no component
  toolkit** ([ADR 0009](docs/src/decisions/0009-toolkits-are-app-level.md)).
- Toolkit seam: `App::setup` and `App::root_with` let the app initialize and wrap the toolkit it
  chose ([spec 0005](docs/src/specs/0005-toolkit-seam.md)); the book names `gpui-component 0.5.x`
  as the tested integration while GPUI is pinned.
- Theming: Auro tokens for its primitives; themes belong to the app's toolkit (the `theme` config
  is passed to `App::setup`).
- `auro dev`: watch + rebuild + relaunch. Experimental hot reload (mechanism under [spec 0004](docs/src/specs/0004-hot-reload.md) / [ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md)).
- CLI: `new`, `dev`, `build`, `doctor`.

**Exit:** a real internal tool can be built with it (log viewer plus a settings window) using the
toolkit the app chooses.

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
- `auro-ui` stays toolkit-free; optional integration crates (for example
  `auro-gpui-component`) wrap advanced widgets such as a virtualized data table, charts, markdown
  and syntax highlighting.
- Hot reload GA with state preservation (mechanism/measurability per [spec 0004](docs/src/specs/0004-hot-reload.md) / [ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md)).

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
3. **Brand:** name, logo, domain, org.

Resolved: **license** — dual MIT/Apache-2.0 (see [ADR 0006](docs/src/decisions/0006-dual-license.md)
and `LICENSE-MIT`, `LICENSE-APACHE`). **Component kit** — Auro ships no toolkit; the app chooses
one ([ADR 0009](docs/src/decisions/0009-toolkits-are-app-level.md),
[spec 0005](docs/src/specs/0005-toolkit-seam.md)).
