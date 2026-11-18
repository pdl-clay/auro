# Performance budget

These numbers are the product argument. A framework that claims native performance and then ships
a 40 MB binary has no reason to exist.

| Metric | Target at 1.0 | Status |
|---|---|---|
| Cold start | < 80 ms | Local 5-run median **154.9 ms** (first-frame request, not painted frame) |
| Idle RAM (hello world) | < 40 MB | Local 5-run median **105.3 MB** |
| Hello-world binary (stripped) | < 6 MB | Local **15.6 MB** |
| Edit to see | < 500 ms | **Met on Linux for the documented warm case; at the boundary on a shared macOS CI runner; not met on Windows.** Hot mode (opt-in), four-crate scaffold: Linux **368 ms** median save→render (N=12, 325-394, `debug=0` ThinLink, state preserved), patch→render 1-26 ms; the earlier single-crate scaffold measured 521 ms (N=11). Windows (WinBoat, MSVC, interactive session) **813 ms** median over nine steady-state patches with the guarded four-crate scaffold (752-897; a plain-boundary four-crate series measured 1168 over nine, and the earlier single-crate scaffold 1375 over five; patch→render 2-16 ms). macOS on a shared `macos-26-arm64` CI runner measured **509.5 ms** and **837.0 ms** medians on two six-warmup runs (N=12 each, 372-2096); a host restart with snapshot restore passed. The runner variance is too large to claim the target there. Relaunch mode still measures 2730 ms save→repaint-requested; an earlier relink-dominated run measured ~60 s. See [spec 0004](../specs/0004-hot-reload.md) and [hot reload](../architecture/hot-reload.md) |
| Frame time | 16.6 ms at 60 fps | Inherited from GPUI, unverified |

## How each metric is defined

Precision matters more than the number, because a budget you cannot reproduce is a slogan.

- **Cold start** - process spawn to first painted frame, measured as the median of N runs on an
  idle machine, with the app installed as a bundled binary rather than run from `target/debug`.
- **Idle RAM** - resident set size after first paint and 5 seconds of no input, hello-world app.
- **Binary size** - release build, `strip = "symbols"`, `panic = "abort"`, `.app`/MSI/AppImage
  payload, not the directory total.
- **Edit to see** - save event in the editor to a changed pixel, median of N edits, hot reload
  enabled, state preserved.

## Process harness

`auro bench --dir <app>` builds the release binary, spawns it `--runs` times with
`AURO_STARTUP_TRACE=1`, measures spawn-to-`auro:startup` (first-frame request), samples RSS after
`--idle-seconds`, and reports the release binary size. `--json` emits a machine-readable result.
The marker is a first-frame request, not proof of a painted frame; the stricter painted-frame
method remains a target. Run it on an otherwise idle machine and record the platform/toolchain with
the result.

Local measurement, 2026-11-18, Linux x86_64, niri/Wayland, release build, `examples/hello`, five
runs with a five-second idle sample: startup median **154.9 ms**, idle RSS median **105.3 MB**,
stripped binary **15.6 MB**. These are alpha measurements, not a published baseline or a Tauri/
Electron comparison; the 1.0 targets above are not met yet.

## Enforcement

The roadmap commits to measuring these in CI and failing the build on regression. That is not
implemented yet: the harness exists, but there is no committed baseline, pinned runner class or
regression gate. When it lands:

- benchmarks run on a pinned runner class, and the runner is recorded with the result;
- results are committed as a baseline file, so a pull request shows a delta;
- a regression beyond a stated tolerance fails the job, with an explicit `perf-regression`
  label override that requires maintainer approval.

Micro-benchmarks use `criterion`; process-level measurements are separate harnesses, because they
measure a binary rather than a function.

## Comparison runs

Tauri and Electron baselines exist only as advertised ranges in the README. Publishing a fair
comparison requires the same app, the same platform and the same measurement method. Until that
exists, the comparison table is a target, not a result.
