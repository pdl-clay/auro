# Summary

[Introduction](introduction.md)

# Architecture

- [Overview](architecture/overview.md)
- [Crates and layering](architecture/crates.md)
- [Threading and IPC](architecture/threading-and-ipc.md)
- [Capabilities](architecture/capabilities.md)
- [Choosing a toolkit](architecture/toolkits.md)
- [Hot reload](architecture/hot-reload.md)

# Decisions

- [Decision records](decisions/README.md)
- [0001 - Tauri's app model, GPUI's renderer](decisions/0001-tauri-app-model-gpui-renderer.md)
- [0002 - Rust-only frontend and typed IPC](decisions/0002-rust-only-frontend.md)
- [0003 - Capabilities at the real boundary](decisions/0003-capabilities-at-the-real-boundary.md)
- [0004 - Main thread belongs to GPUI](decisions/0004-main-thread-ownership.md)
- [0005 - Hot reload with state preservation](decisions/0005-hot-reload-as-the-bet.md)
- [0006 - Dual MIT/Apache-2.0](decisions/0006-dual-license.md)
- [0007 - No styling language in 1.0](decisions/0007-no-styling-language.md)
- [0008 - Hot reload mechanism revised after feasibility spike](decisions/0008-hot-reload-mechanism-revision.md)
- [0009 - Toolkits are the application's choice](decisions/0009-toolkits-are-app-level.md)
- [0010 - Typed events, delivered on the main thread](decisions/0010-typed-events-main-thread.md)
- [0011 - Bundling delegates to tauri-bundler](decisions/0011-bundling-delegates.md)
- [0012 - Release trust](decisions/0012-release-trust.md)

# Specifications

- [How specs work](specs/README.md)
- [What Auro takes from Tauri](specs/tauri-basis.md)
- [0001 - Foundation](specs/0001-foundation.md)
- [0002 - Configuration](specs/0002-config.md)
- [0004 - Hot reload](specs/0004-hot-reload.md)
- [0005 - Toolkit seam](specs/0005-toolkit-seam.md)
- [0006 - Lifecycle events](specs/0006-lifecycle-events.md)
- [0007 - Typed application events](specs/0007-events.md)
- [0008 - Command schema registry](specs/0008-command-schema.md)
- [0009 - Window manager](specs/0009-window-manager.md)
- [0010 - Bundling](specs/0010-bundling.md)
- [0011 - Signing](specs/0011-signing.md)
- [0012 - Update manifests](specs/0012-update-manifests.md)

# Reference

- [Platform support](reference/platform-support.md)
- [Performance budget](reference/performance-budget.md)
- [Repository standards](reference/repository-standards.md)
