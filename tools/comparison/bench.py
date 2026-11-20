#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Same-app process benchmark: Auro vs Tauri vs Electron.

The three fixtures show the same 800x600 window with one line of text. Each prints a
content-ready marker once; the harness measures spawn-to-marker, RSS after an idle period and
the runtime payload size. Markers are the nearest available "first content ready" signal, not a
guaranteed painted frame:

- Auro: `auro:startup` (first frame request, emitted by `Window::on_next_frame`).
- Tauri: the page invokes a command after the document is parsed.
- Electron: `ready-to-show` (the page has rendered and the window can be shown).

Run from anywhere; paths resolve against the repository root. Fixtures are generated under
`--scratch` (default `$TMPDIR/auro-comparison`) so the repository stays free of Node, JavaScript
and Tauri dependencies. Builds and the Electron download are reused across runs.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import selectors
import shutil
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path

TAURI_VERSION = "2.12.7"
TAURI_BUILD_VERSION = "2.7.6"
ELECTRON_VERSION = "45.4.1"
BENCH_MARKER = "BENCH_READY"
AURO_MARKER = "auro:startup"
WINDOW = (480, 320)
IDLE_SECONDS = 5.0
READY_TIMEOUT = 60.0

ROOT = Path(__file__).resolve().parents[2]
HELLO = ROOT / "examples" / "hello"


def run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def fixture_root(scratch: Path) -> Path:
    return scratch / "fixtures"


def tauri_dir(scratch: Path) -> Path:
    return fixture_root(scratch) / "tauri"


def electron_dir(scratch: Path) -> Path:
    return fixture_root(scratch) / "electron"


def write_electron_fixture(directory: Path) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "package.json").write_text(
        json.dumps(
            {
                "name": "auro-bench-electron",
                "version": "0.1.0",
                "private": True,
                "main": "main.js",
            },
            indent=2,
        )
        + "\n"
    )
    (directory / "main.js").write_text(
        """const { app, BrowserWindow } = require('electron');

app.whenReady().then(() => {
  const win = new BrowserWindow({ width: 480, height: 320, show: false });
  win.loadFile('index.html');
  win.once('ready-to-show', () => {
    console.log('BENCH_READY');
    win.show();
  });
});
"""
    )
    (directory / "index.html").write_text(
        """<!doctype html>
<html><body style="margin:0;display:grid;place-items:center;height:100vh;font-family:sans-serif">
Hello from Electron
</body></html>
"""
    )


def write_png(path: Path, width: int = 32, height: int = 32) -> None:
    """Writes a solid RGBA PNG without an image dependency."""
    import struct
    import zlib

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    row = b"\x00" + bytes((32, 64, 128, 255)) * width
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(row * height))
        + chunk(b"IEND", b"")
    )


def write_tauri_fixture(directory: Path) -> None:
    (directory / "src").mkdir(parents=True, exist_ok=True)
    (directory / "dist").mkdir(parents=True, exist_ok=True)
    (directory / "icons").mkdir(parents=True, exist_ok=True)
    write_png(directory / "icons" / "icon.png")
    (directory / "Cargo.toml").write_text(
        f"""[package]
name = "auro-bench-tauri"
version = "0.1.0"
edition = "2021"

[build-dependencies]
tauri-build = {{ version = "={TAURI_BUILD_VERSION}" }}

[dependencies]
tauri = {{ version = "={TAURI_VERSION}" }}

[profile.release]
strip = "symbols"
"""
    )
    (directory / "build.rs").write_text("fn main() {\n    tauri_build::build()\n}\n")
    (directory / "tauri.conf.json").write_text(
        json.dumps(
            {
                "$schema": "https://schema.tauri.app/config/2",
                "productName": "hello-tauri",
                "version": "0.1.0",
                "identifier": "com.auro.bench.tauri",
                "build": {"frontendDist": "dist"},
                "bundle": {"icon": ["icons/icon.png"]},
                "app": {
                    "withGlobalTauri": True,
                    "windows": [
                        {
                            "title": "hello",
                            "width": WINDOW[0],
                            "height": WINDOW[1],
                        }
                    ],
                    "security": {"csp": None},
                },
            },
            indent=2,
        )
        + "\n"
    )
    (directory / "src" / "main.rs").write_text(
        """#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tauri::command]
fn bench_ready() {
    println!("BENCH_READY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![bench_ready])
        .run(tauri::generate_context!())
        .expect("tauri app");
}
"""
    )
    (directory / "dist" / "index.html").write_text(
        """<!doctype html>
<html><body style="margin:0;display:grid;place-items:center;height:100vh;font-family:sans-serif">
Hello from Tauri
<script>window.__TAURI__.core.invoke('bench_ready');</script>
</body></html>
"""
    )


def prepare(scratch: Path) -> None:
    electron = electron_dir(scratch)
    write_electron_fixture(electron)
    electron_bin = electron / "node_modules" / ".bin" / "electron"
    if not electron_bin.exists():
        run(
            [
                "npm",
                "install",
                "--prefix",
                str(electron),
                "--no-audit",
                "--no-fund",
                f"electron@{ELECTRON_VERSION}",
            ],
            cwd=electron,
        )

    tauri = tauri_dir(scratch)
    write_tauri_fixture(tauri)
    run(["cargo", "build", "--release"], cwd=tauri)

    hello_env = os.environ.copy()
    hello_env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    run(["cargo", "build", "--release", "--locked"], cwd=HELLO, env=hello_env)


def auro_command() -> tuple[list[str], dict[str, str]]:
    env = os.environ.copy()
    env["AURO_STARTUP_TRACE"] = "1"
    return [str(ROOT / "target" / "release" / "hello")], env


def tauri_command(scratch: Path) -> tuple[list[str], dict[str, str]]:
    return [str(tauri_dir(scratch) / "target" / "release" / "auro-bench-tauri")], os.environ.copy()


def electron_command(scratch: Path) -> tuple[list[str], dict[str, str]]:
    directory = electron_dir(scratch)
    return [str(directory / "node_modules" / ".bin" / "electron"), str(directory)], os.environ.copy()


def process_tree_rss_kb(root_pid: int) -> int:
    """Sums VmRSS for the process and all of its descendants."""
    parent_of: dict[int, int] = {}
    rss_of: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            stat = (entry / "stat").read_text()
            status = (entry / "status").read_text()
        except OSError:
            continue
        # The comm field may contain spaces and parentheses; PPid follows the last ')'.
        fields = stat.rsplit(")", 1)[1].split()
        parent_of[pid] = int(fields[1])
        for line in status.splitlines():
            if line.startswith("VmRSS:"):
                rss_of[pid] = int(line.split()[1])
                break

    total = rss_of.get(root_pid, 0)
    changed = True
    descendants = {root_pid}
    while changed:
        changed = False
        for pid, parent in parent_of.items():
            if parent in descendants and pid not in descendants:
                descendants.add(pid)
                total += rss_of.get(pid, 0)
                changed = True
    return total


def wait_for_marker(proc: subprocess.Popen[str], marker: str, timeout: float) -> float:
    assert proc.stdout is not None
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    started = time.perf_counter()
    try:
        while True:
            remaining = timeout - (time.perf_counter() - started)
            if remaining <= 0:
                raise TimeoutError(f"no `{marker}` within {timeout:.0f}s")
            if not selector.select(remaining):
                continue
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError(f"process exited before `{marker}`")
            if marker in line:
                return (time.perf_counter() - started) * 1000.0
    finally:
        selector.close()


def stop(proc: subprocess.Popen[str]) -> None:
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait(timeout=5)


def measure(
    name: str,
    command: list[str],
    env: dict[str, str],
    marker: str,
    runs: int,
    warmup: int,
    idle: float,
) -> dict[str, object]:
    startup: list[float] = []
    rss: list[float] = []
    for iteration in range(warmup + runs):
        proc = subprocess.Popen(
            command,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            start_new_session=True,
        )
        try:
            millis = wait_for_marker(proc, marker, READY_TIMEOUT)
            if iteration >= warmup:
                time.sleep(idle)
                rss.append(process_tree_rss_kb(proc.pid) / 1024.0)
                startup.append(millis)
                print(f"  {name} run {len(startup)}: {millis:.1f} ms, {rss[-1]:.1f} MB", flush=True)
        finally:
            stop(proc)
    return {"startup_ms": startup, "rss_mb": rss}


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = max(0, math.ceil(fraction * len(ordered)) - 1)
    return ordered[index]


def summarize(samples: list[float]) -> dict[str, float]:
    return {
        "min": min(samples),
        "median": statistics.median(samples),
        "p95": percentile(samples, 0.95),
        "max": max(samples),
    }


def directory_size(path: Path) -> int:
    return sum(file.stat().st_size for file in path.rglob("*") if file.is_file())


def payload_bytes(scratch: Path, name: str) -> int:
    if name == "auro":
        return (ROOT / "target" / "release" / "hello").stat().st_size
    if name == "tauri":
        return (tauri_dir(scratch) / "target" / "release" / "auro-bench-tauri").stat().st_size
    electron = electron_dir(scratch)
    app_files = sum(
        (electron / file).stat().st_size
        for file in ("package.json", "main.js", "index.html")
    )
    return directory_size(electron / "node_modules" / "electron" / "dist") + app_files


def environment() -> dict[str, str]:
    def output(*cmd: str) -> str:
        try:
            return subprocess.run(
                cmd, capture_output=True, text=True, check=True
            ).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            return "unknown"

    return {
        "os": output("uname", "-srm"),
        "session": os.environ.get("XDG_SESSION_TYPE", "unknown"),
        "compositor": os.environ.get("WAYLAND_DISPLAY", os.environ.get("DISPLAY", "unknown")),
        "rustc": output("rustc", "--version"),
        "tauri": TAURI_VERSION,
        "tauri_build": TAURI_BUILD_VERSION,
        "electron": ELECTRON_VERSION,
        "auro_hello_profile": "release, strip = symbols",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scratch",
        type=Path,
        default=Path(os.environ.get("TMPDIR", "/tmp")) / "auro-comparison",
        help="fixture and build directory (default: $TMPDIR/auro-comparison)",
    )
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--idle", type=float, default=IDLE_SECONDS)
    parser.add_argument(
        "--json",
        type=Path,
        default=ROOT / "tools" / "comparison" / "baseline.json",
        help="where to write the raw samples",
    )
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--run-only", action="store_true")
    arguments = parser.parse_args()
    scratch = arguments.scratch.resolve()

    if arguments.run_only:
        for path in (
            ROOT / "target" / "release" / "hello",
            tauri_dir(scratch) / "target" / "release" / "auro-bench-tauri",
            electron_dir(scratch) / "node_modules" / ".bin" / "electron",
        ):
            if not path.exists():
                raise SystemExit(f"missing fixture (run without --run-only first): {path}")
    else:
        prepare(scratch)

    if arguments.prepare_only:
        print("fixtures prepared", flush=True)
        return 0

    targets = {
        "auro": (*auro_command(), AURO_MARKER),
        "tauri": (*tauri_command(scratch), BENCH_MARKER),
        "electron": (*electron_command(scratch), BENCH_MARKER),
    }
    results: dict[str, object] = {
        "environment": environment(),
        "method": {
            "window": f"{WINDOW[0]}x{WINDOW[1]}",
            "runs": arguments.runs,
            "warmup": arguments.warmup,
            "idle_seconds": arguments.idle,
            "startup": "spawn to content-ready marker; not a painted-frame measurement",
            "rss": "sum of the process tree after the idle period",
            "payload": "uncompressed runtime payload: app binary (Auro/Tauri) or Electron dist plus app files",
        },
        "targets": {},
    }
    for name, (command, env, marker) in targets.items():
        print(f"measuring {name}", flush=True)
        samples = measure(
            name, command, env, marker, arguments.runs, arguments.warmup, arguments.idle
        )
        results["targets"][name] = {
            "startup_ms": samples["startup_ms"],
            "startup_summary": summarize(samples["startup_ms"]),
            "rss_mb": samples["rss_mb"],
            "rss_summary": summarize(samples["rss_mb"]),
            "payload_mb": payload_bytes(scratch, name) / (1024.0 * 1024.0),
        }

    arguments.json.parent.mkdir(parents=True, exist_ok=True)
    arguments.json.write_text(json.dumps(results, indent=2) + "\n")

    print()
    print("| Target | Startup median | p95 | Idle RSS | Payload |")
    print("|---|---:|---:|---:|---:|")
    for name, target in results["targets"].items():
        target = dict(target)
        print(
            f"| {name} | {target['startup_summary']['median']:.1f} ms | "
            f"{target['startup_summary']['p95']:.1f} ms | "
            f"{target['rss_summary']['median']:.1f} MB | {target['payload_mb']:.1f} MB |"
        )
    print(f"\nraw samples: {arguments.json}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
