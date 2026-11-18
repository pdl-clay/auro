# Introduction

Auro is an application framework for native Rust desktop apps. It keeps the parts of
[Tauri](https://tauri.app) that are tedious to rebuild - configuration, capabilities, plugins,
bundling, signed updates, dev server, single-instance, multi-window - and replaces the renderer
with [GPUI](https://gpui.rs), Zed's GPU-accelerated Rust UI toolkit.

The frontend is Rust. There is no HTML, no CSS engine and no JavaScript in the app runtime.

> **Status: pre-alpha.** The workspace exists, Phase 0 and Phase 1 are complete, and Phase 2 is in
> progress, but nothing is published on crates.io and there is no released CLI. This book documents
> the architecture and the contracts the code is being written against; it is reference material,
> not end-user release documentation.

## Where to start

| If you want to | Read |
|---|---|
| Understand the project in five minutes | [README](https://github.com/pdl-clay/auro#readme) |
| Know what ships when | [ROADMAP](https://github.com/pdl-clay/auro/blob/main/ROADMAP.md) |
| Understand how the pieces fit | [Architecture overview](architecture/overview.md) |
| Know why a choice was made | [Decision records](decisions/README.md) |
| Contribute | [CONTRIBUTING](https://github.com/pdl-clay/auro/blob/main/CONTRIBUTING.md) |
| Propose a change to the design | [RFC process](https://github.com/pdl-clay/auro/blob/main/rfcs/README.md) |

## The three decisions that define the project

1. **Typed IPC, everywhere.** Schemas are derived from Rust signatures. There is no
   stringly-typed bridge, because there is no JavaScript to bridge to.
2. **Hot reload or bust.** Rust compile times are the only real reason to stay on the web.
   Target: edit to see in under 500 ms, with app state preserved. The opt-in hot mode measured
   368 ms median on Linux for the documented case; Windows and macOS remain above the target.
3. **Capabilities for the real boundary.** With a Rust frontend, most permission checking is
   compile-time. What needs a runtime fence is plugin code and untrusted content.

Each is recorded as an accepted decision, with its consequences and its costs:
[0002](decisions/0002-rust-only-frontend.md), [0005](decisions/0005-hot-reload-as-the-bet.md),
[0003](decisions/0003-capabilities-at-the-real-boundary.md).

## What Auro is not

- **Not a webview wrapper.** A webview exists only as an explicitly scoped escape hatch for
  OAuth and embedded docs; it is not the foundation.
- **Not a GUI toolkit.** That is GPUI. Auro is an *application* framework.
- **Not a GPUI fork.** We track upstream and keep a thin compatibility layer.
- **Not "CSS for Rust".** There will be no styling language. If you want CSS, use Tauri.
