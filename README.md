# Auro

[![Stars](https://img.shields.io/github/stars/pdl-clay/auro?style=flat-square&label=stars)](https://github.com/pdl-clay/auro/stargazers)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square)](#license)
[![Status](https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square)](#status)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square)](#status)

**English** · [Português](README.pt-BR.md)

**Tauri's app model. GPUI's renderer. Zero webview.**

Auro is an application framework for Rust desktop apps. It takes the parts of Tauri that are
tedious to rebuild - configuration, capabilities, plugins, bundler, signed updater, dev server,
single-instance, multi-window - and swaps the renderer: instead of a webview, the UI is
GPUI, Zed's GPU-accelerated Rust UI toolkit.

The frontend is Rust. There is no HTML, no CSS engine and no JavaScript in the app runtime.

---

## Status

**Pre-alpha. Designing in public.** The workspace exists, Phase 0 and Phase 1 are complete, and
Phase 2 is in progress. `0.1.0-alpha.3` is the current published alpha on crates.io; there is no
stable release or stable CLI yet. Install the alpha CLI with
`cargo install auro-cli --version 0.1.0-alpha.3`, or build and try the framework from a checkout
(see [CONTRIBUTING.md](CONTRIBUTING.md)). The Phase 2
slices in tree cover bundling, signed updates with platform apply paths and platform services;
macOS notarization submission and local verification are release tooling, not a tested end-user
flow. Hot reload is measured, not GA, and only for the opt-in four-crate scaffold's data boundary:
on Linux, edits to `data/src` (the `view_data() -> HotData` boundary) are under the 500 ms target,
above it on Windows, and above it on a shared macOS runner. Edits to `ui`/`Render`, layout code,
`types`, dependencies or configuration rebuild the host and restart the process (state restored
from a snapshot when compatible); they are not in-process view reloads.

| Phase | Codename | State |
|---|---|---|
| 0 | Foundations | **complete** |
| 1 | Hello, native | **complete** |
| 2 | Ship it | **in progress** |
| 3 | Extensible | planned |
| 4 | Ecosystem | planned |
| 5 | 1.0 | planned |

Full detail in [ROADMAP.md](ROADMAP.md). Architecture and the reasoning behind it live in the
[documentation book](https://pdl-clay.github.io/auro/); platform support is
[published as a matrix](docs/src/reference/platform-support.md) from day one.

## Why

Electron and Tauri both ship a browser to draw a button.

| | Electron | Tauri | Auro |
|---|---|---|---|
| App model | minimal | **excellent** | Tauri's |
| Renderer | Chromium | system webview | **GPU, native** |
| Frontend language | JS/TS | JS/TS | **Rust** |
| Idle RAM (hello world, measured) | 291 MB | 315 MB | **99 MB** |
| Cold start (content-ready, measured) | 421 ms | 417 ms | **190 ms** |
| Payload (uncompressed, measured) | 274 MB | **6.7 MB** | 14.9 MB |
| Text/IME/selection | browser | browser | inherited from an editor |

Measured on Linux/Wayland (niri), 2026-11-19: one 480x320 window with one label, process-tree PSS
after 5 s idle, spawn to content-ready (not a painted frame), uncompressed payload. Electron's
payload includes Chromium; Auro and Tauri use the system webview/GPU stack. Method and raw samples:
[performance budget](docs/src/reference/performance-budget.md) and
[`tools/comparison/baseline.json`](tools/comparison/baseline.json).

Tauri got the hard, boring part right: capabilities, a plugin system, bundling for seven package
formats, a signed updater. What it gets wrong, for people who want *native*, is that the app is
still a web page.

Auro keeps the shell and replaces the engine.

## What it will look like

```rust
use auro::prelude::*;

#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> {
    tokio::fs::write(&path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Button::new("save", "Save").on_click(cx.listener(
                |this, _, _window, cx| {
                    let doc = this.document.clone();
                    cx.spawn(async move |this, cx| {
                        let saved = api::save_document(doc.path, doc.body).await?;
                        this.update(cx, |this, cx| {
                            this.status = format!("Saved {} bytes", saved.bytes);
                            cx.notify();
                        });
                        Ok::<(), AuroError>(())
                    })
                    .detach();
                },
            )))
            .child(self.status.clone())
    }
}

fn main() {
    auro::App::new()
        .root::<Editor>("main")
        .run();
}
```

No `invoke("save_document", {...})`. The command is a Rust function; the call site is a Rust
call. Type errors are compile errors.

## The three decisions that define the project

1. **Typed IPC, everywhere.** Schemas are derived from Rust signatures. There is no
   stringly-typed bridge, because there is no JavaScript to bridge to.
2. **Hot reload or bust.** Rust compile times are the only real reason to stay on the web.
   Target: edit to see in **under 500 ms**, with app state preserved. The whole project rests on
   this bet.
3. **Capabilities for the real boundary.** With a Rust frontend, most permission checking is
   compile-time. What actually needs a runtime fence is **plugin code** and **untrusted
   content**. That is where Auro's capability model lives.

## What Auro is not

- **Not a webview wrapper.** A webview exists only as an explicitly scoped escape hatch for
  OAuth and embedded docs; it is not the foundation.
- **Not a GUI toolkit.** That is GPUI. Auro is an *application* framework.
- **Not a component library.** Auro ships the application layer; use GPUI directly or pick a
  toolkit such as `gpui-component`. `App::setup` and `App::root_with` are the seam; the
  framework does not bundle widgets.
- **Not a GPUI fork.** We track upstream and keep a thin compatibility layer.
- **Not "CSS for Rust".** There will be no styling language. If you want CSS, use Tauri.

## Performance we are building to

| Metric | Target at 1.0 | Measured (2026-11-18) |
|---|---|---|
| Cold start | < 80 ms | 154.9 ms median, first-frame request (Linux) |
| Idle RAM | < 40 MB | 105.3 MB (Linux) |
| Hello-world binary | < 6 MB (stripped) | 15.6 MB (Linux) |
| Edit to see (boundary data only) | < 500 ms | 368 ms median, p95 394 ms for edits in `data/src` (`view_data() -> HotData`), opt-in hot mode, N=12 (Linux); 813 ms median, p95 897 ms (Windows); 509.5-956.0 ms medians, p95 up to 2096 ms, three six-warmup runs (shared macOS CI runner) |
| Platforms | macOS 12+, Windows 10+, Ubuntu 22.04+ (Wayland & X11) | Linux runtime verified locally; macOS and Windows build and test in CI |

These are alpha measurements, not a 1.0 baseline, and the CI regression gate is not implemented
yet: the harness exists and the method is documented in the
[performance budget](docs/src/reference/performance-budget.md). The edit-to-see result covers only
the opt-in hot-reload path's data boundary with the four-crate scaffold; hot reload is measured,
not GA. A change to `ui`/`Render`, layout, `types`, dependencies or configuration is host code:
`auro dev` rebuilds and restarts the process, restores opted-in state from a snapshot when the
version is compatible, and labels the result `restarted (state restored)` or `restarted (state
reset: <reason>)`. It never reports that path as a reload. Per-platform status is in the
[support matrix](docs/src/reference/platform-support.md).

## Roadmap

Six phases, ~42 weeks to 1.0. Full detail in [ROADMAP.md](ROADMAP.md).

| Phase | Codename | Goal |
|---|---|---|
| 0 | Foundations | `auro new` to a window, in under two minutes |
| 1 | Hello, native | App layer + typed IPC + `auro dev`, toolkit of your choice |
| 2 | Ship it | Bundling, signed updates, platform services, a11y |
| 3 | Extensible | Out-of-process plugins, hot reload GA |
| 4 | Ecosystem | Web escape hatch, templates, docs |
| 5 | 1.0 | API freeze, audit, perf budget in CI |

## Get involved

Auro is designed in the open. Architecture and roadmap discussion live in
[Discussions](https://github.com/pdl-clay/auro/discussions). Open questions are listed at the
end of [ROADMAP.md](ROADMAP.md). See [CONTRIBUTING.md](CONTRIBUTING.md) to get started; report
security issues through [SECURITY.md](SECURITY.md), never a public issue.

Built on the shoulders of [Tauri](https://tauri.app) and [Zed's GPUI](https://zed.dev).

## License

Dual-licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Auro by you shall be dual-licensed as above, without any additional terms or conditions.
