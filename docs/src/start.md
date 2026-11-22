# Start here

This gets a shell to a native Auro window with the published alpha CLI, then explains what the
default dev loop actually does. Auro is pre-alpha: use this to evaluate the shape of the framework,
not to ship a product.

## Before you start

You need:

- Rust stable with `cargo` on `PATH`. The scaffold uses edition 2024 and declares `rust-version =
  "1.90"`.
- The platform build dependencies. Linux needs the native libraries listed in
  [platform support](reference/platform-support.md#linux-system-dependencies); macOS and Windows
  need their normal native build toolchain.
- The alpha CLI. There is no stable release. The published version is
  `0.1.0-alpha.13`.

The CLI is a cargo-installed binary named `auro`; the crate is `auro-cli`.

## 1. Install the CLI

```bash
cargo install auro-cli --version 0.1.0-alpha.13
auro --version
```

If the second command does not run, check that `~/.cargo/bin` is on `PATH`. To build the CLI from
a checkout instead, follow [CONTRIBUTING](https://github.com/pdl-clay/auro/blob/main/CONTRIBUTING.md).

## 2. Create the app

```bash
auro new hello
cd hello
```

`auro new` refuses a non-empty directory. It writes a four-crate workspace plus configuration:

|Path|Role|
|---|---|
|`Auro.toml`|App identifier, windows, dev watch list, hot-mode switch and bundle icon.|
|`src/main.rs`|Thin binary: calls the UI crate with the hot boundary function.|
|`ui/`|`Render`, window, element tree and command functions. It does not depend on `data`.|
|`data/`|The hot boundary: `view_data() -> HotData`, deliberately tiny and framework-free.|
|`types/`|Shared `ViewData` and the fixed `HotData` frame; no framework dependency.|

The split is the hot-reload boundary: changing `data/src` rebuilds a small crate and relinks the
binary, while `ui` and GPUI stay compiled. That is why the boundary is separate, and why a change
outside it is not a view reload.

## 3. Run it

```bash
auro dev
```

The command builds the app, launches it as a native window, and watches the paths in `[dev] watch`.
A save rebuilds, relaunches and restores opted-in state from a snapshot when the snapshot version is
compatible. The previous app stays visible while a replacement builds.

This default loop is **not hot reload**. It is a build-and-restart loop, and the output says
`restarted (state restored)` or `restarted (state reset)` rather than pretending otherwise.

To check the toolchain, GPUI pin, config parse, hot-mode availability and Linux libraries:

```bash
auro doctor
```

On Linux, `auro doctor` reports each native library as `ok` or `MISSING`. Missing libraries are a
build problem, not an Auro bug.

## 4. Know what the hot path covers

Hot mode is opt-in. Set it in `Auro.toml`:

```toml
[dev]
hot = true
```

It also needs `dioxus-cli` 0.7.x on `PATH` (tested with 0.7.10) or an explicit `AURO_DX` path.
`auro doctor` reports whether the app declares the `hot-reload` feature and whether the devserver is
available.

With hot mode on:

- A save in `data/src` is a boundary edit. The patch replaces that function; the running host keeps
  its window and state.
- A save in `ui`, `types`, the binary, a dependency, a manifest or the profile is host code. It
  rebuilds and restarts the devserver and app, restoring the snapshot when compatible.
- On Linux, boundary-only edits in the generated app measured **368 ms median** save-to-render
  (N=12, 325-394 ms). Windows measured 813 ms median; the shared macOS runner measured 509.5-956 ms
  across runs. Those are above the 500 ms target.
- The measurement is for the documented data boundary only. It is not a general layout or view
  reload, and it is not GA.

The full contract, including failure behavior and what is not reloadable, is in
[hot reload](architecture/hot-reload.md) and [spec 0004](specs/0004-hot-reload.md).

## 5. Build and bundle

```bash
auro build
auro bundle
```

`auro build` produces the release binary. `auro bundle` asks the platform bundler for the host's
installers. Packaging support is uneven by platform and is documented in
[platform support](reference/platform-support.md); do not assume every target produces every
installer.

Useful variants:

```bash
auro dev --dir /path/to/app
auro dev --dir . --config app.product-name="Demo"
auro build --dir .
auro bundle --dir .
```


## 6. Assets for file-backed elements

`svg().path("icons/logo.svg")` and other file-backed GPUI elements need an asset source. Auro
installs none by default; configure one on the builder:

```rust,ignore
App::new()
    .assets(auro::assets::FileAssets::new("assets"))
    // ...
```

Use `EmbeddedAssets` for a static `include_bytes!` table, or implement
`auro::gpui::AssetSource` directly. `FileAssets` rejects `..` and absolute paths, and checks the
opened file handle against the canonical path before reading; symlinked path components are
rejected on Unix, and Windows directory listing is reported as unsupported rather than guessed.
See [toolkits](architecture/toolkits.md#assets-and-svgpath) for the exact boundary.

## Where to go next

|If you want to|Read|
|---|---|
|Understand the app loop| [Architecture overview](architecture/overview.md) |
|Follow a command across threads| [Threading and IPC](architecture/threading-and-ipc.md) |
|Change configuration| [Configuration](specs/0002-config.md) |
|Know what is actually verified| [Platform support](reference/platform-support.md) |
|Know why a decision was made| [Decision records](decisions/index.html) |
|Check the alpha gates| [Alpha release](specs/0024-alpha-release.md) |

For a bug or a build failure, include the output of `auro doctor`, the host OS and compositor, the
CLI version, and the commit or crate version. The project is pre-alpha; a reproducible report is
more useful than a compatibility claim.
