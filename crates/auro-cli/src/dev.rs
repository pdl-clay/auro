// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use auro_core::{AuroError, Config};
use notify::{RecursiveMode, Watcher};

/// Printed by the app once a window's first repaint is requested (spec 0004, D6).
const READY_MARKER: &str = "auro:trace window_requested";
const TRACE_ENV: &str = "AURO_DEV_TRACE";

/// What the child reports to the dev loop over stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChildEvent {
    /// First repaint was requested for this window label.
    Ready(String),
    /// The app restored or reset its snapshot; the payload is the user-facing label.
    State(String),
}

/// Builds, spawns and watches the app. A change rebuilds and relaunches; Ctrl-C leaves nothing
/// behind.
///
/// On Windows a running executable cannot be replaced, so the dev loop runs a copy from
/// `target/auro-dev/` and leaves the cargo artifact free to be relinked.
pub fn run(dir: PathBuf, overrides: &[String]) -> Result<(), AuroError> {
    let dir = dir.canonicalize()?;
    let config = crate::project::load_checked(&dir)?;
    let patch = overrides_json(overrides)?;

    if config.dev.hot {
        return run_hot(&dir, &config, patch.as_deref());
    }

    let first = build(&dir, &config)?;
    let mut generation = 1u64;
    let mut running = deploy(&first, &dir, generation)?;
    println!("auro: running {}", running.display());
    let (child_tx, child_rx) = mpsc::channel::<ChildEvent>();
    let slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(spawn(
        &running,
        &dir,
        &config.dev.run_args,
        patch.as_deref(),
        child_tx.clone(),
    )?)));
    // Save-to-window bookkeeping for the trace line (spec 0004, D6).
    let mut saved_at: Option<Instant> = None;
    let mut built_at: Option<Instant> = None;

    install_ctrl_c(Arc::clone(&slot))?;

    let (events, received) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Reads are not changes. The app opens `Auro.toml` on every start, and reacting to that
        // re-triggers the build that started it: an endless rebuild loop.
        if let Ok(event) = &event
            && matches!(event.kind, notify::EventKind::Access(_))
        {
            return;
        }
        if std::env::var_os("AURO_DEV_TRACE").is_some()
            && let Ok(event) = &event
        {
            eprintln!("auro: event {:?} {:?}", event.kind, event.paths);
        }
        let _ = events.send(event);
    })
    .map_err(|error| AuroError::Message(format!("cannot start the file watcher: {error}")))?;

    for path in watch_paths(&dir, &config) {
        watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|error| {
                AuroError::Message(format!("cannot watch {}: {error}", path.display()))
            })?;
    }

    loop {
        // Short tick: the ready marker is picked up here, so the poll interval bounds the
        // resolution of `build_to_window_ms` (spec 0004, D6).
        match received.recv_timeout(Duration::from_millis(50)) {
            Ok(_) => {
                saved_at = Some(Instant::now());
                // Coalesce the burst of events one save produces.
                std::thread::sleep(Duration::from_millis(200));
                while received.try_recv().is_ok() {}

                println!("auro: change detected, rebuilding");
                match build(&dir, &config) {
                    Ok(next) => {
                        while child_rx.try_recv().is_ok() {}
                        built_at = Some(Instant::now());
                        match deploy(&next, &dir, generation + 1) {
                            Ok(deployed) => {
                                generation += 1;
                                stop(&slot);
                                let previous = std::mem::replace(&mut running, deployed);
                                if previous != running {
                                    // Best effort: Windows may still hold the old image.
                                    let _ = std::fs::remove_file(previous);
                                }
                                let child = spawn(
                                    &running,
                                    &dir,
                                    &config.dev.run_args,
                                    patch.as_deref(),
                                    child_tx.clone(),
                                )?;
                                println!("auro: rebuilt, running {}", running.display());
                                *slot.lock().expect("dev slot poisoned") = Some(child);
                            }
                            Err(error) => eprintln!(
                                "auro: could not stage the new binary, previous UI still running: {error}"
                            ),
                        }
                    }
                    Err(error) => {
                        eprintln!("auro: build failed, previous UI still running: {error}")
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                while let Ok(event) = child_rx.try_recv() {
                    match event {
                        ChildEvent::Ready(label) => {
                            if let (Some(saved), Some(built)) = (saved_at, built_at) {
                                let now = Instant::now();
                                println!(
                                    "{}",
                                    trace_line(&label, now - saved, built - saved, now - built)
                                );
                                // One trace per save: the next edit starts a new measurement.
                                saved_at = None;
                                built_at = None;
                            }
                        }
                        ChildEvent::State(label) => {
                            // Only a relaunch has a save pending; a first launch restores the
                            // snapshot without any restart to report.
                            if saved_at.is_some() {
                                println!("auro: {label}");
                            }
                        }
                    }
                }
                if let Some(status) = child_exited(&slot) {
                    if status.success() {
                        println!("auro: app exited");
                        return Ok(());
                    }
                    return Err(AuroError::Message(format!("app exited with {status}")));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    stop(&slot);
    Ok(())
}

fn watch_paths(dir: &Path, config: &Config) -> Vec<PathBuf> {
    config
        .dev
        .watch
        .iter()
        .map(|path| dir.join(path))
        .filter(|path| path.exists())
        .collect()
}

/// Hot mode also watches the generated layout's crate sources.
///
/// A project scaffolded before the four-crate layout, or one whose `[dev] watch` was not updated,
/// would otherwise hot-patch without a save timestamp and lose the D6 measurement.
fn hot_watch_paths(dir: &Path, config: &Config) -> Vec<PathBuf> {
    let mut paths = watch_paths(dir, config);
    for member in ["ui", "data", "types"] {
        let path = dir.join(member).join("src");
        if path.is_dir() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Runs the hot-patch devserver. `dx` owns the app process; this loop forwards its output,
/// timestamps saves from our own watcher, and reports how long each patch took.
fn run_hot(dir: &Path, config: &Config, patch: Option<&str>) -> Result<(), AuroError> {
    let dx = std::env::var_os("AURO_DX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dx"));
    println!(
        "auro: hot-patch mode ({}); edits reload without relaunching",
        dx.display()
    );
    if !config.dev.run_args.is_empty() {
        eprintln!(
            "auro: note: dev.run-args is not forwarded in hot-patch mode; dx owns the app process"
        );
    }

    check_dx(&dx)?;
    if !crate::project::declares_hot_feature(dir)? {
        return Err(AuroError::Message(format!(
            "hot mode needs a `hot-reload` feature in {}; add:\n\n[features]\nhot-reload = [\"auro/hot-reload\"]\n\n(projects scaffolded by this auro version include it)",
            dir.join("Cargo.toml").display()
        )));
    }

    let mut command = Command::new(&dx);
    command
        .current_dir(dir)
        .arg("serve")
        .arg("--hot-patch")
        .arg("--interactive")
        .arg("false")
        .arg("--platform")
        .arg("desktop")
        .arg("--features")
        .arg("hot-reload")
        .env("AURO_HOT_RELOAD", "1")
        .env(TRACE_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(patch) = patch {
        command.env(auro_core::config::OVERRIDES_ENV, patch);
    }
    let mut child = command.spawn().map_err(|error| {
        AuroError::Message(format!(
            "cannot start {} ({error}); install dioxus-cli 0.7.10 or set AURO_DX",
            dx.display()
        ))
    })?;

    let (events, received) = mpsc::channel::<DxEvent>();
    if let Some(stdout) = child.stdout.take() {
        let events = events.clone();
        std::thread::spawn(move || forward(stdout, events));
    }
    if let Some(stderr) = child.stderr.take() {
        let events = events.clone();
        std::thread::spawn(move || forward(stderr, events));
    }

    let slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let app_pid: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    install_ctrl_c_hot(Arc::clone(&slot), Arc::clone(&app_pid))?;
    {
        let slot = Arc::clone(&slot);
        let events = events.clone();
        std::thread::spawn(move || {
            loop {
                if let Some(status) = child_exited(&slot) {
                    let _ = events.send(DxEvent::Exited(status));
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    let (saves, saved) = mpsc::channel::<Instant>();
    // One save is one burst of watcher events. Timestamp in the callback so the measurement starts
    // when the save is observed, not when the loop next polls the channel.
    let last_save = Arc::new(Mutex::new(None::<Instant>));
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = &event
            && matches!(event.kind, notify::EventKind::Access(_))
        {
            return;
        }
        let now = Instant::now();
        let mut last = last_save.lock().expect("save clock");
        if last.is_some_and(|last| now.duration_since(last) < Duration::from_millis(150)) {
            return;
        }
        *last = Some(now);
        let _ = saves.send(now);
    })
    .map_err(|error| AuroError::Message(format!("cannot start the file watcher: {error}")))?;
    for path in hot_watch_paths(dir, config) {
        watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|error| {
                AuroError::Message(format!("cannot watch {}: {error}", path.display()))
            })?;
    }

    // One edit is one save-to-visible measurement: `saved_at` starts it, the first
    // `auro:trace hot_render` after the patch ends it (spec 0004, D6).
    let mut saved_at: Option<Instant> = None;
    let mut patched_at: Option<Instant> = None;
    let mut patch_took: Option<u128> = None;
    let mut build_failed = false;
    // A pid change is a devserver relaunch, not a hot reload. The app's snapshot report after it
    // must be labelled as a restart (spec 0004, D4).
    let mut known_pid: Option<u32> = None;
    let mut relaunches = 0u32;

    loop {
        let mut newest_save = None;
        while let Ok(when) = saved.try_recv() {
            newest_save = Some(when);
        }
        if let Some(when) = newest_save {
            // A queued save means the edit happened after the previous build started; the newest
            // one is the edit this measurement belongs to.
            saved_at = Some(when);
            patched_at = None;
            patch_took = None;
            build_failed = false;
        }
        match received.recv_timeout(Duration::from_millis(50)) {
            Ok(DxEvent::Line(line)) => {
                println!("{line}");
                let plain = strip_ansi(&line);

                if let Some(pid) = trace_u64(&plain, "auro:trace pid ") {
                    let pid = pid as u32;
                    if let Some(previous) = known_pid
                        && previous != pid
                    {
                        relaunches += 1;
                        println!("auro: app relaunched by the devserver");
                    }
                    known_pid = Some(pid);
                    *app_pid.lock().expect("app pid lock") = Some(pid);
                }

                if let Some(took) = patch_took_ms(&line) {
                    // A devserver line that arrives before our watcher saw the save, or after the
                    // app already reported the patch, cannot be attributed to this measurement.
                    if let Some(saved) = saved_at {
                        if patched_at.is_none() {
                            patched_at = Some(Instant::now());
                            patch_took = Some(took);
                        }
                        println!(
                            "auro: hot patch applied ({took} ms, {} ms after the save)",
                            saved.elapsed().as_millis()
                        );
                    } else {
                        println!("auro: hot patch applied ({took} ms)");
                    }
                }

                if trace_u64(&plain, "auro:trace hot_patch ").is_some() {
                    patched_at.get_or_insert_with(Instant::now);
                }

                if let Some(reason) = trace_text(&plain, "auro:trace hot_data_rejected ") {
                    // The code applied but the view refused its data (for example a schema change
                    // that needs a restart). Do not present that as a reload.
                    println!("auro: reload rejected by the view: {reason}; previous UI kept");
                    saved_at = None;
                    patched_at = None;
                    patch_took = None;
                }

                if let Some(reason) = trace_text(&plain, "auro:trace hot_patch_failed ") {
                    // The app keeps its previous jump table; dx heads for a full rebuild. There is
                    // no render to measure, so this save must not produce a timing line later.
                    println!("auro: patch rejected by the app: {reason}");
                    saved_at = None;
                    patched_at = None;
                    patch_took = None;
                }

                if trace_u64(&plain, "auro:trace hot_render ").is_some() {
                    if let Some(saved) = saved_at {
                        let save_to_render = saved.elapsed();
                        println!(
                            "{}",
                            hot_trace_line(
                                patched_at.map(|patched| patched.duration_since(saved)),
                                patched_at.map(|patched| patched.elapsed()),
                                save_to_render,
                                patch_took,
                            )
                        );
                        println!(
                            "auro: reloaded in {} ms (window kept)",
                            save_to_render.as_millis()
                        );
                    }
                    saved_at = None;
                    patched_at = None;
                    patch_took = None;
                    build_failed = false;
                }

                if saved_at.is_some() && !build_failed && build_failed_line(&plain) {
                    build_failed = true;
                    println!("auro: build failed, previous UI still running");
                }

                if plain.contains("Starting full rebuild") {
                    println!("auro: hot patch failed, dev server is rebuilding the app");
                }

                // The app's own snapshot report only labels a relaunch. The first launch and an
                // in-process patch have no restart to report.
                if relaunches > 0
                    && let Some(label) = relaunch_label(&plain)
                {
                    println!("auro: {label}");
                }

                if plain.contains("exited gracefully") {
                    stop(&slot);
                    println!("auro: app exited");
                    return Ok(());
                }
            }
            Ok(DxEvent::Exited(status)) => {
                stop(&slot);
                if status.success() {
                    println!("auro: dev server exited");
                    return Ok(());
                }
                return Err(AuroError::Message(format!(
                    "dev server exited with {status}"
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Refuses a devserver whose patch protocol is not the one `subsecond` 0.7.10 speaks.
///
/// `dx` versions move the jump-table format; a mismatch fails at apply time inside the app, which
/// is much harder to diagnose than a startup error.
fn check_dx(dx: &Path) -> Result<(), AuroError> {
    let output = Command::new(dx)
        .arg("--version")
        .output()
        .map_err(|error| {
            AuroError::Message(format!(
                "cannot run {} ({error}); install dioxus-cli 0.7.10 or set AURO_DX",
                dx.display()
            ))
        })?;
    let version = String::from_utf8_lossy(&output.stdout);
    let version = version.lines().next().unwrap_or_default().trim();
    if !dx_compatible(version) {
        return Err(AuroError::Message(format!(
            "{} reports `{version}`; hot reload requires dioxus 0.7.x (tested with 0.7.10)",
            dx.display()
        )));
    }
    Ok(())
}

/// `dioxus 0.7.10 (57d6794)` is compatible; other majors/minors are not.
fn dx_compatible(version: &str) -> bool {
    version
        .strip_prefix("dioxus ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|number| number.split_once('.'))
        .is_some_and(|(major, rest)| major == "0" && rest.starts_with("7."))
}

/// A line that says the build or patch failed. `dx` keeps the previous app running.
fn build_failed_line(line: &str) -> bool {
    line.contains("Build failed")
        || line.contains("Failed to hot-patch app")
        || line.contains("error[E")
}

/// Output the devserver produced.
enum DxEvent {
    Line(String),
    Exited(ExitStatus),
}

fn forward(stream: impl std::io::Read, events: mpsc::Sender<DxEvent>) {
    use std::io::{BufRead, BufReader};
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        if events.send(DxEvent::Line(line)).is_err() {
            return;
        }
    }
}

/// `15.2s INFO Hot-patching: src\views\mod.rs took 437ms` -> `437`.
fn patch_took_ms(line: &str) -> Option<u128> {
    let line = strip_ansi(line);
    if !line.contains("Hot-patching:") {
        return None;
    }
    let (_, rest) = line.rsplit_once("took ")?;
    let digits: String = rest
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `auro:trace hot_render 3` -> `3`, even when the devserver prefixes a timestamp.
fn trace_u64(line: &str, marker: &str) -> Option<u64> {
    let (_, rest) = line.split_once(marker)?;
    rest.split_whitespace().next()?.parse().ok()
}

/// The message after a trace marker, e.g. the reason in `auro:trace hot_patch_failed <reason>`.
fn trace_text<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let (_, rest) = line.split_once(marker)?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// One machine-readable line per applied patch (spec 0004, D6).
fn hot_trace_line(
    save_to_patch: Option<Duration>,
    patch_to_render: Option<Duration>,
    save_to_render: Duration,
    patch_took_ms: Option<u128>,
) -> String {
    let field = |value: Option<Duration>| {
        value
            .map(|duration| duration.as_millis().to_string())
            .unwrap_or_else(|| "null".to_owned())
    };
    let took = patch_took_ms
        .map(|took| took.to_string())
        .unwrap_or_else(|| "null".to_owned());
    format!(
        "{{\"event\":\"hot_reload\",\"save_to_patch_ms\":{},\"patch_to_render_ms\":{},\"save_to_render_ms\":{},\"patch_took_ms\":{took}}}",
        field(save_to_patch),
        field(patch_to_render),
        save_to_render.as_millis()
    )
}

/// Removes ANSI control sequences so log parsing sees the text the terminal shows.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            plain.push(character);
            continue;
        }
        if characters.next() == Some('[') {
            for control in characters.by_ref() {
                if ('@'..='~').contains(&control) {
                    break;
                }
            }
        }
    }
    plain
}

fn build(dir: &Path, config: &Config) -> Result<PathBuf, AuroError> {
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--message-format=json");
    command.args(&config.dev.build_args);

    let output = command.output()?;
    if !output.status.success() {
        // The compiler already printed to its own stderr; keep the tail so a failed rebuild says
        // why (a locked executable on Windows was invisible without this).
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(AuroError::Message(format!(
            "cargo build failed:\n{}",
            tail.join("\n")
        )));
    }

    let mut binary = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] == "compiler-artifact"
            && let Some(executable) = value["executable"].as_str()
        {
            binary = Some(PathBuf::from(executable));
        }
    }

    binary.ok_or_else(|| AuroError::Message("cargo produced no binary to run".to_owned()))
}

/// Path the app is launched from.
///
/// Windows locks a running image, and `cargo build` must be able to overwrite its artifact, so each
/// generation runs from its own copy. Other platforms run the artifact directly.
fn deploy(binary: &Path, dir: &Path, generation: u64) -> Result<PathBuf, AuroError> {
    if !cfg!(windows) {
        return Ok(binary.to_path_buf());
    }
    let folder = dir.join("target").join("auro-dev");
    std::fs::create_dir_all(&folder)?;
    // Names unique per dev session: a copy left running by an earlier session must never collide
    // with this one (`fs::copy` over a running image fails with a sharing violation).
    let stem = binary
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("app");
    let copy = folder.join(format!("{stem}-{}-{generation}.exe", std::process::id()));
    std::fs::copy(binary, &copy)?;
    Ok(copy)
}

/// Builds the child command so tests can inspect exactly what the app receives.
fn app_command(binary: &Path, dir: &Path, run_args: &[String], patch: Option<&str>) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(dir).args(run_args);
    if let Some(patch) = patch {
        command.env(auro_core::config::OVERRIDES_ENV, patch);
    }
    command.env(TRACE_ENV, "1");
    command
}

fn spawn(
    binary: &Path,
    dir: &Path,
    run_args: &[String],
    patch: Option<&str>,
    events: mpsc::Sender<ChildEvent>,
) -> Result<Child, AuroError> {
    let mut command = app_command(binary, dir, run_args, patch);
    command.stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        AuroError::Message(format!("cannot start {}: {}", binary.display(), error))
    })?;

    // Forward the app's stderr and watch for the first-repaint marker (spec 0004, D6).
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(label) = ready_label(&line) {
                    let _ = events.send(ChildEvent::Ready(label.to_owned()));
                } else if let Some(label) = relaunch_label(&line) {
                    let _ = events.send(ChildEvent::State(label));
                }
                eprintln!("{line}");
            }
        });
    }

    Ok(child)
}

/// `auro:trace window_requested main` -> `main`; any other line is not a marker.
fn ready_label(line: &str) -> Option<&str> {
    line.strip_prefix(READY_MARKER).map(str::trim)
}

/// Turns the app's snapshot report into the relaunch label required by spec 0004, D4.
fn relaunch_label(line: &str) -> Option<String> {
    // The devserver prefixes the app's stderr with timestamps, so match anywhere in the line.
    if line.contains("auro: state restored") {
        return Some("restarted (state restored)".to_owned());
    }
    let (_, reason) = line.split_once("auro: state reset: ")?;
    Some(format!("restarted (state reset: {reason})"))
}

/// One machine-readable line per save, so save-to-visible can be computed without a stopwatch.
fn trace_line(
    label: &str,
    save_to_window: Duration,
    save_to_build: Duration,
    build_to_window: Duration,
) -> String {
    format!(
        "{{\"event\":\"relaunch\",\"window\":{label:?},\"save_to_build_ms\":{},\"build_to_window_ms\":{},\"save_to_window_ms\":{}}}",
        save_to_build.as_millis(),
        build_to_window.as_millis(),
        save_to_window.as_millis()
    )
}

fn stop(slot: &Arc<Mutex<Option<Child>>>) {
    if let Ok(mut slot) = slot.lock()
        && let Some(mut child) = slot.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn child_exited(slot: &Arc<Mutex<Option<Child>>>) -> Option<ExitStatus> {
    let Ok(mut slot) = slot.lock() else {
        return None;
    };
    let child = slot.as_mut()?;
    match child.try_wait() {
        Ok(Some(status)) => {
            slot.take();
            Some(status)
        }
        Ok(None) | Err(_) => None,
    }
}

fn install_ctrl_c(slot: Arc<Mutex<Option<Child>>>) -> Result<(), AuroError> {
    ctrlc::set_handler(move || {
        stop(&slot);
        std::process::exit(0);
    })
    .map_err(|error| AuroError::Message(format!("cannot install the Ctrl-C handler: {error}")))
}

/// Ctrl-C in hot mode must also end the app `dx` launched.
///
/// Killing the devserver leaves its child running: `dx` owns the app process, so `Child::kill` on
/// the devserver alone orphans a window the user can no longer reload.
fn install_ctrl_c_hot(
    slot: Arc<Mutex<Option<Child>>>,
    app_pid: Arc<Mutex<Option<u32>>>,
) -> Result<(), AuroError> {
    ctrlc::set_handler(move || {
        stop(&slot);
        if let Some(pid) = *app_pid.lock().expect("app pid lock") {
            kill_process(pid);
        }
        std::process::exit(0);
    })
    .map_err(|error| AuroError::Message(format!("cannot install the Ctrl-C handler: {error}")))
}

/// Ends the process the devserver launched, ignoring an already-dead pid.
fn kill_process(pid: u32) {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.arg("-9").arg(pid.to_string());
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.arg("/PID").arg(pid.to_string()).arg("/T").arg("/F");
        command
    };
    #[cfg(not(any(unix, windows)))]
    let mut command = {
        let _ = pid;
        return;
    };
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// `app.product-name=Demo` becomes `{"app":{"product-name":"Demo"}}`.
fn overrides_json(overrides: &[String]) -> Result<Option<String>, AuroError> {
    if overrides.is_empty() {
        return Ok(None);
    }

    let mut root = serde_json::Value::Object(serde_json::Map::new());

    for entry in overrides {
        let (key, value) = entry.split_once('=').ok_or_else(|| {
            AuroError::Message(format!("--config expects KEY=VALUE, got `{entry}`"))
        })?;
        let parts: Vec<&str> = key.split('.').filter(|part| !part.is_empty()).collect();
        if parts.is_empty() {
            return Err(AuroError::Message(format!(
                "--config expects KEY=VALUE, got `{entry}`"
            )));
        }
        set_path(
            &mut root,
            &parts,
            serde_json::Value::String(value.to_owned()),
        );
    }

    Ok(Some(root.to_string()))
}

fn set_path(node: &mut serde_json::Value, path: &[&str], value: serde_json::Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = node;
    for part in parents {
        if !node[*part].is_object() {
            node[*part] = serde_json::Value::Object(serde_json::Map::new());
        }
        node = &mut node[*part];
    }
    node[*last] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_run_args_to_the_app() {
        let command = app_command(
            Path::new("/tmp/demo"),
            Path::new("/tmp"),
            &["--verbose".to_owned(), "value".to_owned()],
            Some("{}"),
        );

        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--verbose", "value"]);

        let overrides = auro_core::config::OVERRIDES_ENV;
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key.to_string_lossy() == overrides && value.is_some()),
            "the override patch must reach the child"
        );
    }

    #[test]
    fn builds_a_merge_patch_from_flags() {
        let patch = overrides_json(&[
            "app.product-name=Demo".to_owned(),
            "app.version=1.2.3".to_owned(),
        ])
        .expect("patch")
        .expect("some");

        let value: serde_json::Value = serde_json::from_str(&patch).expect("json");
        assert_eq!(value["app"]["product-name"], "Demo");
        assert_eq!(value["app"]["version"], "1.2.3");
    }

    #[test]
    fn rejects_a_flag_without_a_value() {
        assert!(overrides_json(&["app".to_owned()]).is_err());
    }

    #[test]
    fn parses_the_first_repaint_marker() {
        assert_eq!(
            ready_label("auro:trace window_requested main"),
            Some("main")
        );
        assert_eq!(
            ready_label("auro:trace window_requested settings"),
            Some("settings")
        );
        assert_eq!(ready_label("auro: running /tmp/app"), None);
        assert_eq!(ready_label(""), None);
    }

    #[test]
    fn labels_a_relaunch_by_what_happened_to_the_snapshot() {
        assert_eq!(
            relaunch_label("auro: state restored from /tmp/dev-snapshot.json").as_deref(),
            Some("restarted (state restored)")
        );
        assert_eq!(
            relaunch_label("auro: state reset: snapshot entry version 2 cannot be read").as_deref(),
            Some("restarted (state reset: snapshot entry version 2 cannot be read)")
        );
        assert_eq!(relaunch_label("auro: rebuilt, running /tmp/app"), None);
        assert_eq!(relaunch_label(""), None);
    }

    #[test]
    fn reads_the_devserver_patch_duration() {
        assert_eq!(
            patch_took_ms("15.2s INFO Hot-patching: src\\views\\mod.rs took 437ms"),
            Some(437)
        );
        assert_eq!(
            patch_took_ms("Hot-patching: src\\main.rs took 1238ms"),
            Some(1238)
        );
        assert_eq!(
            patch_took_ms("8.8s  INFO Hot-patching: src/views/mod.rs took \u{1b}[1m704ms\u{1b}[0m"),
            Some(704)
        );
        assert_eq!(
            patch_took_ms("12.0s INFO Build completed successfully"),
            None
        );
        assert_eq!(patch_took_ms("Hot-patching: something else"), None);
    }

    #[test]
    fn labels_state_reports_inside_devserver_output() {
        assert_eq!(
            relaunch_label("222.5s ERROR auro: state restored from /tmp/s.json").as_deref(),
            Some("restarted (state restored)")
        );
        assert_eq!(
            relaunch_label("1.0s ERROR auro: state reset: bad version").as_deref(),
            Some("restarted (state reset: bad version)")
        );
    }

    #[test]
    fn emits_one_json_trace_per_save() {
        let line = trace_line(
            "main",
            Duration::from_millis(4321),
            Duration::from_millis(3456),
            Duration::from_millis(865),
        );
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert_eq!(value["event"], "relaunch");
        assert_eq!(value["window"], "main");
        assert_eq!(value["save_to_build_ms"], 3456);
        assert_eq!(value["build_to_window_ms"], 865);
        assert_eq!(value["save_to_window_ms"], 4321);
    }

    #[test]
    fn reads_app_trace_markers_through_devserver_prefixes() {
        assert_eq!(
            trace_u64(
                "  1.2s ERROR auro:trace hot_render 3",
                "auro:trace hot_render "
            ),
            Some(3)
        );
        assert_eq!(
            trace_u64("auro:trace hot_patch 12", "auro:trace hot_patch "),
            Some(12)
        );
        assert_eq!(
            trace_u64(" 10s ERROR auro:trace pid 12345", "auro:trace pid "),
            Some(12345)
        );
        assert_eq!(
            trace_u64("auro: view data -> Reloaded", "auro:trace hot_render "),
            None
        );
        assert_eq!(trace_u64("", "auro:trace pid "), None);
    }

    #[test]
    fn emits_hot_reload_timing_with_null_when_a_stage_is_missing() {
        let line = hot_trace_line(
            Some(Duration::from_millis(300)),
            Some(Duration::from_millis(4)),
            Duration::from_millis(304),
            Some(296),
        );
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert_eq!(value["event"], "hot_reload");
        assert_eq!(value["save_to_patch_ms"], 300);
        assert_eq!(value["patch_to_render_ms"], 4);
        assert_eq!(value["save_to_render_ms"], 304);
        assert_eq!(value["patch_took_ms"], 296);

        let line = hot_trace_line(None, None, Duration::from_millis(900), None);
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert!(value["save_to_patch_ms"].is_null());
        assert!(value["patch_to_render_ms"].is_null());
        assert!(value["patch_took_ms"].is_null());
    }

    #[test]
    fn reads_the_reason_a_patch_was_rejected() {
        assert_eq!(
            trace_text(
                "  2.1s ERROR auro:trace hot_patch_failed Failed to load library: file too short",
                "auro:trace hot_patch_failed "
            ),
            Some("Failed to load library: file too short")
        );
        assert_eq!(
            trace_text(
                "auro:trace hot_patch_failed",
                "auro:trace hot_patch_failed "
            ),
            None
        );
        assert_eq!(
            trace_text("auro:trace hot_patch 3", "auro:trace hot_patch_failed "),
            None
        );
    }

    #[test]
    fn recognises_build_and_patch_failures() {
        assert!(build_failed_line(
            "12.1s ERROR Build failed: cargo failed with status 101"
        ));
        assert!(build_failed_line(
            "error[E0425]: cannot find value `x` in this scope"
        ));
        assert!(build_failed_line("Failed to hot-patch app: patch error"));
        assert!(!build_failed_line("Build completed successfully in 1.2s"));
        assert!(!build_failed_line(
            "auro: build failed, previous UI still running"
        ));
    }

    #[test]
    fn accepts_only_the_tested_devserver_line() {
        assert!(dx_compatible("dioxus 0.7.10 (57d6794)"));
        assert!(dx_compatible("dioxus 0.7.11"));
        assert!(!dx_compatible("dioxus 0.8.0 (abc)"));
        assert!(!dx_compatible("dioxus 0.6.3"));
        assert!(!dx_compatible("0.7.10"));
        assert!(!dx_compatible(""));
    }
}
