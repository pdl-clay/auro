# Hello

The minimal Auro app used as the process benchmark fixture. It has no toolkit dependency, so startup,
idle RAM and binary size measure the framework rather than a component library.

```bash
cd examples/hello
cargo run --release
```

Measure it from the repository root:

```bash
auro bench --dir examples/hello
```

The example uses the same release profile as the workspace (`strip = "symbols"`, `panic = "abort"`). The benchmark builds the release binary, waits for `auro:startup` (first-frame request), samples
RSS after the idle interval and reports the stripped binary size. It is a manual/local harness; no
baseline is published yet.
