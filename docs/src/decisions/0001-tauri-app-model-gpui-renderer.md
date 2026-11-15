# 0001 - Tauri's app model, GPUI's renderer

**Status:** Accepted

## Context

Desktop apps in Rust face a choice between two mature, wrong-shaped options. Electron packs a
browser: ~180 MB idle, ~900 ms cold start, ~120 MB binary. Tauri keeps an excellent application
model - config, capabilities, plugins, bundling for seven package formats, signed updater, dev
server, single-instance, multi-window - but renders the app as a web page in the system webview.

GPUI is Zed's GPU-accelerated Rust UI toolkit: pure Rust, a builder API, no DOM, no GC, no CSS
engine. It arrived solved for text, IME, selection and font rendering, because it was a text
editor first.

The parts of Tauri worth keeping are boring and expensive to rebuild. The renderer is the part
worth replacing.

## Decision

Auro is **Tauri's application shell with GPUI as the renderer**. The app model, CLI verbs,
config shape and packaging surface follow Tauri; the UI layer is GPUI, and the frontend language
is Rust.

## Consequences

**Easier**

- The hard, unglamorous work (capabilities, packaging, updater, dev server) has a proven design to
  follow instead of being invented.
- Text, IME and selection come from an editor, not from a browser.
- There is no JS runtime, no bridge, and no second language in the app.

**Harder**

- GPUI is not a stable public library the way a browser is. Upstream churn is a real, ongoing cost
  and the reason for the thin compatibility layer.
- GPUI is most mature on macOS; Windows and Linux parity is the largest schedule risk.
- Anything the web platform gives for free (devtools, CSS, a component ecosystem) has to be built
  or explicitly declined.

**Forbidden**

- Forking GPUI. A fork dies by merge conflict; patches must stay vendored and attributable.
- Treating the webview as the app's foundation. It is a scoped escape hatch, added in Phase 4.
