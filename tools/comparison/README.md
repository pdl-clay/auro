# Comparison benchmark

Same-app process measurement for the numbers in [the performance budget](../../docs/src/reference/performance-budget.md):
Auro, Tauri and Electron, each showing the same 480x320 window with one line of text.

```
python3 tools/comparison/bench.py            # prepare fixtures, measure, write baseline.json
python3 tools/comparison/bench.py --prepare-only
python3 tools/comparison/bench.py --run-only --runs 9
```

Raw samples land in `tools/comparison/baseline.json`. Fixtures and build trees live under
`$TMPDIR/auro-comparison` (override with `--scratch`) and are reused across runs.

## What is measured

| Metric | Method |
|---|---|
| Startup | spawn to the content-ready marker, median of `--runs` after `--warmup` runs |
| Idle RSS | sum of the process tree's `VmRSS` after `--idle` seconds (default 5); shared pages count per process |
| Idle PSS | sum of the process tree's `Pss` after the same idle period; shared pages split across users, the fairer cross-framework number |
| Payload | uncompressed runtime payload: app binary for Auro and Tauri; Electron `dist` plus app files |

Markers are the nearest available "first content ready" signal, not a guaranteed painted frame:

- Auro prints `auro:startup` on the first frame request (`AURO_STARTUP_TRACE=1`).
- Tauri's page invokes a command after the document is parsed; the command prints `BENCH_READY`.
- Electron prints `BENCH_READY` from `ready-to-show`, before showing the window.

Electron's payload includes Chromium. Tauri and Auro use the system WebView/GPU stack, so the
payload excludes the OS webview libraries and the Electron result is the cost of bundling them.
Payload is the uncompressed runtime, not the installer: AppImage/DMG/MSI compression is excluded
for all three.

## Reproducibility

- The three fixtures are generated at run time; no Node, JavaScript or `tauri` dependency is
  committed to this repository.
- Versions are pinned in `bench.py` and recorded in `baseline.json` with the OS and `rustc`.
- Run on an otherwise idle machine, in a real Wayland or X11 session. The window is shown during
  each sample; do not run this on a machine the user needs.
- A headless Wayland compositor is not usable: GPUI requires `wl_seat`, and Auro now rejects a
  compositor without one before starting.
