# GPUI 0.3 spike (2026-11-18)

This is a disposable spike branch, not a migration. It answers one question: what breaks if Auro
moves from `gpui = "=0.2.2"` to the `gpui-pre 0.3.x` line, and is the hot-reload path still viable?

## Environment

- CachyOS Linux 7.3.4-1, x86_64, 12 CPUs, 22 GiB RAM.
- niri/Wayland (`WAYLAND_DISPLAY=wayland-1`, `DISPLAY=:0`).
- rustc/cargo 1.99.1, dx 0.7.10.
- New dependency set: `gpui-pre =0.3.12` (aliased as `gpui`), `gpui-pre-linux =0.3.12`,
  `gpui-component =0.10.0`, `accesskit 0.24.1`.
- `gpui-pre 0.3.13` exists, but `gpui-component 0.10.0` pins `gpui-pre =0.3.12`. The spike pins the
  whole line to `0.3.12`; mixing 0.3.12 and 0.3.13 would create two `gpui` type universes.
- The `xattr 0.2.3` vendor patch is unused by this dependency graph; `xattr 1.7.0` is in the tree.

## Results

| Target | Command | Result |
|---|---|---|
| `auro-ui` | `cargo check -p auro-ui` | pass after 34.8 s of dependency build |
| workspace | `cargo check --workspace` | pass after 2 API edits |
| `examples/hello` | `cargo check` | pass |
| `examples/log-viewer` | `cargo check` with `gpui-component =0.10.0` | pass without source changes |
| generated app | `auro new` + `cargo check --features hot-reload` | pass after replacing `gpui::Timer` |
| hot reload | `auro dev`, `[dev] hot = true` | pass; 20 boundary edits, same pid |

The first app build through `dx` took 72.41 s (shared dependency target, cold app crate). A host
rebuild after a `types`/`ui` edit took 3.09 s.

## Broken APIs

Only two workspace call sites needed changes; the generated template needed one more.

1. `Application::new()` is gone. `gpui-pre` now takes a platform: `Application::with_platform(Rc<dyn
   Platform>)` or `Application::new_inaccessible(...)`. The platform implementation lives in a
   separate crate (`gpui-pre-linux`, `gpui-pre-macos`, `gpui-pre-windows`), and the app chooses it
   with `current_platform(false)`/`linux_platform(...)`. This is an architectural change for Auro:
   `auro-ui` or `auro` must own the target-specific platform dependency, and `auro new`/CI must
   exercise each platform crate. The spike hardcodes the Linux platform.
2. `cx.on_window_closed` now takes `(cx, WindowId)` instead of `cx`. Auro ignores the id and
   recomputes the open-window set, so the change is mechanical.
3. `gpui::Timer` is no longer re-exported (0.2.2 re-exported `smol::Timer`). The scaffold used it in
   the snapshot debounce; the spike adds `smol = "2"` to the `ui` crate and imports `smol::Timer`.
   This is a template change, not a framework API change.

`gpui-component 0.10.0` compiled the existing `log-viewer` source without changes. That is a
compile result, not a runtime or visual verification.

## Hot reload

The existing `auro::hot` data boundary (`view_data() -> HotData`, subsecond/dx 0.7.10) works with
`gpui-pre 0.3.12`. A first edit measured **314 ms** save-to-render (patch 272 ms, patch-to-render
0 ms). The app then received a host-only edit (`ui`/`types`) and was restarted by `auro dev`
(`host rebuild failed; previous UI still running` while the files were inconsistent; then a
successful restart).

For a real layout/tree test, the host was taught to interpret four boundary values (`padding`,
`gap`, `radius`, `tree`) and to add/remove children from `tree`. This host change is a restart. The
running host (pid 2413989) then received 20 boundary edits that changed the title, accent, padding,
gap, radius and tree (0/1/2 extra children):

| N | median | min | max | patch-to-render |
|---|---|---|---|---|
| 20 | **296 ms** | 285 ms | 415 ms | 0-1 ms |

All 20 edits stayed in the same pid, the host `renders` counter advanced 5→24, and no relaunch
occurred. The `ViewData` schema was bumped to 2 and the old host rejected the incompatible frame
during the transition, which is the intended guard.

This is **not** element-code hot reload. The boundary carries layout values; `ui`/`Render` code
still lives in the host and a change there restarts the process. It does show that a data-driven
layout/tree can change in the same process well under the 1 s experimental target.

## Platforms and accessibility

- Only Linux x86_64 was built and run. `gpui-pre-macos` and `gpui-pre-windows` exist, but the
  spike has no Apple or Windows host and did not cross-compile the full GUI stack.
- Windows hot reload of element-returning view code was already blocked on 0.2.2 (main-thread
  stack overflow); this spike did not retest it. The data-boundary design remains the portable
  shape.
- `gpui-pre` has an optional `hot-reload` feature that depends on `subsecond 0.7.10` and
  `dioxus-devtools-types`; it was not enabled or tested. It may be a better migration path than
  Auro's own `auro::hot` boundary, but that needs its own spike.
- AccessKit is a dependency (`accesskit 0.24.1`, `accesskit_unix 0.22.1`). A dependency does not
  prove functional accessibility: no screen reader, AT-SPI tree inspection, keyboard traversal or
  IME test was run. `Application::new_inaccessible` exists, but the default path was not audited.
- `gpui-component 0.10.0` depends on `gpui-pre =0.3.12`; a future 0.3.13 pin needs a matching
  `gpui-component` release or a patch.

## Recommendation

**Do not migrate `main` yet.** The migration is smaller than the 0.2.2 feasibility spike suggested
(two workspace call sites, one template dependency), but the platform split, the AccessKit/IME
audit, the Windows runtime and the hot-reload mechanism choice are still open. If Auro decides to
migrate:

1. Pin `gpui-pre =0.3.12`, `gpui-pre-{linux,macos,windows} =0.3.12`, and
   `gpui-component =0.10.0` together; do not mix 0.3.12 and 0.3.13.
2. Move platform selection into `auro-ui` (the only crate that names GPUI) and add a per-target CI
   check for the platform crates.
3. Change `Application::new` to the platform factory and `on_window_closed` to the two-argument
   callback in the same PR.
4. Replace `gpui::Timer` in the template with `smol::Timer` (or a deliberate `auro-ui` re-export)
   and add `smol` to the generated `ui` crate.
5. Re-run the hot-reload boundary benchmark and the `examples/hello`/`log-viewer` runtime, then
   decide whether to keep `auro::hot` or evaluate `gpui-pre`'s own `hot-reload` feature.
6. Drop the unused `xattr` patch only after the dependency graph proves it is unnecessary.

## Evidence

- Worktree: `/run/media/pdl/data/Projetos/auro-wt-gpui03` (branch `agent/gpui-0.3-spike`).
- Build logs: `gpui03-check.log`, `gpui03-workspace.log` in that worktree.
- Hot-reload log: `/tmp/auro-gpui03-dev.log`; generated app: `/tmp/auro-gpui03-demo`.
- The layout experiment is a throwaway app; it is not a committed example.
