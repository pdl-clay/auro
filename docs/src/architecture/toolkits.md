# Choosing a toolkit

Auro is the application layer. It owns configuration, windows, typed commands, state, lifecycle
and the dev loop; it does **not** ship a component kit. That is deliberate
([ADR 0009](../decisions/0009-toolkits-are-app-level.md)): GPUI is the renderer, widgets are the
renderer's or the ecosystem's, and the app chooses what it wants to pay for.

## The seam

Two builder calls make any GPUI toolkit usable:

- `App::setup` runs after GPUI starts and before any window opens. Initialize the toolkit and
  apply the theme from `Auro.toml` there.
- `App::root_with` builds the window root yourself, so a toolkit can own dialogs, notifications,
  sheets and input focus around your view. `App::root::<V>` remains the shortcut for a
  `Default`-constructed view with no wrapper.

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

```toml
# the app's Cargo.toml, not Auro's
[dependencies]
auro = { path = "../auro/crates/auro" }
gpui-component = "=0.5.1"
```

`App::setup` receives the parsed `Config`, so the theme decision uses the same file that declares
the windows. `app.windows[].theme` is passed through; the toolkit decides what it can honor. A
toolkit with one global theme can honor one theme per app, not a different theme per window.
`auro doctor` reports a non-system theme as toolkit-owned.


## Assets and `svg().path()`

GPUI elements that load bytes by path — most visibly `svg().path("icons/logo.svg")` — read them
from the GPUI asset source. Auro does not install one by default, so an app that uses file-backed
elements must call `App::assets` on the builder. The trait is GPUI's own, re-exported as
`auro::gpui::AssetSource`; Auro does not introduce a second trait.

```rust,ignore
App::new()
    .assets(auro::assets::FileAssets::new("assets"))
    .root::<View>("main")
    .run();
```

`FileAssets` scopes every lookup to the chosen directory: absolute paths, drive prefixes, `..` and
symlinks that resolve outside the root are rejected before the bytes are read. `EmbeddedAssets`
wraps a static `&[(&str, &[u8])]` table for apps that prefer `include_bytes!`; an app can also
implement `AssetSource` itself. The source is type-erased behind `Arc<dyn AssetSource>` in the
builder and applied to `Application::new().with_assets(...)` once, before the event loop starts.

## Versions are part of the choice

`gpui-component` tracks GPUI; Auro pins GPUI exactly. The compatibility line matters:

| Toolkit | GPUI line | Notes |
|---|---|---|
| `gpui-component 0.5.1` | `gpui ^0.2.2` | Last line compatible with Auro's pinned renderer (2026-02-05). The probe measured 0.5.0; 0.5.1 is the newest release on the same dependency line. |
| `gpui-component 0.6+` | `gpui-pre ^0.3.x` | Requires moving Auro's GPUI pin and revalidating hot reload. Current 0.10.0 pins `gpui-pre =0.3.12`. |
| none | `gpui 0.2.2` | `div()`, GPUI elements and Auro's minimal `Button`; smallest app. |

Measured probe (2026-11-17, Linux x86_64, release, stripped, `panic = "abort"`, one Button plus
the toolkit `Root`): **22.0 MB** with `gpui-component 0.5.0` against **15.5 MB** for the minimal
Auro app; spawn→window 147.3 ms against 139.8 ms (median, N=5; not first paint). The app pays the
toolkit's size; the framework's minimal app does not.

These numbers are context, not a benchmark contract. Re-measure when the toolkit or GPUI line
changes, and record the environment with the result.

The working integration is in [`examples/log-viewer`](../../examples/log-viewer/README.md): two
windows, a background event stream, and settings, all with the toolkit at the app level.

## What Auro will not do

- Add a toolkit to `auro-ui` or `auro-core`.
- Promise that any toolkit version works forever.
- Present its minimal `Button` as a component kit.
- Hide a toolkit's per-window limitations (for example, one global theme).

If a toolkit later reaches a stable 1.0 that can be pinned without moving GPUI, an optional
integration crate can wrap it. The core still ships no toolkit.
