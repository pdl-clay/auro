# Performance budget

These numbers are the product argument. A framework that claims native performance and then ships
a 40 MB binary has no reason to exist.

| Metric | Target at 1.0 | Status |
|---|---|---|
| Cold start | < 80 ms | Not measured - no binary |
| Idle RAM (hello world) | < 40 MB | Not measured - no binary |
| Hello-world binary (stripped) | < 6 MB | Not measured - no binary |
| Edit to see | < 500 ms | Not implemented (relaunch only). The traced dev loop measured **2730 ms** save→repaint-requested (median, debug, warm cache) and an earlier relink-dominated run measured ~60 s. The target needs hot reload; see [spec 0004](../specs/0004-hot-reload.md) and [hot reload](../architecture/hot-reload.md) |
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

## Enforcement

The roadmap commits to measuring these in CI and failing the build on regression. That is not
implemented yet: nothing is bundled, so there is no binary to measure. When it lands:

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
