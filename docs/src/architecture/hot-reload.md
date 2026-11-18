# Hot reload

This is the existential bet of the project. If the edit loop stays at a multi-second/minute
rebuild-and-relaunch, nobody leaves the web, and the rest of Auro does not matter.

> Target: edit to see in **under 500 ms**, with app state preserved.

The target and observable contract are fixed; the mechanism is under revision with prototype
evidence. Normative dev-loop contract, staging, and acceptance are drafted in
[spec 0004](../specs/0004-hot-reload.md).

## Contract

- **Default today:** `auro dev` rebuilds and relaunches the app process (Phase 0). A saved change to
  a visible relaunch measured ~60 s on the development machine (relink dominates, 2026-11-15). This
  is not hot reload.
- **Hot reload goal:** a warm, view-only change becomes visible in under 500 ms with opted-in state
  preserved, without closing the window.
- **Honesty rule:** a process restart is never presented as hot reload. A failed build keeps the
  previous UI active. A reload or patch that cannot be applied safely falls back to a
  state-preserving restart only when state can be restored, and the outcome is labelled explicitly.

## Slice 0 finding (2026-11-16)

[ADR 0005](../decisions/0005-hot-reload-as-the-bet.md) (superseded) assumed a `cdylib` compiled UI
swapped by the host via `libloading`. A Slice 0 viability spike
([spec 0004](../specs/0004-hot-reload.md), Linux x86_64, Wayland/niri, rustc 1.99.1, debug,
`gpui = "=0.2.2"`, `libloading = "0.8.9"`) found that mechanism **infeasible** for GPUI view/render
code on the current stack:

- `dlopen`/`dlclose` mechanics work for isolated C-ABI symbol calls, but mounting GPUI views from a
  separately loaded library into a host-owned GPUI application is incompatible with GPUI 0.2.2’s
  type/instance model (`Window::replace_root<E: Render>` generic root, `AnyView`/`Entity<V>`
  downcast and host `App` entity storage, thread-local element arena, `TypeId` identity, upstream
  GPUI rlib-only).
- A realistic GPUI-view `cdylib` (trivial view change, 429 MB debug artifact) warm-relinked in
  ~171 s wall / 2m50s cargo in the spike run, far outside the < 500 ms budget.

`cdylib`/`libloading` swap for GPUI view code is rejected unless new evidence resolves mount,
type/instance identity, unload safety, and measured performance.

## Option 1 consolidation (2026-11-16, prototype evidence)

Follow-up throwaway prototypes (Linux x86_64, Wayland/niri, rustc 1.99.1/cargo 1.99.1, debug
`debug = 0`, warm `dx serve --hot-patch`, `gpui = "=0.2.2"`, `subsecond 0.7.10`,
`dioxus-devtools 0.7.10`, `dx 0.7.10`, not integrated into `auro` crates/CLI) evaluated same-process
function-level hot-patching of the **tip crate**:

- **Mechanism exercised:** explicit stable `subsecond::HotFn::current(...)` boundaries in reloadable
  view code, `dioxus_devtools::connect_subsecond()`/devserver patch application, host-owned GPUI
  `Application`/window/event loop and entity state (ticker-updated counter, stable host string) kept
  in the same process/type universe.
- **Minimal-case results:** plain Rust `HotFn` patch observed at ~125 ms (`rev=0`→`rev=1`, counter
  continued). GPUI 0.2.2 app warm view edits measured ~271-450 ms save-to-first-new-render with
  host state preserved (s1: median 371.5 ms, N=10, values 438/409/371/422/372/406/339/343/350/300;
  s6: median 346 ms, N=20, 20/20 stable). Helper/layout/style changes, coordinated transient struct
  layout changes, multi-module tip-crate edits (functionally applied), build/syntax-error recovery,
  and non-dx instrumented run were demonstrated.
- **Constraints observed:** patchable code is tip-crate code reached via hot boundaries (changes
  outside boundary or init-captured async work not retroactive, e.g. `Render` impl outside `HotFn`
  and running ticker future unchanged). Tip-crate statics/render-local counters reset on patch
  (host entity state preserved). `thread_local!` used in a patched path can fail patch generation
  (`R_X86_64_TPOFF32`/`-shared`) while the app stays alive on previous code. `dx`/target-dir
  isolation and build artifact contention require care.
- **Status:** evidenced experimental candidate for the in-process direction on Linux x86_64/GPUI
  0.2.2 in minimal conditions; **not** a D6 claim for representative Auro scaffolds/apps, not
  integrated into `auro dev`, and not proven across platforms/profiles.
- **Windows plain prototype follow-up (2026-11-16, WinBoat guest, not integrated):** the same
  non-GPUI tip-crate `HotFn` mechanism was validated on Windows 11 Pro 10.0.26200.0 x64
  (rustc/cargo 1.99.0 MSVC, `dx`/`subsecond`/`dioxus-devtools` 0.7.10) with
  `dx serve --hot-patch --interactive false --platform desktop`, a clean target, and external logs.
  Build completed in 30.66 s and 3/3 warm `src\main.rs` hot-patches applied in 484/504/499 ms with
  monotonic counter continuity `rev=0 1..361 -> rev=1 421..781 -> rev=2 841..1261 -> rev=3 1321..1741`
  in the same process (`ERR_LEN=0`, `plain-proofwin-20261116152539.*`), demonstrating mechanism
  viability on Windows x86_64 for this minimal plain case only (not a D6 benchmark). Caveats:
  corrupted reused target (`libdeps-62094602.a: unknown file type`), inconclusive hidden/background
  runs, and watcher noise from in-project verbose log redirection.
- **Windows advanced front/back follow-up (2026-11-16, WinBoat guest, plain non-GPUI, not
  integrated):** a second tip crate rendered through `HotFn::current(render_frontend)` with
  host-owned state (`session`, `items`, `backend_ticks`) and an init-captured backend thread.
  View edits applied in 590 ms (`ui0`→`ui1`) and 523 ms (`ui1`→`ui2` with added
  radius/padding/icon) while `session=s5500`, `items` 4→36 and `backend_ticks` 8→72 stayed
  continuous in the same process. A source change to the backend interval (500→1500 ms) was
  hot-patched (527 ms) but the running task kept `active_interval_ms=500`; restarting with the
  changed source produced a new process (`session=s9516`, `init_backend_ms=1500`) and a 1500 ms
  tick cadence (`adv-fix-20261116161242.*`, `adv-fix-restart-20261116161242.*`, `ERR_LEN=0`). This
  is the observed hot-boundary versus init-captured split for this mechanism, not a Tauri
  backend-process comparison.
- **Windows GPUI 0.2.2 follow-up (2026-11-16, WinBoat guest, not integrated):** the GPUI prototype
  built in 181 s (23.9 MB exe) and rendered when launched into the active interactive console
  session via a scheduled task (`/RU pdl /IT`): 206 `[hot_view]` renders, `counter` 0→204,
  `stable=host-owned-state` preserved, no panic. The same exe launched from SSH/session 0 panicked
  with `Creating DirectX renderer` → `Creating DirectX resources` → `0x887A0022`
  (`DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`), as did `dx serve --hot-patch` from that context after a
  199.20 s build. A `dx serve --hot-patch` run in the interactive session itself (scheduled task,
  `SESSION=1`, clean `target-gpui-hot3`) built in 473.85 s, rendered (`label=v0`, `counter` 0→43),
  applied its first patch (`Hot-patching: src\main.rs took 1453ms`, `[patch_applied] t_ms=5760`)
  and then died with `thread 'main' has overflowed its stack` / `exit code: 0xc000041d` before any
  `label=v1` render (`gpui-hot3-20261116-170828.*`). Follow-up isolation on the same guest: the jump
  table itself is sound (runtime dump: `entries=746`, `aslr_ref=0xcab0`/`new_base=0xcb10` as RVAs,
  exe exporting `main` at RVA `0xcab0`), the crash reproduces with a 16 MiB main-thread stack
  (`/STACK:16777216`, PE-verified), a leaf `HotFn` (`fn(u64) -> u64`) patched twice in one process
  without crashing (`bumped=1050` at counter 50, `bumped=2193` at counter 193), and a
  **framework-free `ViewData` boundary** reloaded UI values twice (`1324 ms`, `1084 ms`) with
  `data=v1`→`data=v2`, 120/187 renders after each, `counter` 4→175, `stable` preserved, no crash —
  visually confirmed by a QMP screenshot of the same window (`gpui-hot7-20261116-175144.*`). The
  Windows patch DLL is 89,600 bytes with only `main` exported and no GPUI/imports from the exe, so
  the observed limit is consistent with the patch unit not carrying the framework. Windows GPUI
  render works in an interactive session; **view-code hot patching is blocked, framework-free hot
  boundaries work**.

## Integrated hot mode (2026-11-18)

`[dev] hot = true` now runs a usable loop rather than a patch that needed a click to appear:

- **Repaint on commit.** The devserver applies a patch without touching GPUI's frame; the app kept
  the previous frame, and the patched boundary only ran on the next interaction. On Linux this made
  a patch look applied (`Hot-patching ... took`, handler fired) with no visible change; this is also
  the mechanism behind the earlier Windows "boundary never ran" observation. `auro::hot::install`
  now registers a subsecond handler that bumps a revision and prints `auro:trace hot_patch <n>`;
  `App::try_run` polls it (16 ms) on the main thread and calls `cx.notify(root_entity)` for every
  open window, so the next frame runs the patched boundary.
- **Measured render.** `auro::hot::hot`/`hot_with` print `auro:trace hot_render <n>` on the first
  call after a patch. A view that validates the frame first (the generated one does) calls
  `hot_frame` and `rendered` only after the frame is accepted, so a rejected frame does not produce
  a render marker. `auro dev` combines its own save timestamp, the devserver's
  `Hot-patching ... took` line and this marker into one JSON line per edit
  (`save_to_patch_ms`, `patch_to_render_ms`, `save_to_render_ms`, `patch_took_ms`).
- **Change classification.** A save under `data/src` is a hot boundary edit. A save in `ui`,
  `types`, the bin, a dependency, a manifest or the profile is labelled `outside the hot boundary`;
  `auro dev` restarts the devserver and app, restores the snapshot and labels the outcome
  `restarted (state restored|reset)`, never as a reload. The previous app stays visible while the
  replacement builds; a failed host rebuild keeps that UI running and is reported, and boundary
  build errors also keep the previous UI. This makes the documented reload unit visible and gives
  host-code changes the documented fallback instead of a silent no-op.
- **Boundary is the reloadable unit.** `view_data() -> HotData` returns plain data; element and
  layout code in `Render` is host-stable. Only functions reached through `hot`/`hot_with` consult
  the jump table, so a patch of `render` alone does not change the running UI; `auro dev` restarts
  the devserver/app for that host-code save. The scaffold prints the boundary's returned value per
  revision so a boundary reload is visible.
- **Dev profile.** Scaffolded `Cargo.toml` defines `[profile.desktop-dev]` with
  `inherits = "dev"`, `debug = 0`, `incremental = false`. The Dioxus CLI uses that adhoc profile for
  `dx serve`; `debug_assertions` remain on, which subsecond requires. DWARF dominated the relink
  (3.3 s → ~0.5 s save-to-render on the demo), and disabling the incremental session was faster and
  steadier for the one-file ThinLink rebuild.
- **Crate boundary.** The scaffold is a four-crate workspace: `types` (`ViewData` + fixed
  `HotData`), `data` (the boundary), `ui` (`Render`, window, element tree, snapshot/autosave) and a
  thin bin. `ui` does not depend on `data`, so `dx` recompiles the changed boundary crate and the
  bin, not GPUI or the view. The patch library falls from roughly the size of the app to ~12 KB, and
  the measured Linux median fell from 521 ms to 368 ms (N=12, 325-394). A plan struct boundary
  without the fixed-frame check measured 288 ms; the guard costs ~80 ms and is kept because it
  rejects a patch built against a changed data layout instead of letting the old host read it.
- **Apply containment.** `auro::hot` opens the devserver connection itself. A patch that fails to
  load is caught (error or loader panic), reported, and the previous jump table keeps running; the
  listener stays open for the next edit. The dev loop labels it `patch rejected by the app`, never
  as a reload. A committed patch wakes the main thread through a watch channel instead of a 16 ms
  poll, so patch-to-render is 1-26 ms and an idle app does not wake twice a frame.

**Linux measurement, earlier single-crate scaffold (niri/Wayland, x86_64, 12 CPUs, rustc 1.99.1,
gpui 0.2.2, dx/subsecond/dioxus-devtools 0.7.10, warm dependency cache, boundary-only edits):**
save-to-render 473/488/519/521/521/522/528/539/548/567/572 ms (**median 521 ms, min 473, max 572,
N=11**); patch-to-render 8-24 ms; same pid and host state across patches. With `incremental = true`
the same app measured median 650.5 ms (N=10, 547-1722).

**Linux measurement, four-crate scaffold (2026-11-18):** save-to-render
393/371/385/370/366/339/357/370/394/350/325/366 ms (**median 368 ms, min 325, max 394, N=12**),
patch-to-render 1-26 ms, `patch_took` 268-325 ms, same pid/window/host state. The host was under
load average ~16 from an unrelated workspace test build for the series, so the numbers are
conservative for this machine. A type error kept the previous UI and the fixing edit applied in
270 ms (simple-frame series). A schema change applied as a patch was rejected by the running view
and left the previous values on screen.

## Platform status (2026-11-18)

- **Linux:** measured above; four-crate scaffold median 368 ms (N=12), integrated loop, state
  continuity, build-failure recovery, apply containment, schema rejection, graceful exit and Ctrl-C
  cleanup verified (niri/Wayland).
- **Windows:** integrated loop verified in the WinBoat guest (interactive console session 1, MSVC
  1.99.0): first patch after launch 2806 ms save-to-render, five steady-state patches
  1823/1505/1375/1237/1282 ms (median 1375), patch-to-render 8-38 ms; same pid and ticking host
  state across patches, a type error kept the previous UI live, the fix reloaded in 1696 ms, and
  closing the window exited the dev loop with code 0. A four-crate re-test (simple boundary)
  measured 873/1872/1735/1229/1168/961/820/2359/921 ms (N=9, median 1168, patch-to-render 3-13 ms)
  with the same pid/window/state: better, still above 500 ms. The guarded `HotData` template then
  measured 851/765/768/880/813/897/816/752/768 ms (N=9, **median 813**, min 752, max 897;
  patch-to-render 2-16 ms) through `auro dev`, same pid/window, patch DLL 14,848 bytes. Defender
  exclusions for the app and repo left four steady patches at 719-735 ms (one 1293 ms outlier);
  `incremental = true` measured 830/848/799/819 ms steady (N=4); `codegen-units = 1` measured
  810/748/769/738/719 ms (N=5, median 748). The remaining gap is MSVC compile/link, not repaint,
  the incremental session or codegen-unit count. `dx serve` must run inside the interactive
  session (a session-0 run fails before render with DirectX `0x887A0022`). Defender exclusions for
  the project/cargo/rustup directories are recommended, but in this run they left four steady
  patches at 719-735 ms (one 1293 ms outlier), not enough to reach the target. `rust-lld` linked a
  plain cargo build but dx 0.7.10 rejected the hot patch with `Missing linker args for fat link`;
  its fat-link interception requires the default linker, so lld is not a usable lever. `/OPT:REF`
  plus `/OPT:ICF` measured a 767 ms median save-to-render (N=4), no gain. A `ui/src/lib.rs` save
  restarted the devserver/app in 5.91 s with a new pid and restored snapshot; a following `data`
  edit patched in 910 ms, and a syntax error kept the previous app and printed `host rebuild
  failed; previous UI still running`. The data boundary is the reloadable unit; Windows cannot
  patch element-returning code.
- **macOS:** no local Apple host, but a manual workflow (`Hot reload macOS benchmark`) measured the
  guarded scaffold on a shared `macos-26-arm64` runner with the prebuilt dx 0.7.10: six warm-up
  edits then twelve measured, save-to-render
  536/520/444/625/1071/598/481/507/439/512/462/372 ms (N=12, **median 509.5**, min 372, max 1071;
  patch-to-render 2-17 ms), same process/window. A first cold run measured median 515 (N=12,
  431-715). The runner is noisy and the median is slightly above target; five of twelve warm edits
  were under 500. A second six-warmup run measured a 837.0 ms median (N=12, 583-2096) and passed the
  host-restart check (`restarted (state restored)`, new pid). A twelve-warmup run measured 748.5 ms,
  so extra warm-up does not remove the variance; the two six-warmup runs span 509.5-837 ms and the
  target is not claimed met. Local hardware, signing and hardened-runtime behaviour remain open.

The < 500 ms reference case is met on Linux (median 368 ms), not on Windows (median 813 ms), and
is above the target on a shared macOS runner (509.5 ms median; a second run measured 837.0 ms). It
is reported per platform, never as a single global claim.

| Platform | Hot mode | Save-to-render (warm) | Evidence |
|---|---|---|---|
| Linux x86_64, niri/Wayland | Opt-in, verified | **368 ms** median, N=12 (p95 394, 325-394) | Four-crate scaffold, `desktop-dev`, boundary edits, same pid/window/state |
| Windows 11 x86_64, MSVC | Opt-in, verified in interactive session | **813 ms** median, N=9 (p95 897, 752-897) | Guarded four-crate scaffold, `auro dev` trace, same pid/window; still above target (a plain-boundary series measured 1168 ms) |
| macOS arm64 (`macos-26-arm64` runner) | Opt-in, measured in CI | **509.5 / 837.0 ms** medians, two runs, N=12 after 6 warm-up (p95 1071/2096) | Shared GitHub runner; host restart and snapshot restore verified; no local Apple hardware, signing or hardened-runtime test |

The host-code restart path is verified on Linux, Windows and macOS CI. macOS local hardware,
signing and hardened-runtime behaviour remain unverified.

## Mechanism and layout

Per [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md), the mechanism keeps one GPUI
`Application`/window/event loop and reloads updated UI code in the same process and type universe.
The implemented mechanism is `subsecond`/`dx` function-level patching of an explicit `HotFn`
boundary, with host-owned GPUI state and a versioned snapshot. The generated layout makes the
reloadable unit a small workspace crate (`data`) that the view crate (`ui`) does not depend on,
which keeps GPUI out of the patch and brings the Linux warm-edit median under 500 ms. The boundary
returns a fixed `HotData` frame so a changed data layout is rejected rather than misinterpreted.
The layout and fixed-frame decision is recorded in
[ADR 0022](../decisions/0022-hot-reload-boundary-crate.md); the contract, measurements and
remaining platform limits are in [spec 0004](../specs/0004-hot-reload.md).

## State preservation

Regardless of mechanism, reloadable state that opts in is captured as a versioned,
`serde`-compatible snapshot owned by the host before apply or before a state-preserving restart, and
restored after success. State that cannot be restored (incompatible version/key, non-serialisable
resource, or explicit reset) must be reported, never silently replaced. Tip-crate statics and other
patch-unit-local state must not be treated as preserved across an in-process patch.

## Fallback

When a change is not safely reloadable in-process (host/bootstrap/toolchain/config changes, failed
compatibility/apply/patch generation, or mechanism limitations), the fallback is a
**state-preserving process restart**: persist snapshot, restart, restore. Slower, but it preserves
context and is labelled honestly.

## Constraints this imposes on application code

- Reloadable UI logic must cross explicit, stable hot boundaries; code outside those boundaries (or
  already captured in running futures/callbacks/init state) must not be assumed to update in place.
- Views and reloadable state must be reconstructible from host-owned snapshots/state.
- Reloadable tasks, subscriptions, and callbacks must be cancellable or invalidatable before replaced
  code can run.
- Preserved state must live on the host-stable side or in snapshotable form; tip-crate/patch-unit
  statics, thread-locals, and layout-incompatible values need reset or migration handling.
- Entity/view handles and callbacks must not be stored in global/static locations that outlive the
  reload unit or prevent safe invalidation.
- On Windows/MSVC (`dx`/`subsecond` 0.7.10, `gpui 0.2.2`) the patched unit cannot construct GPUI
  elements: an element-returning hot boundary applies and then overflows the main thread's stack.
  The unit must return framework-free data (`ViewData`-style) that host-stable code turns into
  elements; view-code hot reload there is unproven.
- Only calls that go through `HotFn` consult the jump table. A function the framework calls
  directly (the `Render` impl, element construction) is compiled into the patch but not invoked
  through the jump table, so editing it leaves the running UI unchanged. The scaffold keeps the
  element tree outside the boundary for exactly this reason.
- Hot patching requires a build with `debug_assertions` enabled: `HotFn::try_call` calls the local
  function directly in release builds, so the jump table is bypassed and code changes silently do
  not apply.
- The boundary's return type must keep a fixed layout: the running host was compiled against the old
  one. The scaffold uses `types::HotData`, a fixed `#[repr(C)]` frame with a schema number; change
  `ViewData` and its encoder, bump `SCHEMA`, and never re-lay-out `HotData`.
- The patch loader keeps each applied library mapped for the life of the process (unloading live
  patched code is unsafe). A ~12 KB patch keeps a long session bounded; an app-sized patch would
  grow the process by hundreds of MB per apply.
- A patch that cannot be loaded must not kill the listener. `auro::hot` handles `apply_patch`
  errors and loader panics and keeps the previous jump table; the next edit can still apply.
- A patched boundary must be pure data so a panic can be contained. The generated view wraps
  `hot_frame` in `catch_unwind`, reports a rejected frame and keeps the previous values; removing a
  panic applied in 327 ms in the same process.

## Open problems

- Hot mode is integrated behind `[dev] hot = true`; open: change classification for edits outside
  the boundary (currently a patched, unchanged boundary), forced fallback restart, and a headless
  reload fixture.
- Project/layout/build strategy: the four-crate `data`/`ui`/`types`/bin shape is implemented and
  documented, including migration for older scaffolds; the author contract for larger reloadable
  units is open. Edits outside `data` restart the devserver/app and restore the snapshot.
- Snapshot format/versioning, incompatible-change classification, and state migration/reset policy.
- Representative D6 measurements on Auro apps/scaffolds (cold/warm, debug/release, more hardware and
  Windows/macOS coverage); Linux, Windows and macOS each have one scaffold series.
- Debugging, symbols, breakpoints, and backtraces through the chosen reload mechanism.
- Full view-code hot reload on Windows (element-returning hot boundaries crash after apply; the
  implemented layout keeps elements host-stable), macOS feasibility, signing/entitlements (a
  hardened-runtime dylib would be refused), debugger behavior, and CI verification beyond compiling
  the feature.
