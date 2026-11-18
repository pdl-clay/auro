# 0022 - Hot reload boundary is a separate crate with a fixed frame

**Status:** Accepted (2026-11-18)

## Context

[ADR 0008](0008-hot-reload-mechanism-revision.md) selected same-process function-level patching
with an explicit `HotFn` boundary and left the project layout open. The first integrated scaffold
put the boundary and the GPUI view in one bin crate. `dx` recompiles a changed crate and its
dependents, so every boundary edit recompiled and relinked the view, and the patch library carried
GPUI: the measured Linux median was 521 ms and each apply mapped an app-sized library.

A patch also has to agree with the host that is already running. A typed boundary returning a
locally defined struct is fine while the two layouts match, but a freshly compiled patch can return
a different layout while the old host reads the old one. That is undefined behaviour, not a
diagnostic.

## Decision

The generated app is a four-crate workspace:

| Crate | Contents | May depend on |
|---|---|---|
| `types` | `ViewData` and the fixed `HotData` frame | nothing |
| `data` | the hot boundary `view_data() -> HotData` | `types` |
| `ui` | `Render`, window, element tree, snapshot/autosave | `auro`, `types`, `serde` |
| bin | `main` calling `ui::run(data::view_data)` | `ui`, `data` |

`ui` never depends on `data`. A boundary edit therefore recompiles `data` and the thin bin, not
GPUI or the view. The dependency direction is a tested invariant, not a convention.

`HotData` is a fixed `#[repr(C)]` frame with a schema number, title length and accent. The host
checks schema and bounds before decoding; a changed schema is rejected and the previous values stay
on screen. Change `ViewData` and its encoder, bump `SCHEMA`, and never re-lay-out `HotData`.
`auro::hot::hot_frame`/`rendered` report a render only after the view accepts the frame.

`auro::hot` owns the devserver connection. `apply_patch` errors and loader panics are caught and
reported; the previous jump table keeps running and the listener stays open. A committed patch
wakes the main thread through a watch channel rather than a poll.

## Consequences

**Easier**

- Warm boundary edits are cheaper: the Linux median fell from 521 ms (N=11) to 368 ms (N=12,
  325-394), and the patch library from app-sized to ~12 KB. The process, window and host state
  continue.
- A mismatched data layout is a reported rejection instead of an out-of-bounds read.
- A failed apply is a message and a still-live app, not a dead devserver listener.
- A panic in patched boundary code is contained; the view keeps its previous values and the process
  survives.
- A save outside `data` restarts the devserver/app and restores the snapshot; it is labelled as a
  restart, never as a reload.

**Harder**

- The scaffold and migrations carry more crates, and adding a dependency from `ui` to `data` undoes
  the fast path; a scaffold test fails if it happens.
- `HotData` is an ABI: its shape is frozen and changes require a restart/version bump.
- Windows remains above the 500 ms target (813 ms median, N=9) because MSVC compile/link of the
  boundary and bin dominates; `incremental` and Defender exclusions did not close the gap. The
  relaunch loop remains the Windows fallback.
- macOS is measured on a shared CI runner at 509.5 ms median (N=12 after warm-up), at the target
  boundary; local hardware and signing remain unverified.

**Forbidden**

- Adding `ui -> data` or `data -> auro` dependencies in the generated layout.
- Changing `HotData`'s shape without treating it as an ABI change.
- Presenting a patch that the view rejected, or a relaunch, as a successful reload.

## References

- [ADR 0008 - Hot reload mechanism revised after feasibility spike](0008-hot-reload-mechanism-revision.md)
- [Spec 0004 - Hot reload](../specs/0004-hot-reload.md)
- [Hot reload architecture](../architecture/hot-reload.md)
