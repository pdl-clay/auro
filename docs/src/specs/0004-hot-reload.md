# 0004 - Hot reload

**Status:** Draft - opt-in hot mode integrated (four-crate data boundary; Linux median 368 ms, Windows 813 ms, macOS 509.5-956 ms across three shared CI runs); `auro dev` still relaunches by default

Spec 0001 closed Phase 0 with a relaunch loop and recorded ~60 s save-to-visible on the development
machine (relink dominates). This spec defines the contract that must replace that loop for UI work:
a long-lived dev host that applies reloadable UI changes while preserving restorable state, how
save-to-visible is measured, and the fallback when a reload cannot be performed safely.

It realises [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md)
([ADR 0005](../decisions/0005-hot-reload-as-the-bet.md), superseded, set the goal and assumed a
`cdylib`/`libloading` swap mechanism) and the
[hot reload architecture](../architecture/hot-reload.md). Where this spec learns from implementation
or research it must be corrected in the same pull request.

## Slice 0 viability spike (2026-11-16)

A time-boxed spike prototyped the previously drafted mechanism (host-owned GPUI application with a
separately built/loaded/unloaded reloadable UI `cdylib` exporting a C-ABI mount/snapshot/restore
contract) before committing implementation effort.

**Environment:** Linux 7.3.4-1-cachyos x86_64, Wayland/niri (`DISPLAY=:0`, `WAYLAND_DISPLAY=wayland-1`),
12 CPUs, 22 GiB RAM, rustc 1.99.1 (stable-x86_64-unknown-linux-gnu), cargo 1.99.1, debug profile,
warm incremental builds, `gpui = "=0.2.2"` (registry, rlib-only upstream crate), `libloading = "0.8.9"`.

**Observed:**

| Area | Observation |
|---|---|
| Baseline (relaunch) | ~60 s save-to-visible, single visible change, relink dominates (spec 0001, 2026-11-15). Not hot reload. |
| `dlopen`/`dlclose` mechanics (C-ABI symbols only, no live GPUI render/app task graph) | Load/unload, 3-5 cycle load/unload, and A/B copy swap simulations succeeded for a no-dependency `cdylib` and for a 429 MB debug `cdylib` statically linking GPUI (ABI symbol calls returned; `dlclose` returned). Necessary but not sufficient for safe unload under a live GUI session. |
| GPUI 0.2.2 mount boundary | Swapping/mounting GPUI view code from a separately loaded library into a host-owned GPUI application is not feasible under standard Rust/cargo builds. `Window::replace_root` is generic over concrete `E: Render`; `AnyView` stores a monomorphized `render::<V>` that downcasts to `Entity<V>` and mutates host `App` entity storage; `AnyElement` is arena-backed in a thread-local element arena; entities/refcounts use `TypeId` and host `App` maps. Upstream GPUI is rlib-only, so host and library carry separate GPUI instances/statics/type identities. Passing Rust/GPUI view/entity/element types across a C ABI is unsound/incompatible with normal view authoring. |
| Warm rebuild cost (trivial one-line string/view change, debug, after warm cache) | Monolithic GPUI bin: 1.81 s (1m30s initial). Trivial GPUI-dependent `cdylib` export (4.3 MB): 0.60 s (1m26s initial). No-dependency `cdylib`: 0.17 s. Realistic GPUI-view `cdylib` (`div()`/`AnyElement`, 429 MB debug): **2m50s** cargo / ~171 s wall (full relink; package-cache/build-dir lock contention noted, link-dominated). |
| Unload safety | A GPUI-linked `cdylib` had TLS and init/fini arrays; live-window/task/thread/platform-resource behavior was not exercised. Safe quiesce/unload under a running GPUI app remains unproven even if symbol-only `dlclose` returns. |

**Conclusion:** the drafted `cdylib`/`libloading` swap of GPUI view/render code is **rejected** for the
current stack (GPUI 0.2.2, standard linkage). The observable hot-reload contract remains; the
mechanism must be revised (see [D1](#d1---long-lived-dev-host-same-process-reload-mechanism-tbd)).


## Option 1 consolidation spike (2026-11-16)

Follow-up throwaway prototypes (not integrated into `auro` crates/CLI) evaluated the leading
same-process candidate on Linux 7.3.4-1-cachyos x86_64, Wayland/niri (`:0`/`wayland-1`), 12 CPUs,
22 GiB RAM, rustc 1.99.1/cargo 1.99.1 (stable), `dx 0.7.10`, `subsecond 0.7.10`,
`dioxus-devtools 0.7.10`, `gpui = "=0.2.2"` (registry, rlib-only), debug profile `debug = 0`, warm
`dx serve --hot-patch`, xattr patch as in workspace. Artifacts: `/tmp/auro-option1-spike`
(`subsecond-plain`, `gpui-subsecond-spike`, scenario logs/JSON).

**Mechanism exercised:** tip-crate function-level hot-patching (jump-table/ThinLink) with explicit
stable `subsecond::HotFn::current(...)` boundaries; `dioxus_devtools::connect_subsecond()`;
host-owned GPUI `Application`/window/event loop and entity/managed state in the same process/type
universe (root entity `counter` ticker + stable host string).

| Case | Observation |
|---|---|
| Plain Rust (non-GPUI) `HotFn` patch | `rev=0`→`rev=1` in tip-crate function: `Hot-patching ... took 125ms`, output changed while loop counter continued (state/process preserved). |
| s1 warm latency/state, minimal GPUI app (N=10, sequential trivial label edits) | 10/10 visible, host `counter`/`stable`/pid preserved. Save→first-new-render: 438,409,371,422,372,406,339,343,350,300 ms (median 371.5, min 300, max 438). `dx` hot-patch: 351,328,326,322,322,304,289,297,293,284 ms. |
| s2 view/helper/layout/style (4 edits) | Helper-only change (label unchanged) 304 ms; label+added child 292 ms; style/bg 297 ms; helper+label 383 ms; helper changes visible; host state preserved. |
| s3 outside boundary / init not retroactive | Patches visible (314,321 ms), state preserved. `Render` impl change outside `HotFn` boundary not reflected in rendered stable value; init-created host state literal not retroactive. |
| s4 tip-crate statics/reset | Before: `n=23 tip=1022 counter=19`; first patched render: `n=1 tip=1000 counter=24`, `stable` preserved. Tip-crate counters/statics reset; host entity state preserved. |
| s5 build/syntax failure + recovery | Broken edit kept app alive on previous `v0` with progressing counter (no crash/apply of broken code); fix patched (`314 ms`) to `v1-fixed` with `n=1 tip=1000 counter=1247`, state preserved. |
| s6 longevity (N=20) | 20/20 visible, all state preserved, final `v20`, same pid. Save→first-new-render median 346 ms (min 271, max 370). |
| s7 init-captured async work | Ticker-interval source edit (100→400 ms) patched but running ticker rate unchanged (~10.00→10.00 cps): captured future not retroactive. |
| s8 coordinated transient struct layout change | Added `Vm` field with coordinated construct/log/UI update in same edit: visible (362 ms), `extra` logged, counter progressed, no error. |
| s9 multi-module tip crate | Baseline `ex=m0`. Module-only edit visible as `ex=m1` (269 ms) with host state preserved; module+main visible as `v1 ex=m2`; `dx` logged hot-patches for `src/extra.rs` and `src/main.rs` (functionally applied). |
| s10 no dx/devserver | `cargo build` + run without DIOXUS/devserver: alive, 59 renders, counter 0→56 progressed, no panic (SIGINT exit) - instrumented app runs without hotpatch tooling. |
| s11 `thread_local!` in patched path | `Failed to generate patch` (`rust-lld` `R_X86_64_TPOFF32 ... cannot be used with -shared`), no patch applied, app stayed alive on `v0` with progressing counter (honest failure). |

**Conclusion:** subsecond/dx tip-crate `HotFn` patching is a **viable experimental same-process
candidate** on Linux x86_64/GPUI 0.2.2 in minimal prototype conditions (sub-500 ms warm view edits
and preserved host state demonstrated), but **not** integrated into `auro dev` and **not** a D6 claim
for representative Auro scaffolds/apps or other platforms/profiles. Required follow-up: Auro project
layout/tip-crate/build strategy, host-owned versioned snapshot/restore and incompatible-change
classification/reset, `auro dev` opt-in integration (dx/devserver or equivalent driver, target-dir
isolation, timing/trace and diagnostics), and representative D6 measurements.

**Windows plain prototype follow-up (2026-11-16, WinBoat guest, not integrated):** a throwaway
non-GPUI Rust tip-crate prototype (same `subsecond`/`dioxus-devtools`/`dx` 0.7.10 `HotFn`
mechanism, `dioxus_devtools::connect_subsecond()`, and a `HotFn::current(render).call((count,))`
loop with `[workspace]` isolation) was validated on a Windows 11 Pro 10.0.26200.0 x64 guest
(`WIN-30IUPGSQL9F`, WinBoat VM, rustc 1.99.0/cargo 1.99.0 `stable-x86_64-pc-windows-msvc`, MSVC
Build Tools 14.44.35207, debug). With a clean target
(`CARGO_TARGET_DIR=C:\Users\pdl\wvmt\op1win\target-plain-proof`), external stdout/stderr logs, and
`dx serve --hot-patch --interactive false --platform desktop`, build completed in 30.66 s and 3/3
warm hot-patches of `src\main.rs` applied in 484 ms, 504 ms, and 499 ms with monotonic counter
continuity across revisions in the same process: `rev=0 count 1..361 -> rev=1 421..781 -> rev=2
841..1261 -> rev=3 1321..1741` (30 sampled app lines, `ERR_LEN=0`; evidence:
`C:\Users\pdl\wvmt\op1win\logs\plain-proofwin-20261116152539.*`). This demonstrates the same-process
tip-crate `HotFn` patching mechanism works on Windows x86_64 in this minimal plain prototype (not a
D6 benchmark). An earlier observed run (transcript, artifacts cleaned) showed `rev=0 count=541 ->
637 ms hot-patch -> rev=1 count=601` with continuity. Caveats: non-GPUI only; not integrated into
`auro dev`; not production/GA; not representative D6 for Auro scaffolds/apps. Observed failures/caveats: reusing corrupted target `target-plain-dx3` caused
`Failed to generate fat binary: rust-lld: error: ...\desktop-dev\libdeps-62094602.a: unknown file
type`; hidden/background dx runs were inconclusive (missing/0-byte logs in early attempts); and
in-project verbose log redirection produced watcher noise/misleading hot-patch labels.

**Windows advanced front/back prototype (2026-11-16, WinBoat guest, plain non-GPUI, not
integrated):** a second throwaway tip crate modelled a host-owned UI state plus an init-captured
backend task (a thread that reads its interval once at startup), with the frontend rendered through
`HotFn::current(render_frontend)` and printed state lines (`session`, `items`, `backend_ticks`,
`init_backend_ms`). Three edits inside the hot boundary applied as warm hot-patches of
`src\main.rs`: `590 ms` (`ui0` blue/120/Save/light → `ui1` green/180/Salvar/dark), `523 ms`
(`ui1` → `ui2` orange/240/Publicar/contrast, adding `radius=12 padding=24 icon=rocket`), and
`527 ms` (backend-interval source edit). Across all three the process and host state were
continuous: `session=s5500`, `items` 4→36, `backend_ticks` 8→72, same pid, `init_backend_ms=500`.
The backend source change (`backend_tick_interval_ms` 500→1500) was hot-patched but **not
retroactive**: the running backend task kept logging `active_interval_ms=500` (18 tick lines after
the change, last `ticks=69 active_interval_ms=500`). A restart with the changed source produced a new
process (`session=s9516`, first render `ui_v=ui2`, `init_backend_ms=1500`) and a 1500 ms cadence
(`[backend_tick] ... ticks=1 active_interval_ms=1500`, ticks 1→21 over ~30 s). Evidence:
`C:\Users\pdl\wvmt\op1win\logs\adv-fix-20261116161242.*` and
`adv-fix-restart-20261116161242.*`, `ERR_LEN=0` in both phases. This is the observed
frontend-hot-boundary versus init-captured-backend split for this mechanism on Windows; it is not a
Tauri backend-process comparison and not a product contract.

**GPUI 0.2.2 on Windows (2026-11-16, WinBoat guest, not integrated):** the GPUI prototype
(`gpui = "=0.2.2"`, `subsecond`/`dioxus-devtools` 0.7.10, host-owned `RootView` counter ticker and
stable string, `HotFn::current(hot_view)` in `Render::render`) built in 181 s
(`target-gpui-win2\debug\gpui-subsecond-spike.exe`, 23.9 MB). Launched through a scheduled task in
the active interactive console session (`/RU pdl /IT`, session 1) it rendered: pid 708, 206
`[hot_view]` renders, `counter` 0→204, `stable=host-owned-state` preserved, no panic
(`gpui-run-int2-20261116162859.*`). The same executable launched from the SSH/session-0 context
panicked before render with `Creating DirectX renderer` → `Creating DirectX resources` →
`0x887A0022` (`DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`), as did a `dx serve --hot-patch` run from that
context after a 199.20 s build (`gpui-win1-20261116155242.*`, `gpui-hot-20261116163202.*`). So the
earlier launch failure is a non-interactive-session/GPU-availability condition, not a GPUI+
subsecond incompatibility. A `dx serve --hot-patch` run then executed in the interactive session
itself (scheduled task, `SESSION=1`, clean `target-gpui-hot3`): build 473.85 s, app rendered
(`[hot_view] ... label=v0`, `counter` 0→43), and the first patch applied
(`Hot-patching: src\main.rs took 1453ms`, `[patch_applied] t_ms=5760`) — after which the process
died with `thread 'main' has overflowed its stack` and `exit code: 0xc000041d`, before rendering
`label=v1` (`gpui-hot3-20261116-170828.*`, `gpui-hot3-current.result.txt`). **GPUI hot-patching on
Windows is blocked for element-returning functions, and works for a framework-free boundary.** The
investigation on the same guest (2026-11-16) established:

| Observation | Evidence |
|---|---|
| Patch transport is sound | `apply_patch` on Windows is `LoadLibrary` + a jump table (`endereço_antigo → endereço_novo`), no in-memory detour; runtime dump showed `entries=746`, `aslr_ref=0xcab0`/`new_base=0xcb10` (RVAs) and resolved targets ~19.2 GB away, and the exe exports `main` at RVA `0xcab0` as the ASLR anchor. |
| Stack budget is not the cause | the crash reproduced at the default 1 MiB main-thread stack and with `/STACK:16777216` (PE verified: `1000000 size of stack reserve`). |
| Patching an element-returning view crashes | `HotFn::current(hot_view)` applied (`Hot-patching: src\main.rs took 1238ms`, `[patch_applied] t_ms=5212`) then `thread 'main' has overflowed its stack` / `exit code: 0xc000041d`, before any patched render. |
| Patching a leaf function works, repeatedly | `HotFn::current(hot_add)` (`fn(u64) -> u64`) patched twice in one process: `bumped=1050` at `counter=50`, then `bumped=2193` at `counter=193`, 303 renders after, no crash (`Hot-patching ... 1055ms`). |
| A framework-free hot boundary reloads UI values | boundary `fn view_data() -> ViewData` (`label`, `hs`, `bg: u32`, `width: f32`, `extra`), host builds the `div()`: two warm patches (`1324 ms`, `1084 ms`) produced `data=v1` (`bg=0x65f46`, `w=600`, `extra=panel=contrast`) and `data=v2` (`bg=0x7c2d12`, `w=640`, `hs=hs2`, `extra=panel=dark radius=12 icon=rocket`), with 120 and 187 renders after each, `counter` 4→175, `stable` preserved, no crash, captured in a QMP screenshot of the same window (`gpui-hot7-20261116-175144.log`, `gpui-hot7-current.result.txt`, shot `20261116-175532`). |
| Patch DLL does not carry the framework | the Windows patch DLL is 89,600 bytes, exports only `main` (RVA `0xcb10`) and imports only KERNEL32/VCRUNTIME140/UCRT (37 functions); the Linux patch `.so` carried GPUI (429 MB). |

Consequence recorded for the mechanism contract: on Windows/MSVC with `dx`/`subsecond` 0.7.10, hot
boundaries must return framework-free data; code that constructs GPUI elements cannot be the patched
unit. `HotFn::try_call` also bypasses the jump table entirely when `debug_assertions` is off, so hot
reload requires a dev-profile build. Full view-code hot reload on Windows remains open (candidate
levers: `dx --force-fat-binary`, a full-relink patch unit, or a non-Windows patch transport).
First library surface (2026-11-16): `auro::hot` behind the off-by-default `hot-reload` feature —
`hot()` / `hot_with()` over `subsecond::HotFn` with `AURO_HOT_RELOAD=1` installing the dev-host hook.
No CLI mode, snapshot/restore, or change classification yet.


## Scope

- The `auro dev` hot-reload flow: watch, build, apply reload or fallback, and user-visible reporting.
- The host/reloadable-UI boundary for development: process ownership, reload unit, and compatibility
  contract.
- Snapshot, restore, and invalidation of state across a reload or state-preserving restart.
- Reload eligibility: which source changes may reload in-process and which must restart.
- Failure handling: build errors, incompatibility/apply errors, restore errors, panics, and
  cancellation/rollback.
- Measurability: the definition and recording of save-to-visible latency.
- Staged delivery from experimental hot reload (Phase 1) to GA with state preservation (Phase 3).

## Out of scope

- Shipping hot updates to released binaries (`auro-update`, Phase 2). This spec covers the dev
  loop only.
- Hot reload of plugins or out-of-process plugin hosts (Phase 3), except stating that plugin
  reloadability is a separate contract.
- Requiring a specific patching/swap technology. The required contract is observable
  reload/restore behaviour and measured latency, not a named technique, until a mechanism is proven
  and recorded here.
- Mobile targets.
- Preserving state that is inherently non-serialisable or external (open sockets, OS handles,
  unmanaged threads, GPU resources owned outside the snapshot contract) except making loss visible.
- A stable cross-compiler or cross-version reload ABI for third parties in 0.x.

## Baseline (measured)

| Path | Observed | Notes |
|---|---|---|
| Phase 0 relaunch (`auro dev`) | ~60 s save-to-visible | Development machine, single visible change, relink dominates; watcher reacts once per edit (spec 0001 status, 2026-11-15). Not a hot reload. |
| Slice 0 spike: realistic GPUI-view `cdylib` warm rebuild | 2m50s cargo / ~171 s wall | Debug, trivial view string change, 429 MB `cdylib`, GPUI 0.2.2, Linux x86_64; lock contention noted, relink-dominated. Rejects cdylib-view-swap as a latency path. |

Targets below are targets until measured by the method in [D6](#d6---measurability).

## Decisions

### D1 - Long-lived dev host, same-process reload (subsecond/dx, experimental)

`auro dev` in hot-reload mode runs a **host process** that owns the GPUI `Application`, windows,
event loop, Auro runtime, managed host state, and the snapshot store for the lifetime of the dev
session. UI code changes that are classified as reloadable must become visible without closing the
window and without losing opted-in state, by applying updated code/state in the same process and
same GPUI/App/type universe.

The concrete reload mechanism is **not yet selected for product integration**. It must satisfy the
observable contract in this spec (D3, D4, D6, D7) and be proven by prototype/measurement and
integrated safely before being declared supported.

**Rejected by Slice 0:** compiling UI as a separately loaded/unloaded `cdylib` and swapping it via
`libloading` for GPUI view/render code. This is infeasible with GPUI 0.2.2 under standard builds due
to generic/type-bound root mounting, `AnyView`/entity/`TypeId`/arena dependence on a single GPUI
instance and host `App`, upstream GPUI being rlib-only, and prohibitive relink cost for a realistic
view library. It must not be reintroduced for GPUI view code without new evidence resolving mount,
type/instance identity, unload safety, and measured performance.

**Evidenced candidate (Option 1 consolidation, 2026-11-16, prototype, not integrated):**
same-process function-level hot-patching of the tip crate via `subsecond 0.7.10` +
`dioxus-devtools 0.7.10` driven by `dx 0.7.10` (jump-table/ThinLink), with explicit stable
`HotFn::current(...)` boundaries in reloadable view code and host-owned GPUI
application/window/entities/managed state. On Linux x86_64/Wayland (`gpui = "=0.2.2"`, rustc/cargo
1.99.1, debug `debug = 0`, warm minimal app), observed save-to-first-new-render was ~271-450 ms
with host state preserved (s1: median 371.5 ms, N=10; s6: median 346 ms, N=20, 20/20 stable), and
core contract properties were demonstrated in prototype scenarios (patch visibility, host-state
preservation, build/syntax-error recovery keeping previous UI, patch-generation failure keeping app
alive on previous code). Documented constraints: patchable code limited to tip-crate hot
boundaries (outside-boundary/init-captured work not retroactive), tip-crate statics/reset on
patch, `thread_local!` in patched path can fail patch generation, and tooling/layout/snapshot/
classification/integration and representative Auro-scaffold D6 measurement remain pending.
A Windows 11 x86_64 plain (non-GPUI) prototype follow-up (2026-11-16, WinBoat guest, same dx/subsecond
tip-crate mechanism) additionally showed 3/3 warm `HotFn` patches in 484/504/499 ms with monotonic
counter continuity in the same process (build 30.66 s, clean target/external logs), and a second
plain prototype showed warm view edits applying in 590/523 ms with host state continuous while an
init-captured backend interval change stayed non-retroactive until restart. GPUI 0.2.2 built and
rendered on the same guest in the interactive console session (206 renders, host state preserved),
while `dx serve` started from the SSH/session-0 context hit DirectX `0x887A0022` before render. On
Windows, patching an element-returning view applies and then overflows the main thread's stack
(`0xc000041d`, reproduced at 1 MiB and 16 MiB), while a leaf `HotFn` patched twice with continuity
(`bumped=1050`/`2193`) and a framework-free `ViewData` boundary hot-reloaded UI values twice
(1324/1084 ms, `data=v1`→`data=v2`, state preserved, screenshot). `auro dev` integration remains
unproven.

*Rejected:* recompiling and relinking the whole app process on every save as the hot path. *Rejected:*
claiming a process restart is hot reload (forbidden by ADR 0008). *Rejected:* discarding the window
on every successful reload.

**Hot mode integrated (2026-11-18, experimental, opt-in).** `[dev] hot = true` makes `auro dev` run
`dx serve --hot-patch --interactive false --platform desktop --features hot-reload` with
`AURO_HOT_RELOAD=1` and `AURO_DEV_TRACE=1`; scaffolded apps expose a data-only boundary
(`view_data() -> HotData` wrapped by `auro::hot::hot_frame`/`rendered`) and define the
`desktop-dev` profile the devserver builds (`debug=0`, `incremental=false`). `auro dev` refuses to start unless `dx` is
0.7.x-compatible and the app declares the `hot-reload` feature; `auro doctor` reports the same
inputs.

The 2026-11-16 integration applied patches but did not show them, on Linux as well: GPUI kept the
frame it had already drawn, so the patched boundary only ran on the next interaction. That is the
same failure previously seen on Windows ("patch applied, boundary never observed"). `auro::hot`
now bumps a revision in the subsecond handler, prints `auro:trace hot_patch <n>`, and `App::try_run`
polls it on the main thread every 16 ms and notifies each open root view, so the next frame runs the
patched boundary. The first accepted boundary call after a patch prints
`auro:trace hot_render <n>`, which gives the dev loop an observable "new UI produced" event instead
of trusting the devserver's "Hot-patching" line. A boundary whose data is rejected by the view
prints `auro:trace hot_data_rejected <reason>` instead, so the dev loop does not count it as a
reload.

The reloadable unit is the boundary's returned value. Element/layout code in `Render` is host-stable
and is not reached through the jump table, so editing it does not change the running UI (and does not
crash on Windows); the scaffold prints the boundary value per revision to make that visible. This is
the D2 boundary, not full source-level view reload.

The loop also owns the session lifecycle: the app's pid distinguishes the first launch from a
devserver relaunch (a state report is labelled `restarted (...)` only after a pid change), a failed
build prints `build failed, previous UI still running` and applies nothing, a graceful window close
ends the loop with exit 0, and Ctrl-C ends both the devserver and the app process (killing only `dx`
orphaned the app, observed before the fix).

**Linux measurement (niri/Wayland, x86_64, 12 CPUs, rustc 1.99.1, gpui 0.2.2, dx/subsecond/
dioxus-devtools 0.7.10, demo scaffold, warm dependency cache):** boundary-only edits measured
473/488/519/521/521/522/528/539/548/567/572 ms save-to-render (N=11, **median 521**, min 473, max
572) with patch-to-render 8-24 ms; the process id and a ticking host counter continued across
patches (52→59). With `incremental=true` the same app measured median 650.5 ms (N=10, 547-1722); a
`debug=2` profile measured 3.3 s. The demo window was visually confirmed showing the patched
title/accent. The target is < 500 ms: 3/11 edits landed under it and the median did not.

**Windows measurement (2026-11-18, WinBoat guest: Windows 11 Pro 10.0.26200.0 x64, rustc/cargo
1.99.0 MSVC, dx/subsecond/dioxus-devtools 0.7.10, interactive console session 1, Defender exclusions
for the work, cargo and rustup directories):** the integrated scaffold built (151.59 s warm-target)
and rendered; the first boundary edit produced `view data -> Win1` with the patched accent in the
same process (pid 3444) while the host ticker continued (133→134...). First patch after launch:
2806 ms save-to-render (`patch_took_ms` 2715). Five steady-state patches:
1823/1505/1375/1237/1282 ms (N=5, **median 1375**), patch-to-render 8-38 ms (one cold patch 360 ms).
A type error kept the previous UI live; the fixing edit reloaded in 1696 ms with the same pid;
closing the window ended the loop with exit 0 and the task exited 0. Windows is slower than Linux
because MSVC compile/link dominates the patch, not the repaint. The data boundary remains the only
reloadable unit, as on Linux.

**Windows re-test, four-crate scaffold (2026-11-18, WinBoat guest, simple data boundary):** a fresh
`auro new` app with `ui`/`data`/`types`/bin and a plain `ViewData` boundary applied nine patches in
873/1872/1735/1229/1168/961/820/2359/921 ms save-to-render (N=9, **median 1168**, min 820, max
2359; `patch_took` 686-2196, patch-to-render 3-13 ms) with the same pid, window and state. That is
~15% below the earlier single-crate median (1375 ms) but still above 500 ms. The guarded `HotData`
template then ran through `auro dev` in the same guest: 851/765/768/880/813/897/816/752/768 ms
(N=9, **median 813**, p95 897, min 752, max 897; patch-to-render 2-16 ms), same pid/window, patch DLL
14,848 bytes. Adding Defender exclusions for the app and repo did not change the conclusion: four
steady patches measured 719-735 ms (one 1293 ms outlier). `incremental = true` measured a 1966 ms
first patch and 830/848/799/819 ms steady (N=4), also no improvement. `codegen-units = 1` measured
810/748/769/738/719 ms (N=5, median 748). Responsibility for the remaining Windows gap is in the
MSVC compile/link of the boundary and bin, not the repaint, the incremental session or codegen-unit
count. `rust-lld` linked a plain `cargo build` successfully, but `dx serve --hot-patch` refused
with `Missing linker args for fat link of 'auro_hot_template2'. The tip crate likely did not run
through linker interception for this build`; dx 0.7.10's fat-link interception requires the default
linker, so lld is not a usable lever for the Windows patch path. Adding `/OPT:REF` and `/OPT:ICF`
to the MSVC link measured 765/817/848/767 ms save-to-render (N=4, median 767; patch_took
641-727 ms), no improvement over the 748 ms codegen-units series. The host-code restart path was
also exercised on Windows with the latest CLI: a `ui/src/lib.rs` save stopped dx and the app,
rebuilt in 5.91 s, launched a new pid and restored the snapshot; a following `data` edit
hot-patched in 910 ms (11 ms patch-to-render).

**macOS measurement (2026-11-18, GitHub macOS 26 arm64 runner).** There is no local Apple host,
but the manual `Hot reload macOS benchmark` workflow ran the guarded four-crate scaffold with the
prebuilt dx 0.7.10 on `macos-latest` (`macos-26-arm64`): `auro dev`, six warm-up boundary edits,
then twelve measured edits. Save-to-render was
536/520/444/625/1071/598/481/507/439/512/462/372 ms (N=12, **median 509.5**, p95 1071, min 372, max 1071;
patch-to-render 2-17 ms), with the app process and window kept. A first cold run without warm-up
measured median 515 ms (N=12, 431-715). A second six-warmup run, with a seeded snapshot and a
host-restart check appended, measured median 837.0 ms (N=12, p95 2096, 583-2096); the restart passed:
`outside the hot boundary; restarting`, `restarted (state restored)` and a new app pid. A third run
with twelve warm-up edits measured median 748.5 ms (N=12, p95 1114, 611-1114), so extra warm-up does not fix
the variance. The two six-warmup runs span 509.5-837 ms on the shared runner; that is not stable
enough to claim the target met. No local Apple hardware, hardened-runtime build, signing or
interactive Aqua session was exercised. macOS is measured but not GA, and CI also builds/tests the
feature and the generated scaffold on `macos-latest`.

**Boundary crate split (2026-11-18, integrated).** The first integrated scaffold kept the boundary
and the GPUI view in the same bin crate. Every patch then recompiled and relinked the view code and
`dx` produced a patch library containing GPUI; the measured Linux median was 521 ms and the patch
artifact was the size of the app. The scaffold now generates four workspace crates:

| Crate | Contents | Depends on |
|---|---|---|
| `types` | the decoded `ViewData` and the fixed `HotData` frame | nothing |
| `data` | the hot boundary `view_data() -> HotData` | `types` |
| `ui` | `Render`, window code, the element tree, snapshot/autosave | `auro`, `types`, `serde` |
| the bin | `main` wiring `ui::run(data::view_data)` | `ui`, `data` |

`ui` deliberately does not depend on `data`: `dx` recompiles the changed crate and its dependents,
so editing the boundary recompiles `data` and the thin bin, not GPUI or the view. The patch library
is ~12 KB instead of the size of the fat binary (the loader keeps each applied library mapped, so
this also bounds long-session growth), apply and load are cheap, and the steady-state
save-to-render distribution on the same Linux reference machine was 325-394 ms (N=12, median 368)
with `patch_took` 268-325 ms and patch-to-render 1-26 ms; the same setup with a plain struct
boundary and no fixed-frame check measured median 288 ms (N=12). The process id, window and host
state continued across patches. A scaffold test asserts the crate edges (no `ui -> data`, no
`data -> auro`) so the fast path cannot be lost by an innocuous dependency addition.

**Apply-failure containment (2026-11-18).** `auro::hot` owns the devserver connection instead of
delegating to `dioxus_devtools::connect_subsecond`, which unwraps a failed apply and kills the
listener. A patch that cannot be loaded (missing sentinel, bad relocation, truncated file, loader
panic) is caught, reported as `auro:trace hot_patch_failed <reason>` and as
`auro: patch rejected by the app: <reason>`, and the previous jump table keeps running. The
websocket stays open, so the next edit can still apply. A committed patch wakes the main thread
through a `tokio::sync::watch` channel rather than a 16 ms poll; the measured patch-to-render
latency dropped to 1-26 ms and an idle app no longer wakes twice a frame.


### D2 - Host-stable and reloadable code have an explicit boundary

Code that must survive a reload lives on the host-stable side: process bootstrap, window lifecycle,
the tokio runtime, host-owned managed state containers, snapshot persistence, and the reload/apply
machinery. Code that defines views and reloadable UI behaviour lives in the reloadable side.

The generated project layout and the `auro` API must make this boundary explicit once a mechanism is
chosen. A single-crate bin scaffold (Phase 0 shape) is sufficient for relaunch mode but is not known
sufficient for in-process hot reload; hot-reload-capable projects may require a specific layout,
build flags, or patchable/reloadable compilation units. The exact shape is fixed by the
implementation/prototype that proves the mechanism and recorded here when chosen.

State, tasks, subscriptions, and callbacks owned by reloadable code must have an explicit teardown or
invalidation contract so replaced code is never invoked after a reload completes or is rolled back.

*Rejected:* implicit, arbitrary mutation of live memory without a snapshot/restore and
teardown/rollback contract. *Rejected:* storing entity handles or view callbacks in process-global
state that cannot be invalidated on reload.

**Implemented layout (2026-11-18, [ADR 0022](../decisions/0022-hot-reload-boundary-crate.md)).**
The boundary is a crate edge, not a convention: `ui` owns the
host-stable `Render`/window/element code and must not depend on `data`; `data` owns the reloadable
boundary and depends only on `types`. The bin wires the function pointer
(`ui::run(data::view_data)`), so a patch replaces code in `data` without recompiling the view. The
scaffold test checks both edges. `types::HotData` is the fixed, `#[repr(C)]` frame exchanged with
the running host; `ViewData` can change inside it. A schema mismatch is rejected by the host and
reported, and the old values stay on screen instead of being reinterpreted under a new layout.

### D3 - State survives as a versioned, opt-in snapshot

The host owns the snapshot store. A snapshot is a versioned envelope containing named, typed entries
for state that opts into preservation. At minimum the format records:

- `format_version` (snapshot schema version, integer).
- a build/session identity sufficient to diagnose mismatches (e.g. build id/hash of the reload
  unit).
- per-entry payloads: a stable key, an entry version, and serialized bytes (the canonical encoding
  is `serde`-compatible; the wire encoding is fixed at implementation time and recorded here).

State categories:

| Category | Owner | Reload behaviour |
|---|---|---|
| View/UI state explicitly marked restorable | Reloadable UI, via snapshot contract | Serialized before apply/restart fallback, restored after successful apply/start. On decode/version mismatch, reset that entry and report it. |
| Host-managed state outside the replaced/restarted UI unit | Host | Preserved in memory across in-process reload when host-stable; preserved across state-preserving restart only through snapshot/persisted host state. Never passed as a borrowed reference that outlives its owner. |
| Ephemeral/UI-local state not opted in | Reloadable UI | Reset on reload/restart. Reset must be visible in dev diagnostics, not silent. |
| External/non-serialisable resources | OS/runtime | Recreated or dropped according to the resource contract; loss reported. |

A view or state unit opts into preservation by implementing reload snapshot/restore hooks for its
stable key. “Stable key” means stable across saves for the same logical UI element within a dev
session (window label + root/view identity as defined by the API). Renaming/removing a key is a
reset of that entry.

*Rejected:* snapshotting arbitrary memory or entity graphs implicitly. *Rejected:* a silent fallback
to default state on mismatch.

**Implemented (2026-11-16).** `auro-core::snapshot`, re-exported as
`auro::{Snapshot, ReloadSnapshot, Restored, ResetReason}`, fixes the envelope: JSON with
`format_version` (currently `1`), the `build` identity of the writer, and a map of stable key to
`{ "version": u32, "payload": <serde JSON> }`. State opts in by implementing `ReloadSnapshot` with
its `KEY` and `VERSION`. `Snapshot::restore` consumes one entry; a missing key, a version mismatch or
an undecodable payload drops **only that entry** and returns `Restored::Reset(reason)` whose
`message()` is printable — nothing defaults silently. A `format_version` this build does not
understand makes `Snapshot::decode` fail outright. `Snapshot::save_to` writes atomically (sibling
temporary file plus rename) and `load_from` separates "missing" (`None`) from "unreadable" (error),
so a corrupted file never looks like fresh state. `auro_core::Autosave` captures watched entries on
the thread that calls `tick()` (UI state stays on the main thread; no background reader), writes
after a 250 ms quiet period, and exposes `flush()` for an explicit save; the dev path is
`<platform data dir>/<identifier>/dev-snapshot.json`; a leading UTF-8 BOM is tolerated on load. The
scaffold wires it: `CounterState`
implements `ReloadSnapshot`, the view restores on start and prints `auro: state restored from <path>`
or `auro: state reset: <reason>`, and a click marks dirty then ticks 350 ms later. Verified on
niri/Wayland 2026-11-16: a hand-written snapshot with `count: 7` restored (`state restored`), and the
same entry with `version: 2` produced `state reset: snapshot entry version 2 cannot be read by
version 1; state starts from its default`; both runs opened the window and exited 0, and the file was
left untouched because no click happened. Not yet wired to capture/restore around a reload or a
state-preserving restart.

### D4 - Successful reload, failed build, and fallback are distinct outcomes

| Outcome | Condition | Host behaviour | User-visible label |
|---|---|---|---|
| Hot-reloaded | Reloadable build/patch succeeds, compatibility checks pass, apply and restore complete (with or without per-entry resets) | Keep window and host state; present updated UI; repaint | “reloaded” with duration and any reset warnings |
| Build failed | A boundary build/patch compilation fails | Keep previous UI active and interactive; do not apply a partial reload | compiler/build diagnostics; “previous UI still running” |
| Host restart, rebuild failed | A host-code save restarted the devserver and its rebuild fails | Keep the previous app running; keep the devserver for the next save; do not apply a partial reload | “host rebuild failed; previous UI still running” |
| Reload failed, recovered | Build/patch succeeds but compatibility/apply/restore fails and previous UI remains valid or can be safely rolled back | Keep or restore previous UI as applicable; do not present as reloaded | explicit failure reason |
| State-preserving restart (fallback) | Reload cannot be completed safely, or a configured/required restart change occurred | Restart app/host session; restore the last snapshot the app wrote to its host-controlled store. State changed since that write is reset and reported. | “restarted (state restored)” or “restarted (state reset: \<reason\>)” - never “hot reloaded” |
| Hard failure | Restart/restore impossible | Exit non-zero with diagnostic | error |

A failed build must never apply broken code. A boundary build failure keeps the previous UI active.
A host-code save restarts the devserver while keeping the previous app visible; a failed host
rebuild therefore keeps that app running, is reported, and the next successful save starts a new
app before the old one is retired. Reloadable tasks/callbacks that can outlive a reload must be
cancellable or invalidated before replaced code can run; the host must not invoke replaced code
after a failed or completed reload.

*Rejected:* applying partial/broken reloads. *Rejected:* leaving tasks running against replaced code.
*Rejected:* presenting a restart as hot reload.

**Implemented (2026-11-16, relaunch mode).** The dev loop reads the app's snapshot report from its
stderr and prints the label for the relaunch that follows a save: `auro: restarted (state restored)`
or `auro: restarted (state reset: <reason>)`. The first launch prints no label because nothing
restarted. A failed boundary rebuild prints the tail of the compiler output and keeps the previous
UI running. A failed host rebuild is reported as `host rebuild failed; previous UI still running`;
the old app stays visible until the replacement launches.
On Windows the app runs from `target/auro-dev/<stem>-<pid>-<n>.exe`: a running image cannot be
relinked, and a staging failure is reported while the previous UI stays up. Verified on niri/Wayland
and on Windows 11 (WinBoat), both with a hand-written snapshot: restore and version-mismatch reset
each produced the matching label, initial start produced none, and the two relaunches were traced at
2408/2357/2380 ms (Linux) and 10754/8159 ms (Windows) save-to-build, `debug`, warm dependency cache.

**Implemented (2026-11-18, hot mode).** `auro dev` labels the hot outcomes separately: an applied
patch prints `hot patch applied (<took> ms, <elapsed> ms after the save)` and, when the boundary
renders, `reloaded in <n> ms (window kept)` plus the JSON trace; a failed boundary build prints
`build failed, previous UI still running` and applies nothing; `Failed to hot-patch app` prints
`hot patch failed, dev server is rebuilding the app`. A host-code save prints `outside the hot
boundary; restarting the app (state preserved)` and a failed host rebuild prints `host rebuild
failed; previous UI still running`. A relaunch is detected by a changed app
pid (`auro:trace pid`) and only then is the snapshot report labelled `restarted (state restored)` or
`restarted (state reset: <reason>)`; the first launch is not labelled. A graceful app exit ends the
loop with exit 0, and Ctrl-C kills the devserver and the app process (verified on Linux; the Windows
run was ended by closing its window, which also exited 0). The `< 500 ms` target is met by the Linux
median (368 ms) for the documented warm case; Windows exceeds it and macOS is above it on a shared
CI runner (see D6). The label never claims a target it did not measure.

**Rejected patches (2026-11-18).** A patch the app cannot load prints
`auro: patch rejected by the app: <reason>`; the previous UI stays live and no timing line is
emitted. A frame the view refuses (schema or bounds check) prints
`auro: reload rejected by the view: <reason>; previous UI kept` and is never presented as a
successful reload; the view reports that frame as rejected rather than rendered
(`hot_frame`/`rendered`), so no `hot_render` or timing line is produced. Observed on Linux: a
`SCHEMA` bump produced `hot_data_rejected` and left the previous values, no reload line; reverting
`SCHEMA` then applied in 959 ms in the same process, so the listener and jump table survived the
rejection. A patched boundary that panics is caught by the generated view, reported as
`hot data rejected: the patched boundary panicked`, and leaves the previous values and process
alive; removing the panic applied in 327 ms in the same process.

**Host-code fallback (2026-11-18).** A save in `ui/src/lib.rs` was classified outside the hot
boundary, the devserver and app were restarted (build 3.14 s), the app printed `state restored` from
the existing snapshot, and the dev loop printed `restarted (state restored)`; the new process
rendered the edited button and a following `data` edit hot-patched in 978 ms with a 5 ms
patch-to-render. Editor swap files beside the source produced no restart. A host rebuild with a
syntax error kept the previous app alive (same pid) and printed `auro: host rebuild failed;
previous UI still running`; the fixed save rebuilt in 3.15 s, launched a new pid and restored the
snapshot. The same host restart succeeded on Windows: a `ui/src/lib.rs` save rebuilt in 5.91 s,
launched a new pid, restored the snapshot and the next `data` edit patched in 910 ms. A Windows
host rebuild with a syntax error kept the previous app (pid 11740) and printed `host rebuild failed;
previous UI still running`. macOS CI also passed the restart check (`restarted (state restored)`,
new pid) after a seeded snapshot. The old app remains interactive during the build; state changed
there after the replacement has loaded its snapshot is not guaranteed to carry over. This is the
state-preserving restart path required by D4/D7, not a reload.

### D5 - Compatibility is enforced, not assumed

The dev host and reloadable UI are built under a compatible toolchain/profile/target/dependency set
for the selected reload mechanism. Before apply, the host checks a build identity and any
mechanism-specific compatibility data required to avoid undefined behaviour; a mismatch is a reload
failure (D4), not a best-effort apply.

If the chosen mechanism involves dynamically loaded artifacts, teardown/quiesce and unload safety are
required; leaked unloads or live references into unloaded code are treated as reload failure with
safe fallback. If the chosen mechanism patches in-process code, patch applicability/rollback safety
must be explicit and diagnosable.

*Rejected:* promising stable cross-version reload compatibility in 0.x. *Rejected:* silent
best-effort apply on mismatch.

**Implemented guard (2026-11-18).** The generated boundary returns `types::HotData`, a fixed
`#[repr(C)]` frame (240 data bytes) with a schema number, title length and accent. The running host
checks the schema and bounds before decoding any field, so a patch compiled against a changed
`ViewData` cannot make the old host read a different layout. `types` documents the rule: change
`ViewData` and its encoder, bump `HotData::SCHEMA`; do not change `HotData`'s shape. The raw typed
`auro::hot::hot` boundary remains available for advanced cases and carries the documented layout
agreement; the scaffold uses the guarded frame.

### D6 - Measurability

“Save-to-visible” is defined as: **wall-clock time from the editor save event observed by the
watcher to the first frame presenting the updated UI** (or, when pixels cannot be observed
headlessly, to the host’s “reload committed and repaint requested” event - the method must be stated
with the measurement). Each recorded measurement states:

- machine/OS, CPU class, storage class (if relevant), debug/release, warm/cold incremental state;
- what changed (view-only, style/token, command signature, config, dependency) and file count;
- whether the outcome was hot-reloaded, reset entries, or fallback restart;
- median, p95 and spread over N >= 10 edits after warm-up (or an explained smaller N). The current
  trace separates watcher→devserver (`save_to_patch`), dx's reported patch work (`patch_took`,
  compile+link+load as dx reports it), and apply→first render (`patch_to_render`). dx 0.7.10 does
  not expose compile, link and loader-load separately; that finer split remains open.

The host must emit machine-readable timing events in dev mode (log/trace or structured output)
sufficient to compute this metric without manual stopwatch use. `AURO_DEV_TRACE` may be extended for
this purpose. Numbers published in docs, roadmap, or performance budget are measured or explicitly
marked as targets.

**Implemented (2026-11-16, relaunch mode).** `auro dev` sets `AURO_DEV_TRACE` for the child; the app
then prints `auro:trace window_requested <label>` immediately after `open_window`, and the dev loop
prints one line per save:

```
{"event":"relaunch","window":"main","save_to_build_ms":N,"build_to_window_ms":N,"save_to_window_ms":N}
```

`saved_at` is the first watcher event of a burst (not the editor's own save timestamp), `built_at` is
the moment the replacement binary exists, and the window marker means the first repaint was
requested - the method is "repaint requested", not measured pixels. The loop polls the watcher at
50 ms, which bounds the resolution of `build_to_window_ms`.

**Measured run (2026-11-16, niri/Wayland, x86_64, debug, warm dependency cache, scaffold demo,
three consecutive and visible reloads):** `save_to_build_ms` 2408/2357/2380, `build_to_window_ms`
400/350/350, `save_to_window_ms` 2809/2708/2730 (medians 2380 / 350 / 2730); closing the window
through `auro dev` exited 0. This is relaunch mode, not hot reload, and it is far below the earlier
~60 s figure recorded for a different configuration; both numbers stay labelled with their method.

**Implemented (2026-11-18, hot mode).** The dev loop sets `AURO_DEV_TRACE=1` for `dx`; the app prints
`auro:trace pid <pid>`, the patch handler prints `auro:trace hot_patch <n>`, and the first hot
boundary render after a patch prints `auro:trace hot_render <n>`. Each edit produces one line:

```
{"event":"hot_reload","save_to_patch_ms":N,"patch_to_render_ms":N,"save_to_render_ms":N,"patch_took_ms":N}
```

`save_to_patch_ms` is the dev loop's own watcher timestamp to the devserver's patch line (or the
handler marker when the line is missing); `patch_to_render_ms` ends at the marker printed inside the
boundary call, i.e. the render that produces the new frame, not a measured pixel. The main thread
polls the patch revision every 16 ms, and the loop polls its channels every 50 ms; those bound the
resolution. `patch_took_ms` is `dx`'s own report and is not used for the target.

**Measured run (2026-11-18, niri/Wayland, x86_64, 12 CPUs, rustc 1.99.1, gpui 0.2.2,
dx/subsecond/dioxus-devtools 0.7.10, scaffold demo, `desktop-dev` `debug=0`, `incremental=false`,
warm dependency cache, boundary-only edits):** `save_to_render_ms`
473/488/519/521/521/522/528/539/548/567/572 (N=11, median 521, min 473, max 572);
`patch_to_render_ms` 8-24; `patch_took_ms` 491-575. The same app with `incremental=true` measured
median 650.5 ms (N=10, range 547-1722). An earlier `debug=2` profile measured 3.3 s. Host state
continued across patches in the same process (pid unchanged, counter 52→59), and a deliberate type
error kept the previous UI live until the fixing edit reloaded in 634 ms. A fresh `auro new` scaffold
of the template reloaded its boundary in 580 ms (revision 1, same pid), which verifies the generated
manifest/profile as well as the demo fixture. Windows numbers are in D1/platform notes; macOS is
unmeasured, so the target stays a target.

**Measured run (2026-11-18, four-crate scaffold, niri/Wayland, x86_64, 12 CPUs, rustc 1.99.1,
gpui 0.2.2, dx/subsecond/dioxus-devtools 0.7.10, `desktop-dev` `debug=0`, `incremental=false`,
warm dependency cache, boundary edits in `data/src/lib.rs`):** `save_to_render_ms`
393/371/385/370/366/339/357/370/394/350/325/366 (N=12, **median 368**, p95 394, min 325, max 394);
`patch_to_render_ms` 1-26; `patch_took_ms` 268-325. The same layout with a plain struct boundary
(no fixed-frame guard) measured 306/271/292/252/270/247/313/264/387/357/309/283 (N=12, median
287.5, min 247, max 387), isolating ~80 ms for the guarded frame. Both series ran while an
unrelated workspace test build kept the host at load average ~16; the numbers are therefore
conservative for this machine. The first patch after launch can be slower (1698 ms on the guarded
scaffold) because `dx` compiles the changed boundary crate and its tip for the first time; the
target case is a warm edit, and the first patch is reported separately in the raw log. The same pid,
window and host state continued across all patches. A type error kept the previous UI live and the
fix applied in 270 ms (simple frame). A `types::HotData::SCHEMA` change applied as a patch was
rejected by the running view (`hot data rejected: ... different view-data schema`), left the
previous values on screen and did not crash.

**Measured run (2026-11-18, macOS 26 arm64 GitHub runner, guarded four-crate scaffold):** the
manual `Hot reload macOS benchmark` workflow installed the prebuilt dx 0.7.10, ran `auro dev`
with `AURO_HOT_RELOAD=1`/`AURO_DEV_TRACE=1`, applied six warm-up edits, then twelve measured edits
to `data/src/lib.rs`. `save_to_render_ms` was
536/520/444/625/1071/598/481/507/439/512/462/372 (N=12, **median 509.5**, min 372, max 1071);
`patch_to_render_ms` 2-17; the app process and window stayed. The first cold run without warm-up
measured 715/699/517/513/467/655/571/439/445/698/508/431 (N=12, median 515, min 431, max 715).
`macos-26-arm64` is a shared GitHub runner, so the spread and the 1071 ms outlier are environment
noise, not a controlled hardware benchmark; no local Apple hardware, signing or hardened-runtime
build was exercised. Five of the twelve warm edits were under 500 ms, so macOS is recorded as at
the target boundary, not as met.

Target (from ADR 0008 / roadmap): **< 500 ms save-to-visible** for a view-only change on a warm
incremental build, with state preserved. This is a target, not a guarantee, until recorded under D6.
The integrated scaffold uses a warm dependency cache and a one-file ThinLink rebuild with
`incremental=false`; that is the measured method, and the target wording predates it.

*Rejected:* unverifiable latency claims. *Rejected:* counting only compile time while excluding
apply/restore/repaint.

### D7 - Reload eligibility and staged delivery

Not every edit is safely reloadable in-process. The dev loop classifies changes:

| Change class | Expected path | Notes |
|---|---|---|
| Reloadable UI/view code and resources within the supported reload unit | Hot-reload | Primary path; must preserve opted-in state per D3. |
| Host-stable/bootstrap code, host dependency graph, toolchain/config affecting host/runtime compatibility | State-preserving restart or full restart | Classified and reported; never a silent no-op. |
| `Auro.toml` / window-identity changes | Reload config in host where safe; otherwise restart with explanation | Must not corrupt running host assumptions. |
| Compatibility/build identity mismatch after rebuild | Reload failed → fallback per D4/D5 | |

The classification rules are part of the implementation contract and must be documented for app
authors once implemented.

**Hot mode rules (2026-11-18, implemented):** only code reached through `auro::hot::hot`/
`hot_with` reloads in-process. In the generated four-crate layout the reloadable unit is
`data/src/lib.rs`; `ui` (the `Render` impl, layout, element construction) is outside the jump table
and is deliberately not a dependency of `data`. Editing `ui`, `types`, the bin, a dependency or the
dev profile is host code: a patch cannot replace the running view's code. A save outside
`data/src` is labelled at save time (`auro: change in ui/src/lib.rs is outside the hot boundary;
restarting the app (state preserved)`), the devserver and app are restarted, the snapshot is
restored, and the outcome is labelled `restarted (state restored)` or
`restarted (state reset: <reason>)`, never `reloaded`. Editor swap/lock files are ignored. The
`types::HotData` schema check still rejects a frame built against a changed data layout if a patch
races the restart. A change to a non-tip crate or the dev profile takes the same restart path.
A patch that cannot be generated makes `dx` start a full rebuild; the dev loop prints
`hot patch failed, dev server is rebuilding the app`. A build error in a boundary patch keeps the
previous UI live. A host-code change keeps the previous app visible while the replacement builds;
a failed host rebuild leaves that UI running and the dev loop reports
`host rebuild failed; previous UI still running`.

Staged delivery (aligns roadmap):

- **Experimental (Phase 1):** opt-in hot reload in `auro dev` for supported project layouts and
  mechanisms; view-only reloads; snapshot/restore for opted-in view state; visible diagnostics and
  timing; fallback to state-preserving restart where reload is not possible. Correctness and honesty
  over coverage.
- **GA (Phase 3):** default dev experience for supported apps; documented state contract stable enough
  for third-party app authors; measured latency published per platform tier; CI/perf-budget integration
  as the budget becomes enforceable.

*Rejected:* enabling an unsafe/undefined reload path by default. *Rejected:* declaring GA without
measured D6 evidence for the documented case.

## Interfaces

This section fixes the surfaces this spec introduces or constrains. Signatures shown as
`rust,ignore` are illustrative until implemented; normative details are the behavioural contracts
above and the keys/items listed here.

### CLI (`auro-cli`)

| Surface | Behaviour |
|---|---|
| `auro dev` | Default remains the documented relaunch loop until hot reload is declared stable for the selected project; hot reload is enabled by an explicit opt-in (flag and/or `[dev]` config key defined at implementation time and recorded in spec 0002/CLI docs). |
| `auro dev` (hot-reload mode) | Starts/uses the dev host, watches configured paths, builds the reload unit(s), performs reload or fallback per D4, streams diagnostics, and emits timing events per D6. |
| Exit/status | Ctrl-C stops host and child tasks cleanly (exit 0). Natural app exit in dev follows current contract. A failed boundary build keeps the session alive with previous UI; a failed host rebuild keeps the previous app and the devserver alive and reports it. A non-recoverable host/reload failure exits non-zero with a labelled reason. |
| Diagnostics | Build failures, compatibility mismatches, apply/restore failures, snapshot resets, and fallbacks are reported with actionable labels distinguishing “reloaded”, “previous UI still running”, and “restarted (state restored/reset)”. |

### Configuration (`Auro.toml`, coordinated with spec 0002)

Additions required by this spec (exact key names finalised at implementation; defaults must preserve
current relaunch behaviour until GA):

- A dev setting to enable/select hot reload mode (experimental opt-in).
- Watch/build inputs sufficient to distinguish host-stable and reloadable rebuilds, or an explicit
  documented convention derived from project layout/mechanism.
- Optional dev settings for snapshot/fallback behaviour if needed (e.g. restart-on-reload-failure
  policy) - only if they change observable behaviour; otherwise internal.

Any new key is added to spec 0002, validation, and `auro doctor` in the same change that implements
it.

### Application / runtime API (`auro`, `auro-core`, `auro-ui`, `auro-ipc`)

- A way to declare reloadable roots/views and their stable snapshot identity.
- A snapshot/restore hook for opted-in view state (trait or callback), invoked only on the main
  thread with host-provided serialization context.
- A host-visible distinction between host-stable managed state and reloadable-UI-owned state, so
  reload safety (D2, D3, D5) can be enforced or diagnosed.
- Teardown/cancellation/invalidation integration so reloadable tasks and subscriptions cannot run
  against replaced code after reload.

Illustrative (non-normative):

```rust,ignore
pub trait ReloadSnapshot: serde::Serialize + serde::de::DeserializeOwned {
    const SNAPSHOT_KEY: &'static str;
    const SNAPSHOT_VERSION: u32;
}
```

### Reload mechanism contract (to be defined by prototype)

The implementation must define and record a mechanism-specific contract covering, at minimum:

- build identity and compatibility checks performed before apply;
- how updated UI code is applied in the same GPUI/App process;
- snapshot capture/restore ordering and rollback on failure;
- teardown/quiesce of reloadable tasks, callbacks, and subscriptions;
- diagnostics for incompatibility, apply failure, and reset state.

A separately loaded/unloaded `cdylib` C-ABI contract for GPUI view code is not part of this spec
after the Slice 0 rejection unless new evidence overturns that conclusion.

### Generated project layout

`auro new` must continue to scaffold a project that builds and runs under the default (relaunch) dev
loop. When hot reload mode is enabled for a project, the project must satisfy the documented layout
and build/mechanism requirements; `auro doctor` reports whether the current project is hot-reload
capable and why not. Migration/upgrade steps for existing Phase 0 scaffolds are documented in the
implementing change.

**Migration (2026-11-18).** An existing single-crate app becomes hot-reload capable in this shape:

1. Split it into a workspace with a thin bin, `ui` (view, `Render`, element tree, snapshot/autosave;
   depends on `auro` and `types`, never on `data`), `data` (the boundary; depends only on `types`),
   and `types` (`ViewData` plus the fixed `HotData` frame). The generated app is the reference:
   `cargo new` a scaffold and copy the manifests. `ui` must not depend on `data`; that edge is what
   keeps GPUI out of the patch.
2. Put the changing values in `data::view_data() -> HotData` and call `ui::run(data::view_data)`
   from `main`. The view decodes the frame; the boundary never constructs GPUI types.
3. Add `[features] hot-reload = ["<app>-ui/hot-reload"]` (the ui crate forwards to
   `auro/hot-reload`), the `[profile.desktop-dev]` block (`inherits = "dev"`, `debug = 0`,
   `incremental = false`), and a `Dioxus.toml` with `default_platform = "desktop"`.
4. Set `[dev] hot = true` and include `src`, `ui/src`, `data/src`, `types/src` and `Auro.toml` in
   `[dev] watch`. `auro doctor` prints which of the manifest feature, `dx`, and the config flag is
   missing.

A single-crate app can still run the relaunch loop unchanged; hot mode only claims the supported
layout.

## Acceptance

Checks are runnable by someone who did not write the implementation. Items that require a real
display or platform-specific behaviour are marked; headless/CI-safe subsets are preferred where
possible. Acceptance 1-5 define the contract; the status paragraph after the list records what the
integrated opt-in path demonstrates and what remains. Prototype evidence (Option 1 consolidation,
2026-11-16, not integrated) demonstrated analogous properties in minimal GPUI 0.2.2 scenarios
(host-state preservation across patches, build/syntax failure keeping previous UI alive,
patch-generation failure keeping app on previous code, and warm view-only save-to-first-new-render
~271-450 ms, s1/s6) before the feature was integrated.

1. **Reload correctness (headless where feasible):** a host test harness mounts/runs a reloadable UI
   fixture, applies a state mutation, applies a newly built fixture with changed view output, and
   asserts opted-in state is restored and the new output is observable. A fixture whose snapshot
   version/key is incompatible resets only the affected entry and reports a visible reset diagnostic.
2. **Build failure keeps UI live:** with a running UI, an intentionally failing rebuild leaves the
   previous UI active and interactive; diagnostics are surfaced; no broken reload is applied.
3. **Reload failure safety:** a compatibility/apply/restore failure does not crash the host; the
   outcome follows D4 and is labelled (previous UI retained/rolled back or explicit state-preserving
   restart).
4. **No replaced-code execution:** teardown/invalidation prevents reloadable tasks/callbacks from
   running against replaced code after apply/rollback (verified by a fixture that would otherwise
   touch stale state; test fails if invoked).
5. **Fallback honesty:** a forced fallback restart restores snapshot state when possible and is
   reported as “restarted (state restored/reset)”, never as “hot reloaded”.
6. **Measurability:** in hot-reload mode the dev session emits timing/trace data sufficient to
   compute save-to-visible per D6; at least one measured run is recorded in this spec (or linked
   artefact) with machine, method, N, median, p95, and outcome class before experimental mode is described
   as meeting any latency target.
7. **CLI/dev integration (manual/platform where needed):** `auro dev` on a hot-reload-capable demo
   project performs a view-only reload without closing the window; Ctrl-C and natural window-close
   semantics remain correct (spec 0001 acceptance 3/4). Platform limitations are recorded, not hidden.
8. **Documentation and gates:** `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass. CI also runs the
   `hot-reload` feature tests and builds/tests a generated scaffold on Linux, macOS and Windows, so
   template drift is caught on the platforms without a live hot-reload measurement. Spec 0001 open
   question, roadmap, performance budget, changelog, and architecture notes are updated to refer to
   this spec and the measured status.

**Status (2026-11-18, integrated hot mode, four-crate scaffold):** item 2 (a boundary build failure
keeps the previous UI) is demonstrated on Linux and Windows; a failed host rebuild is reported
separately and leaves the previous app running until the next successful save. Item 3's apply-failure path is contained by
`apply_patch` error/panic handling (unit-tested selection, manual rejection of an incompatible frame
observed). Item 5 is demonstrated: a save outside `data/src` restarts the devserver and app,
restores the snapshot and is labelled `restarted (state restored)`. Item 6 is demonstrated
on Linux (N=12, median 368 ms, p95 394, min 325, max 394, patch-to-render 1-26 ms), Windows (N=9, median
813 ms, p95 897, 752-897, patch-to-render 2-16 ms) and macOS (two runs, N=12 after warm-up, medians 509.5, 837.0 and
956.0 ms, p95 1071/2096/1563, 372-2096,
patch-to-render 2-17 ms), each recorded with machine and method in D6. Item 7 is partially
demonstrated (same window, pid and host state across patches on Linux, Windows and macOS; host
restart with state restored on Linux, Windows and macOS CI; graceful exit and Ctrl-C cleanup on
Linux and Windows) for boundary-only edits. Item 1 has a headless
fixture for mounting the view, changing the boundary output and keeping state
(`crates/auro/tests/hot_fixture.rs`); it does not load a patch dylib, and the snapshot roundtrip is
covered by the `auro-core` tests. Item 4 rests on the crate boundary (`ui` does not depend on `data`, only
`hot`-wrapped calls read the jump table); item 8 is the repository gates. The Linux median is now
under 500 ms for the documented warm case, macOS is above the target on a shared CI runner and
Windows remains above it, so the target is not met platform-wide and experimental mode must not
claim it.

GA additionally requires: measured < 500 ms save-to-visible for the documented warm view-only case on
each Tier 1 platform (or an explicit, approved revision of the target recorded in the performance
budget), published numbers, and a stable documented author contract for reloadable state.

## Open questions

| Question | Where it settles |
|---|---|
| Reload mechanism: in-process/function-level patching feasibility (e.g. subsecond-style) on Linux x86_64 with GPUI 0.2.2, and alternatives if not viable | **Partially evidenced (prototype, 2026-11-16):** subsecond/dx tip-crate `HotFn` patching viable in minimal GPUI 0.2.2 cases with constraints; on Windows 11 x86_64 a plain (non-GPUI) prototype applied 3/3 warm patches (484/504/499 ms) with state continuity and a second plain prototype showed view edits at 590/523 ms with host state continuous and an init-captured backend change applying only after restart; GPUI 0.2.2 built and rendered on Windows in the interactive console session; there, a patch of an element-returning view applies then overflows the main thread's stack, while a leaf `HotFn` and a framework-free `ViewData` boundary hot-patched repeatedly with state preserved (UI values reloaded in 1324/1084 ms). Productization, representative D6, and other platforms remain open. Mechanism recorded here; ADR revision/addition only if decision changes. |
| Exact scaffold/layout/build flags for hot-reload-capable projects and migration from Phase 0 layout | Implementation of this spec; recorded here when chosen |
| Canonical snapshot encoding (JSON, CBOR, postcard/bincode-class) and compatibility policy | Implementation; must remain `serde`-compatible at the API boundary |
| How managed `State<T>` participates when its type is defined in reloadable code vs host-stable code | Implementation + follow-up ADR/spec if it changes state ownership rules (D5 of spec 0001) |
| Debug/release mixing, optimization levels, and symbol/debug info required by the chosen mechanism | Implementation, per platform notes |
| In-app surfacing of build/reload diagnostics (overlay/panel) vs terminal-only in experimental mode | `auro-ui`/dev UX follow-up; terminal reporting is the minimum |
| Windows and macOS feasibility, code-signing implications, and debugger behavior for the chosen mechanism | **Partially evidenced on Windows x86_64 (2026-11-16, WinBoat guest):** plain non-GPUI tip-crate `HotFn` patching applied 3/3 warm patches with continuity; a second plain prototype showed frontend view edits (590/523 ms) with host state continuous and an init-captured backend interval applying only after restart; GPUI 0.2.2 built and rendered in the interactive console session, but `dx serve` from session 0 hit DirectX `0x887A0022` before render, and patching an element-returning view in session 1 applied then overflowed the main thread's stack (`0xc000041d`); a leaf `HotFn` and a framework-free `ViewData` boundary hot-patched repeatedly with state preserved, so Windows hot reload is limited to framework-free boundaries. macOS, signing/entitlements, debugger behavior, and representative D6 remain unverified. **Integrated re-test (2026-11-18):** the Windows scaffold applied boundary patches in the interactive session with the same pid and continuing state (median 1375 ms save-to-render, N=5 steady; see D1), kept the previous UI on a type error and exited 0 when the window closed. **macOS measurement (2026-11-18):** a manual GitHub `macos-26-arm64` workflow measured medians 509.5 and 837.0 ms on two six-warmup runs (N=12 each, 372-2096); a host restart with snapshot restore passed; local hardware and hardened-runtime signing remain open. |
| Change classification for `types`/`ui`/bin edits in hot mode: `dx` still compiles a patch, the running host keeps its old code, and the schema guard rejects an incompatible data frame; a restart is the honest path | **Implemented (2026-11-18):** the dev loop classifies `.rs`/`.toml` saves, restarts the devserver and app for host-code changes, restores the snapshot, labels `restarted (state restored|reset)`, and ignores editor swap files. |
