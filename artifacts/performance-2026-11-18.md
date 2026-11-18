# Performance measurement - 2026-11-18

This is an alpha measurement, not a published baseline. It does not change the 1.0 targets.

## Environment

| Item | Value |
|---|---|
| Commit | `726d6e3` (`origin/main`, merge of PR #9) |
| OS | CachyOS Linux 7.3.4-1, x86-64 |
| Compositor | niri 26.04 (Wayland, `wayland-1`; X11 `:0` also available) |
| CPU | AMD Ryzen 5 5600H, 12 logical CPUs |
| RAM | 22 GiB |
| Toolchain | rustc 1.99.1, cargo 1.99.1 |
| App | `examples/hello` (release, `strip = "symbols"`, `panic = "abort"`) |
| Command | `CARGO_TARGET_DIR=... cargo run -q -p auro-cli -- bench --dir examples/hello --runs 20 --idle-seconds 5 --json` |
| Build | release build was 2m44s cold in the shared target; the measured run reused it |

The marker is `auro:startup`, printed by `window.on_next_frame` in `App::try_run`: it is a request
for the first frame, **not proof of a painted frame**. The stricter painted-frame method is still
a target.

## Warm run, N=20

Raw data: `performance-2026-11-18-bench.json`.

| Metric | min | median | p95 | max |
|---|---|---|---|---|
| Startup (ms) | 134.1 | **143.8** | 151.6 | 151.6 |
| Idle RSS (MiB, after 5 s) | 104.8 | **105.1** | 105.4 | 105.4 |

Startup samples (ms): 141.6, 147.6, 134.1, 144.6, 151.6, 136.2, 151.6, 138.2, 143.3, 139.0,
135.8, 145.7, 142.8, 144.9, 148.4, 140.7, 149.6, 143.9, 147.4, 143.6.

Binary: 15,618,392 bytes (15.6 MB decimal, stripped).

The first `auro bench` run after the build had a first-sample outlier of 1879.5 ms; its median was
144.8 ms and p95 161.1 ms. The warm run above has no such outlier. The outlier is recorded, not
smoothed away; it is why the harness prints raw samples.

The previous documented 5-run measurement was 154.9 ms / 105.3 MB / 15.6 MB. The warm 20-run
median is 143.8 ms, but the run count, warmth and sample set differ; this is **not** a claim of a
performance improvement.

## RSS composition

A separate 5-second sample of the same release binary:

```
Rss:            108128 kB
Pss:            103425 kB
Pss_File:        85809 kB
Pss_Anon:        17608 kB
Private_Clean:   84940 kB
Private_Dirty:   17616 kB
```

Most of the RSS is file-backed private mapping (the binary, GPU driver and shared libraries), not
anonymous heap. The bench's `sysinfo` RSS is consistent with this.

## Binary size

`llvm-size -A` on the stripped artifact:

| Section | Size |
|---|---|
| `.text` | 11,780,845 bytes |
| `.rodata` | 1,292,708 bytes |
| `.eh_frame` | 994,320 bytes |
| `.rela.dyn` | 765,504 bytes |
| `.eh_frame_hdr` | 148,620 bytes |
| other | remainder |

`cargo bloat --release --crates` (analysis binary is unstripped, 26.8 MiB; `.text` 11.2 MiB, so
the tool's file size is not the shipped artifact size):

| Crate | .text | Share of .text |
|---|---|---|
| gpui | 2.1 MiB | 18.8% |
| zbus | 1.7 MiB | 15.3% |
| naga | 1.3 MiB | 11.7% |
| std | 753 KiB | 6.5% |
| rustybuzz | 529 KiB | 4.6% |
| oo7 | 438 KiB | 3.8% |
| skrifa | 389 KiB | 3.4% |
| usvg | 357 KiB | 3.1% |
| swash | 278 KiB | 2.4% |
| ttf_parser | 262 KiB | 2.3% |
| taffy | 202 KiB | 1.8% |
| read_fonts | 150 KiB | 1.3% |
| zvariant | 143 KiB | 1.2% |
| auro_core | 135 KiB | 1.2% |
| wayland_client | 125 KiB | 1.1% |
| ashpd | 117 KiB | 1.0% |
| blade_graphics | 115 KiB | 1.0% |
| cosmic_text | 114 KiB | 1.0% |
| x11rb_protocol | 113 KiB | 1.0% |
| 189 more | 1.2 MiB | 10.4% |

Top functions are layout and shader/font work, not Auro's app layer: Taffy grid track sizing
(77.4 KiB), Taffy flexbox (45.7 KiB), Skrifa glyph hinting (35.0 KiB), zbus builder/signal
closures (~33 KiB each), oo7 keyring init (~31 KiB each), naga SPIR-V writer/validator, swash
bitmap resize and usvg text layout. `cargo bloat` itself labels the numbers as estimates.

A linker map was produced at `/tmp/auro-hello.map` (23 MB, not committed). Its top `.text` objects
are GPUI codegen units (495.7, 414.9, 407.4, 318.0, 310.0 KiB), then `libstd` (279.2 KiB), Naga
(223.0/214.3/181.6 KiB), zbus (149.1 KiB), swash (148.5 KiB) and Skrifa (125.7 KiB). This agrees
with `cargo bloat`.

## Loader

`LD_DEBUG=statistics` for one launch:

```
total startup time in dynamic loader: 755007 cycles
time needed for relocation: 218064 cycles (28.8%)
number of relocations: 871
number of relative relocations: 31655
```

At the host's ~3.3 GHz, the loader portion is on the order of a fraction of a millisecond; it is
not the 144 ms startup. The exact wall-clock split between GPUI init, window creation, GPU/driver
work and the first-frame request was **not** measured: `perf`, `strace` and `valgrind` are not
installed on this host, and no sampling profiler was run. Do not turn the binary-size shares into
a startup attribution.

## Limits

- No Windows or macOS measurement was made here. Do not infer them.
- The marker is a first-frame request, not a painted frame.
- The host was otherwise idle for the warm run, but the first run had a 1879 ms outlier; the raw
  samples are kept.
- The 1.0 targets (<80 ms startup, <40 MB RSS, <6 MB binary) are unchanged. No optimization was
  attempted, so no gain is claimed.
- A CI regression baseline is still not implemented; this artifact is a point-in-time record.

## Next measurements

1. Add a painted-frame marker (or a frame-presented callback) and record both markers.
2. Run N>=20 on Windows and macOS on real hosts; record compositor/GPU.
3. Install a sampling profiler (`perf` or `samply`) and attribute startup to loader, GPUI/Window,
   GPU/shader and font initialization. Do not optimize from `cargo bloat` alone.
4. Trim optional features only after a measured experiment (for example, a build without the
   portal/keyring path if GPUI allows it) and keep the target unchanged until the data justifies it.
