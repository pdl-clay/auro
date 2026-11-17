# 0005 - Toolkit seam

**Status:** Implemented (2026-11-17)

[ADR 0009](../decisions/0009-toolkits-are-app-level.md) fixes the product decision: Auro is the
application layer and ships no component toolkit. This spec defines the small seam that makes the
decision usable: how an app initializes the toolkit it chose and how it puts that toolkit's root
around Auro's view.

## Scope

- `App::setup`: a hook that runs after GPUI starts and before any window opens.
- `App::root_with`: a way for the app to build the window's root view instead of `V::default()`.
- The default `App::root` stays as the `Default`-only sugar.
- The config handoff that lets an app apply `app.windows[].theme` to its toolkit.
- Documentation of one tested integration (`gpui-component 0.5.x` with GPUI 0.2.2).

## Out of scope

- Any component toolkit, component implementation, component tests or toolkit compatibility
  guarantee beyond the documented line.
- Auro-owned themes or a theme system.
- Per-window toolkit theming, dialogs, notifications, focus management: the toolkit owns them.
- Loading a toolkit through a plugin; this is a normal application dependency.

## Decisions

### D1 - The framework owns no toolkit

`auro-ui` remains the GPUI seam: re-exports, tokens, and the primitives the scaffold needs. A
toolkit is an app dependency (`gpui-component`, a private crate, or none). See
[ADR 0009](../decisions/0009-toolkits-are-app-level.md).

### D2 - Setup runs once, before windows

`App::setup` registers a closure invoked once per app, in registration order, after the GPUI
application starts and before any configured window opens. It receives the parsed `Config` so the
app can choose a toolkit theme from the same file that declares the windows.

Setup APIs have no error channel. A panic in setup is an application bug and aborts the process the
same way a panic in a view would; Auro does not hide it.

### D3 - The app can own the window root

`App::root_with` takes a builder that returns `Entity<V>` for the window. It exists because some
toolkits must be the window root (they host dialogs, notifications, sheets and input focus above
the app's view). The builder runs inside the GPUI window callback, after the window is created and
its title is set.

`App::root::<V>` is sugar for a builder that returns `cx.new(|_| V::default())`. Both paths share
the same window creation and title handling; a change to one applies to the other.

### D4 - Windows keep their existing contract

Every window declared in `Auro.toml` is opened once, in declaration order, using the root
registered for its label. A window with no matching root logs an error and does not open. Labels
remain unique. Setup does not change the window list.

### D5 - Config is the only handoff

The app receives the same `Config` that declares its windows. `app.windows[].theme` is parsed and
passed through; the app maps it to its toolkit in `setup`. `auro doctor` reports a non-system theme
as toolkit-owned instead of claiming the renderer ignores it.

Per-window themes depend on the toolkit. A toolkit with one global theme (as `gpui-component` has)
can honor one window's theme, not a different theme per window; the app must decide and document
that. Auro does not pretend otherwise.

### D6 - One tested integration, versioned honestly

The documented integration is `gpui-component 0.5.x` with the pinned `gpui = 0.2.2`. The current
component line (0.10.x) requires `gpui-pre = 0.3.12`; using it requires moving the renderer pin and
is not part of this spec. Measured probe (2026-11-17, Linux x86_64, release, stripped,
`panic = "abort"`, one Button + Root): 22.0 MB with the toolkit against 15.5 MB without it, and
spawn→window 147.3 ms against 139.8 ms (median, N=5; not first paint). The app pays the toolkit's
binary size; Auro's minimal app does not.

## Interfaces

```rust,ignore
impl App {
    /// Runs after GPUI starts and before any window opens; registration order.
    pub fn setup(self, f: impl Fn(&mut gpui::App, &Config) + 'static) -> Self;

    /// Builds the window root instead of `V::default()`.
    pub fn root_with<V: Render + 'static>(
        self,
        label: &'static str,
        f: impl Fn(&mut Window, &mut gpui::App) -> Entity<V> + 'static,
    ) -> Self;

    /// Unchanged sugar for a `Default` view.
    pub fn root<V: Render + Default + 'static>(self, label: &'static str) -> Self;
}
```

Order inside `Application::run`:

```
setups (registration order) -> configured windows, in order:
    window created, title set -> root builder -> window mapped
-> on_window_closed -> activate
```

A toolkit integration looks like this (illustrative, not compiled in this repository):

```toml
# app Cargo.toml - not Auro's
[package]
name = "log-viewer"

[dependencies]
auro = { path = "../auro/crates/auro" }
gpui-component = "=0.5.1"
```

```rust,ignore
use auro::prelude::*;
use gpui_component::{Root, Theme as KitTheme, ThemeMode};

fn main() {
    App::new()
        .setup(|cx, config| {
            gpui_component::init(cx);
            let mode = match config.app.windows[0].theme {
                auro::Theme::Dark => ThemeMode::Dark,
                auro::Theme::Light => ThemeMode::Light,
                auro::Theme::System => ThemeMode::Light, // or follow the system appearance
            };
            KitTheme::change(mode, None, cx);
        })
        .root_with::<Root>("main", |window, cx| {
            let view = cx.new(|_| LogViewer::default());
            cx.new(|cx| Root::new(view, window, cx))
        })
        .run();
}
```

The app adds the toolkit, not Auro. If the toolkit does not need a custom root, `root::<V>` is still
enough and no wrapper is required.

## Acceptance

1. A setup closure runs once, in registration order, before any root builder; a headless test
   records the order (`app::tests::setup_runs_before_roots_and_root_with_uses_the_app_builder`).
2. `root_with` opens every declared window that has a root and uses the app's builder; the same
   test asserts one window opens and the builder ran.
3. `root` keeps its existing behavior; the window-title and window-options tests cover the shared
   opener.
4. `Config` reaching `setup` includes the declared theme; `auro doctor` reports a non-system theme
   as toolkit-owned.
5. `docs/src/architecture/toolkits.md` documents choosing a toolkit, the `0.5.x`/`0.10.x` split,
   and the measured size caveat; `mdbook build docs` passes.
6. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
   `cargo test --workspace --locked` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Whether to publish an optional `auro-gpui-component` integration crate | After the next GPUI line migration; ADR 0009 keeps the core toolkit-free meanwhile |
| How per-window themes map onto a toolkit with a global theme | App-level decision documented by the app; Auro passes the config through |
| Whether an optional toolkit integration should provide a shared theme bridge | Phase 3, if more than one toolkit needs it |
