# Hot reload

This is the existential bet of the project. If the edit loop is a 20-second `cargo build`, nobody
leaves the web, and the rest of Auro does not matter.

> Target: edit to see in **under 500 ms**, with app state preserved.

Normative dev-loop contract, staging and acceptance are drafted in
[spec 0004](../specs/0004-hot-reload.md).
## Mechanism

```
editor saves
    |
    v
cargo build -p app-ui --crate-type cdylib   (incremental, background)
    |
    v
host loads new library (libloading), swaps the render entry point
    |
    v
state snapshot (serde) is handed to the new library
    |
    v
window repaints with preserved state
```

1. **UI as `cdylib`.** The app's views compile to a dynamic library, separate from the host
   process. The host owns the window and the event loop.
2. **`libloading` swap.** On a successful build, the host unloads the previous library and loads
   the new one. Failed builds keep the previous library live and surface the compiler error in the
   app.
3. **State survives the swap.** App state lives in the host. The new library receives a `serde`
   snapshot and reconstructs its entities from it. State that cannot be serialised is reset, and
   that must be visible in documentation rather than a silent surprise.

## Function-level patching

Whole-library swaps pay relocation and re-layout cost on every save. The roadmap targets
function-level patching along the lines of Zed's `subsecond` crate: patch only the functions whose
code changed, leaving the rest of the process untouched.

Status: research. It is an optimisation on top of the swap, not a prerequisite for it.

## Fallback

If the sub-500 ms target is missed, the fallback is a **process restart with state restoration**:
the host exits, the new binary starts, and the app restores from the last snapshot. Slower, but it
still preserves context, and it is honest about what is happening.

The fallback exists so the project is not blocked by a research result. It must not be presented
as hot reload.

## Constraints this imposes on application code

- Views must be reconstructible from state. Anything built ad hoc inside a render function is lost
  on reload.
- Entity handles must not be stored in `static`s that outlive the library. A stale handle into an
  unloaded `cdylib` is a use-after-free.
- Long-lived tasks that capture view code must be cancellable on swap, or they will call into the
  old library.

## Open problems

- ABI stability across compiler versions and across the host/library boundary.
- Whether the snapshot format is versioned per app, and what happens when it does not match.
- Debugging a swapped library: line tables, breakpoints and backtraces through `libloading`.
- Linux `dlclose` semantics when the old library still has live threads.
