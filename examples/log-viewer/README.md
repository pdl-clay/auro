# Log viewer

The Phase 1 reference app: a log viewer plus a settings window, built with Auro and an
application-chosen toolkit. It is not a workspace member and is not built by the main CI matrix; it
is the manual exit proof for the phase.

```bash
cd examples/log-viewer
cargo run
```

## What it demonstrates

- `Auro.toml` declares two windows (`main`, `settings`) and both are rooted in `main.rs`.
- `App::setup` initializes `gpui-component 0.5.1` and applies the theme from `Auro.toml`.
- `App::root_with` wraps both views in the toolkit's `Root` (dialogs/notifications/focus).
- A background thread emits typed `LogLine` events through `EventSender`; `App::on_event` appends
  them to a GPUI entity on the main thread.
- `auro::windows::open` and `windows::close` operate the settings window by its configured label.
- `LifecycleEvent::Exiting` stops the generator; `WindowClosed`/`Exiting` come from the close
  observer.
- The filter input, pause/clear buttons, level colors and the dark-theme switch use the toolkit;
  Auro provides the app model, not the widgets.

The toolkit pin is deliberate: `gpui-component 0.5.1` is the last line compatible with the pinned
`gpui 0.2.2`. The current component line requires `gpui-pre 0.3.12`; see
[ADR 0009](../../docs/src/decisions/0009-toolkits-are-app-level.md) and
[choosing a toolkit](../../docs/src/architecture/toolkits.md).

The stripped release binary for this app measured ~22 MB with the toolkit vs ~15.5 MB for the
minimal Auro probe (Linux x86_64, 2026-11-17; see the document above). The framework does not pay
that size for apps that choose no toolkit.

## Verified

On 2026-11-17 (niri/Wayland, Linux x86_64):

- both windows opened with the expected titles and app id;
- the main window rendered live log lines with levels and the counter, and the settings window
  rendered its switches and close button (screenshots from the identical build);
- closing both windows exited the app;
- the window manager's close/reopen-by-label path is covered by headless tests and by a separate
  niri two-window probe, not by the example's own Settings button.
