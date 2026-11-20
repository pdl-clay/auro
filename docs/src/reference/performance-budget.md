# Performance budget

These numbers are the product argument. A framework that claims native performance and then ships
a 40 MB binary has no reason to exist.

| Metric | Target at 1.0 | Status |
|---|---|---|
| Cold start | < 80 ms | Local 5-run median **154.9 ms** (first-frame request, not painted frame) |
| Idle RAM (hello world) | < 40 MB | Local 5-run median **105.3 MB** |
| Hello-world binary (stripped) | < 6 MB | Local **15.6 MB** |
| Edit to see (boundary data only) | < 500 ms | **Met on Linux for the documented warm case; above it on a shared macOS CI runner; not met on Windows.** Hot mode (opt-in), four-crate scaffold: Linux **368 ms** median save→render for edits in `data/src` (`view_data() -> HotData`), N=12, p95 394, 325-394, `debug=0` ThinLink, state preserved, patch→render 1-26 ms; the earlier single-crate scaffold measured 521 ms (N=11). Windows (WinBoat, MSVC, interactive session) **813 ms** median over nine steady-state patches with the guarded four-crate scaffold (p95 897, 752-897; a plain-boundary four-crate series measured 1168 over nine, and the earlier single-crate scaffold 1375 over five; patch→render 2-16 ms). macOS on a shared `macos-26-arm64` CI runner measured **509.5**, **837.0** and **956.0 ms** medians on three six-warmup runs (N=12 each, p95 1071/2096/1563, 372-2096); a host restart with snapshot restore passed. The runner variance is too large to claim the target there. Relaunch mode still measures 2730 ms save→repaint-requested; an earlier relink-dominated run measured ~60 s. See [spec 0004](../specs/0004-hot-reload.md) and [hot reload](../architecture/hot-reload.md) |
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

`auro bench --dir <app>` builds the release binary (honoring `CARGO_TARGET_DIR` the way cargo
does), spawns it `--runs` times with
`AURO_STARTUP_TRACE=1`, measures spawn-to-`auro:startup` (first-frame request), samples RSS after
`--idle-seconds`, and reports the release binary size. Every sample is retained: the text report
prints min, median, p95 and max for startup and RSS, and `--json` emits the same summary plus the
raw `startup_ms_samples`/`rss_mb_samples` arrays. The p95 uses the nearest-rank definition, so a
small run count (five is the default) makes p95 the maximum sample; raise `--runs` before reading
it as a tail estimate. A process that exits after the startup marker but before or during the idle
sample is reported as an error with its exit status, never as a 0 MB sample. The marker is a
first-frame request, not proof of a painted frame; the stricter painted-frame method remains a
target. Run it on an otherwise idle machine and record the platform/toolchain with the result.

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

The same-app comparison runs through [`tools/comparison/bench.py`](https://github.com/pdl-clay/auro/blob/main/tools/comparison/bench.py):
one 480x320 window with a single line of text, built in release mode for Auro, Tauri and Electron
on the same machine, measured with the same spawn-to-content-ready, process-tree-RSS/PSS and
uncompressed-payload method. The marker is the nearest available "first content ready" signal
(first frame request for Auro, page parsed for Tauri, `ready-to-show` for Electron), not a
guaranteed painted frame. Electron's payload includes Chromium; Auro and Tauri use the system
webview/GPU stack. Installer compression is excluded for all three.

Measured 2026-11-19, Linux 7.3.4 x86_64, niri/Wayland, rustc 1.99.1, five runs after one warm-up,
5 s idle. The desktop was not fully idle (Windows VM and browser running; load average 3.5 at
start), so treat the absolute numbers as a provisional baseline, not an idle-machine record:

| Target | Startup median | p95 | Idle RSS | Idle PSS | Payload |
|---|---:|---:|---:|---:|---:|
| Auro (GPUI) | 189.5 ms | 205.3 ms | 105.9 MB | 98.6 MB | 14.9 MB |
| Tauri 2.12.7 (WebKitGTK 2.54.2) | 417.4 ms | 507.6 ms | 500.7 MB | 314.5 MB | 6.7 MB |
| Electron 45.4.1 | 420.5 ms | 451.9 ms | 589.2 MB | 291.1 MB | 273.7 MB |

Raw samples: [`tools/comparison/baseline.json`](https://github.com/pdl-clay/auro/blob/main/tools/comparison/baseline.json).
The Tauri result reflects this host's WebKitGTK build; do not generalize it to other webview
versions. RSS sums shared pages per process and overstates multi-process runtimes; PSS is the
comparison number. Auro's payload is larger than Tauri's and above the 6 MB 1.0 target; its startup
and idle memory are the measured advantage here.
