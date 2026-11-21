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
    let patch = overrides_json(overrides, &serde_json::to_value(&config)?)?;

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
    let watch_root = dir.clone();
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
        if let Ok(event) = &event
            && !event.paths.is_empty()
            && event
                .paths
                .iter()
                .all(|path| is_ignored_build_event(path, &watch_root))
        {
            return;
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

/// Whether a watcher event is a build artifact or an editor side file rather than a source edit.
///
/// `target/` is written by the build this loop starts; watching the app root (a valid
/// configuration) would otherwise rebuild forever. Editor swap/backup files would cause a restart
/// without a code change.
fn is_ignored_build_event(path: &Path, app_dir: &Path) -> bool {
    let relative = path.strip_prefix(app_dir).unwrap_or(path);
    if let Some(first) = relative.components().next()
        && matches!(first.as_os_str().to_str(), Some("target" | ".git"))
    {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.starts_with(".#") || name.ends_with('~') || name.ends_with(".swp")
}

/// Source and manifest files hot mode reacts to. Swap/lock files are ignored.
fn hot_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.starts_with(".#") || name.ends_with('~') || name.ends_with(".swp") {
                return false;
            }
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("rs") || extension.eq_ignore_ascii_case("toml")
                })
        })
        .collect()
}

/// The first changed file outside the reloadable crate, relative to the app.
fn outside_boundary_change(
    paths: &[PathBuf],
    dir: &Path,
    boundary_root: &Path,
    has_boundary: bool,
) -> Option<String> {
    if !has_boundary {
        return None;
    }
    paths
        .iter()
        .find(|path| !path.starts_with(boundary_root))
        .map(|path| path.strip_prefix(dir).unwrap_or(path).display().to_string())
}

/// Hot mode also watches the generated layout's crate sources.
///
/// A project scaffolded before the four-crate layout, or one whose `[dev] watch` was not updated,
/// would otherwise hot-patch without a save timestamp and lose the D6 measurement.
fn hot_watch_paths(dir: &Path, config: &Config) -> Vec<PathBuf> {
    let mut paths = watch_paths(dir, config);
    let mut add = |path: PathBuf| {
        if path.exists() && !paths.contains(&path) {
            paths.push(path);
        }
    };
    for member in ["ui", "data", "types"] {
        add(dir.join(member).join("src"));
        add(dir.join(member).join("Cargo.toml"));
    }
    add(dir.join("Cargo.toml"));
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

    let (events, received) = mpsc::channel::<DxEvent>();
    let child = spawn_devserver(&dx, dir, patch, &events)?;
    let slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let app_pid: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    install_ctrl_c_hot(Arc::clone(&slot), Arc::clone(&app_pid))?;

    // The generated layout's only reloadable crate. A save outside it compiles host code that the
    // running view never calls, so the loop must not label that outcome as a reload.
    let boundary_root = dir.join("data").join("src");
    let has_boundary = boundary_root.is_dir();
    let (saves, saved) = mpsc::channel::<(Instant, Vec<PathBuf>)>();
    // One save is one burst of watcher events. Timestamp in the callback so the measurement starts
    // when the save is observed, not when the loop next polls the channel.
    let last_save = Arc::new(Mutex::new(None::<Instant>));
    let hot_root = dir.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let paths = hot_paths(
            event
                .paths
                .into_iter()
                .filter(|path| !is_ignored_build_event(path, &hot_root))
                .collect(),
        );
        if paths.is_empty() {
            // Editors create swap files and lock files next to the source; a restart on those
            // would interrupt the session without a code change.
            return;
        }
        let now = Instant::now();
        let mut last = last_save.lock().expect("save clock");
        if last.is_some_and(|last| now.duration_since(last) < Duration::from_millis(150)) {
            return;
        }
        *last = Some(now);
        let _ = saves.send((now, paths));
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
    // A host-code change restarts the devserver. The previous app stays visible until the
    // replacement has launched; a failed rebuild must not leave the user without a window.
    let mut host_rebuild = false;
    let mut pending_app: Option<u32> = None;

    loop {
        let mut newest_save = None;
        while let Ok(save) = saved.try_recv() {
            newest_save = Some(save);
        }
        if let Some((when, paths)) = newest_save {
            // A queued save means the edit happened after the previous build started; the newest
            // one is the edit this measurement belongs to.
            saved_at = Some(when);
            patched_at = None;
            patch_took = None;
            build_failed = false;
            let outside_boundary =
                outside_boundary_change(&paths, dir, &boundary_root, has_boundary);
            if let Some(path) = outside_boundary {
                // A patch cannot replace host code: the running view and the bin were compiled
                // against the old crates. Restart the devserver and app, which restores the
                // snapshot, instead of applying a patch that cannot take effect.
                println!(
                    "auro: change in {path} is outside the hot boundary; restarting the app (state preserved)"
                );
                let previous_app = restart_devserver(&dx, dir, patch, &slot, &app_pid, &events)?;
                pending_app = previous_app;
                host_rebuild = true;
                saved_at = None;
                patched_at = None;
                patch_took = None;
                build_failed = false;
                continue;
            }
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
                    if let Some(previous) = pending_app.take()
                        && previous != pid
                    {
                        // The replacement is running; now retire the old window. Keeping it until
                        // this point means a failed build leaves the previous UI usable.
                        kill_process(previous);
                    }
                    known_pid = Some(pid);
                    host_rebuild = false;
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

                if host_rebuild && plain.contains("Build completed successfully") {
                    host_rebuild = false;
                }
                if host_rebuild && build_failed_line(&plain) {
                    host_rebuild = false;
                    if pending_app.is_some() {
                        println!("auro: host rebuild failed; previous UI still running");
                    } else {
                        println!(
                            "auro: host rebuild failed; no app is running until the next successful build"
                        );
                    }
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
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child_exited(&slot) {
                    stop(&slot);
                    if pending_app.is_some() {
                        // The old app was deliberately kept alive while the replacement was being
                        // built; do not turn a devserver crash into a lost window.
                        println!("auro: dev server exited; previous UI still running");
                    } else {
                        let pid = *app_pid.lock().expect("app pid lock");
                        if let Some(pid) = pid {
                            kill_process(pid);
                            *app_pid.lock().expect("app pid lock") = None;
                        }
                    }
                    if status.success() {
                        println!("auro: dev server exited");
                        return Ok(());
                    }
                    return Err(AuroError::Message(format!(
                        "dev server exited with {status}"
                    )));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Starts `dx serve --hot-patch` with stdout/stderr forwarded into `events`.
fn spawn_devserver(
    dx: &Path,
    dir: &Path,
    patch: Option<&str>,
    events: &mpsc::Sender<DxEvent>,
) -> Result<Child, AuroError> {
    let mut command = Command::new(dx);
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

    if let Some(stdout) = child.stdout.take() {
        let events = events.clone();
        std::thread::spawn(move || forward(stdout, events));
    }
    if let Some(stderr) = child.stderr.take() {
        let events = events.clone();
        std::thread::spawn(move || forward(stderr, events));
    }
    Ok(child)
}

/// Restarts the devserver and its app for a host-code change.
///
/// A patch cannot replace `ui`, `types`, the bin, a dependency or the profile: the running view was
/// compiled against the old code. The honest path is a process restart, which restores the snapshot
/// and is labelled as a restart, never as a hot reload.
fn restart_devserver(
    dx: &Path,
    dir: &Path,
    patch: Option<&str>,
    slot: &Arc<Mutex<Option<Child>>>,
    app_pid: &Arc<Mutex<Option<u32>>>,
    events: &mpsc::Sender<DxEvent>,
) -> Result<Option<u32>, AuroError> {
    stop(slot);
    // Keep the previous app running while the replacement builds. Callers retire it when the new
    // app reports its pid, or leave it in place if the build fails.
    let previous = *app_pid.lock().expect("app pid lock");
    let child = spawn_devserver(dx, dir, patch, events)?;
    *slot.lock().expect("dev slot poisoned") = Some(child);
    Ok(previous)
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
    let expected = crate::project::binary_name_with_args(dir, &config.dev.build_args)?;
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

    select_binary(&String::from_utf8_lossy(&output.stdout), &expected)
}

/// Picks the artifact for the app's own binary, not whichever artifact cargo emitted last.
fn select_binary(output: &str, expected: &str) -> Result<PathBuf, AuroError> {
    let mut built = Vec::new();
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-artifact" {
            continue;
        }
        let target = &value["target"];
        let is_binary = target["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"));
        let Some(name) = target["name"].as_str() else {
            continue;
        };
        if is_binary {
            built.push(name.to_owned());
        }
        if is_binary
            && name == expected
            && let Some(executable) = value["executable"].as_str()
        {
            return Ok(PathBuf::from(executable));
        }
    }

    Err(AuroError::Message(format!(
        "cargo produced no `{expected}` binary to run{}",
        if built.is_empty() {
            String::new()
        } else {
            format!(" (built: {})", built.join(", "))
        }
    )))
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
///
/// Arrays are replaced wholesale by the merge patch, so an indexed path starts its patch array
/// from the resolved config: `app.windows.0.width=800` keeps the window's `label` and other
/// fields instead of replacing the array with one partial element.
fn overrides_json(
    overrides: &[String],
    base: &serde_json::Value,
) -> Result<Option<String>, AuroError> {
    if overrides.is_empty() {
        return Ok(None);
    }

    // The patch must deserialize as TOML, which has no null; drop null-valued optional fields
    // from the base clone. Missing fields fall back to their config defaults.
    let mut patch_base = base.clone();
    remove_nulls(&mut patch_base);

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
        let parsed = parse_override(value, base, &parts)?;
        set_path(&mut root, &parts, parsed, &patch_base)
            .map_err(|message| AuroError::Message(format!("--config `{entry}`: {message}")))?;
    }

    Ok(Some(root.to_string()))
}

/// Parses one override, using the resolved config as the type hint.
///
/// A string field keeps a bare `123` as the string `123`; numeric, boolean, array and object
/// fields parse JSON. Quoting a value always makes it a JSON string.
fn parse_override(
    value: &str,
    base: &serde_json::Value,
    path: &[&str],
) -> Result<serde_json::Value, AuroError> {
    let hint = value_at(base, path);
    if matches!(hint, Some(serde_json::Value::String(_))) {
        if value.starts_with('"') {
            return serde_json::from_str(value).map_err(|error| {
                AuroError::Message(format!(
                    "--config value is not a valid JSON string: {error}"
                ))
            });
        }
        return Ok(serde_json::Value::String(value.to_owned()));
    }

    let parsed = serde_json::from_str::<serde_json::Value>(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
    if parsed.is_null() {
        return Err(AuroError::Message(
            "--config does not support `null`; TOML has no null value".to_owned(),
        ));
    }

    if hint.is_none_or(serde_json::Value::is_null) {
        // A nullable field has no value to hint from. Ask the resolved `Config` which candidate
        // fits: `Option<String>` accepts the string form, `Option<u32>` accepts the number.
        let string = serde_json::Value::String(value.to_owned());
        let string_ok = config_accepts(base, path, string.clone());
        let parsed_ok = config_accepts(base, path, parsed.clone());
        if string_ok && !parsed_ok {
            return Ok(string);
        }
        if parsed_ok {
            return Ok(parsed);
        }
        if string_ok {
            return Ok(string);
        }
    }
    Ok(parsed)
}

fn value_at<'a>(base: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut value = base;
    for part in path {
        value = base_part(value, part)?;
    }
    Some(value)
}

fn config_accepts(base: &serde_json::Value, path: &[&str], candidate: serde_json::Value) -> bool {
    let Some(value) = with_value(base, path, candidate) else {
        return false;
    };
    serde_json::from_value::<Config>(value).is_ok()
}

fn with_value(
    base: &serde_json::Value,
    path: &[&str],
    candidate: serde_json::Value,
) -> Option<serde_json::Value> {
    let mut root = base.clone();
    let mut node = &mut root;
    for part in path {
        node = base_part_mut(node, part)?;
    }
    *node = candidate;
    Some(root)
}

fn base_part_mut<'a>(
    node: &'a mut serde_json::Value,
    part: &str,
) -> Option<&'a mut serde_json::Value> {
    if let Ok(index) = part.parse::<usize>() {
        node.as_array_mut()?.get_mut(index)
    } else {
        node.as_object_mut()?.get_mut(part)
    }
}

/// Applies one `--config` path, creating objects or arrays as the segments require.
///
/// Array indices are limited to the resolved array: inventing a slot would require a complete
/// default element and would emit TOML null for skipped positions.
fn set_path(
    node: &mut serde_json::Value,
    path: &[&str],
    value: serde_json::Value,
    base: &serde_json::Value,
) -> Result<(), String> {
    let Some((last, parents)) = path.split_last() else {
        return Ok(());
    };
    let mut node = node;
    let mut base = Some(base);
    for part in parents {
        node = child(node, base, part)?;
        base = base.and_then(|base| base_part(base, part));
    }
    if let Ok(index) = last.parse::<usize>() {
        let array = array_for(node, base);
        if index >= array.len() {
            return Err(array_index_error(index, array.len()));
        }
        array[index] = value;
    } else {
        ensure_object(node).insert((*last).to_owned(), value);
    }
    Ok(())
}

fn child<'a>(
    node: &'a mut serde_json::Value,
    base: Option<&serde_json::Value>,
    part: &str,
) -> Result<&'a mut serde_json::Value, String> {
    if let Ok(index) = part.parse::<usize>() {
        let array = array_for(node, base);
        if index >= array.len() {
            return Err(array_index_error(index, array.len()));
        }
        if array[index].is_null()
            && let Some(element) = base
                .and_then(|base| base.as_array())
                .and_then(|base| base.get(index))
        {
            array[index] = element.clone();
        }
        Ok(&mut array[index])
    } else {
        Ok(ensure_object(node)
            .entry(part.to_owned())
            .or_insert(serde_json::Value::Null))
    }
}

fn array_index_error(index: usize, length: usize) -> String {
    format!(
        "array index {index} is out of range; the resolved array has {length} element(s) and a patch cannot insert null slots"
    )
}

/// Removes `null` values recursively so the patch can be represented as TOML.
fn remove_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.retain(|_, value| {
                remove_nulls(value);
                !value.is_null()
            });
        }
        serde_json::Value::Array(array) => {
            for value in array.iter_mut() {
                remove_nulls(value);
            }
            array.retain(|value| !value.is_null());
        }
        _ => {}
    }
}

/// The patch array for an indexed path, seeded from the resolved config on first use.
fn array_for<'a>(
    node: &'a mut serde_json::Value,
    base: Option<&serde_json::Value>,
) -> &'a mut Vec<serde_json::Value> {
    let array = ensure_array(node);
    if array.is_empty()
        && let Some(base) = base.and_then(|base| base.as_array())
    {
        array.clone_from(base);
    }
    array
}

fn base_part<'a>(base: &'a serde_json::Value, part: &str) -> Option<&'a serde_json::Value> {
    if let Ok(index) = part.parse::<usize>() {
        base.as_array().and_then(|array| array.get(index))
    } else {
        base.get(part)
    }
}

fn ensure_object(node: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    if !node.is_object() {
        *node = serde_json::Value::Object(serde_json::Map::new());
    }
    node.as_object_mut().expect("just replaced with an object")
}

fn ensure_array(node: &mut serde_json::Value) -> &mut Vec<serde_json::Value> {
    if !node.is_array() {
        *node = serde_json::Value::Array(Vec::new());
    }
    node.as_array_mut().expect("just replaced with an array")
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
        let patch = overrides_json(
            &[
                "app.product-name=Demo".to_owned(),
                "app.version=1.2.3".to_owned(),
            ],
            &serde_json::json!({}),
        )
        .expect("patch")
        .expect("some");

        let value: serde_json::Value = serde_json::from_str(&patch).expect("json");
        assert_eq!(value["app"]["product-name"], "Demo");
        assert_eq!(value["app"]["version"], "1.2.3");
    }

    #[test]
    fn rejects_a_flag_without_a_value() {
        assert!(overrides_json(&["app".to_owned()], &serde_json::json!({})).is_err());
    }

    #[test]
    fn types_override_values_and_indexes_arrays() {
        let base = serde_json::json!({
            "app": {
                "product-name": "Repro",
                "windows": [
                    { "label": "main", "width": 960, "max-width": 1600 },
                    { "label": "secondary", "width": 640 }
                ]
            }
        });
        let patch = overrides_json(
            &[
                "app.windows.0.width=800".to_owned(),
                "dev.hot=true".to_owned(),
                "app.product-name=Demo".to_owned(),
                "app.authors=[\"Ana\"]".to_owned(),
            ],
            &base,
        )
        .expect("patch")
        .expect("some");

        let value: serde_json::Value = serde_json::from_str(&patch).expect("json");
        assert_eq!(value["app"]["windows"][0]["width"], 800);
        assert_eq!(
            value["app"]["windows"][0]["label"], "main",
            "the sibling fields must survive an indexed override"
        );
        assert_eq!(value["app"]["windows"][0]["max-width"], 1600);
        assert_eq!(value["app"]["windows"][1]["label"], "secondary");
        assert_eq!(value["dev"]["hot"], true);
        assert_eq!(value["app"]["product-name"], "Demo");
        assert_eq!(value["app"]["authors"], serde_json::json!(["Ana"]));
    }

    #[test]
    fn a_numeric_looking_value_stays_a_string_for_a_string_field() {
        let base = serde_json::json!({ "app": { "product-name": "Repro" } });
        let patch = overrides_json(&["app.product-name=123".to_owned()], &base)
            .expect("patch")
            .expect("some");
        let value: serde_json::Value = serde_json::from_str(&patch).expect("json");
        assert_eq!(value["app"]["product-name"], "123");
    }

    #[test]
    fn nullable_fields_infer_string_and_number_from_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Auro.toml"),
            r#"
[app]
identifier = "com.example.nullable"
product-name = "Nullable"
version = "0.1.0"

[[app.windows]]
label = "main"
max-width = 1600
"#,
        )
        .expect("config");
        let config = Config::load_from(&temp.path().join("Auro.toml")).expect("valid config");
        let base = serde_json::to_value(&config).expect("json");
        let patch = overrides_json(
            &[
                "app.windows.0.title=123".to_owned(),
                "app.windows.0.min-width=320".to_owned(),
                "app.default-window=123".to_owned(),
            ],
            &base,
        )
        .expect("patch")
        .expect("some");
        let value: serde_json::Value = serde_json::from_str(&patch).expect("json");
        assert_eq!(value["app"]["windows"][0]["title"], "123");
        assert_eq!(value["app"]["windows"][0]["min-width"], 320);
        assert_eq!(value["app"]["default-window"], "123");
    }

    #[test]
    fn an_index_past_the_base_array_is_rejected_without_null() {
        let base = serde_json::json!({
            "app": { "windows": [{ "label": "main", "width": 960 }] }
        });
        let error = overrides_json(&["app.windows.2.label=third".to_owned()], &base)
            .expect_err("index beyond the resolved array");
        let message = error.to_string();
        assert!(message.contains("out of range"), "{message}");
        assert!(message.contains("app.windows.2"), "{message}");
    }

    #[test]
    fn a_patch_with_an_indexed_path_is_valid_toml() {
        let base = serde_json::json!({
            "app": {
                "windows": [{ "label": "main", "title": null, "width": 960 }]
            }
        });
        let patch = overrides_json(&["app.windows.0.width=800".to_owned()], &base)
            .expect("patch")
            .expect("some");
        assert!(!patch.contains("null"), "{patch}");
        serde_json::from_str::<toml::Value>(&patch)
            .expect("the patch must deserialize as the TOML value the child reads");
    }

    #[test]
    fn rejects_a_null_override_with_a_clear_error() {
        let error = overrides_json(
            &["app.default-window=null".to_owned()],
            &serde_json::json!({}),
        )
        .expect_err("TOML has no null");
        assert!(error.to_string().contains("null"), "{error}");
    }

    #[test]
    fn selects_the_app_binary_not_the_last_artifact() {
        let output = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"app","kind":["bin"]},"executable":"/tmp/app"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"helper","kind":["bin"]},"executable":"/tmp/helper"}"#,
        );
        assert_eq!(
            select_binary(output, "app").expect("app"),
            Path::new("/tmp/app")
        );
    }

    #[test]
    fn reports_when_the_expected_binary_is_absent() {
        let output = r#"{"reason":"compiler-artifact","target":{"name":"helper","kind":["bin"]},"executable":"/tmp/helper"}"#;
        let error = select_binary(output, "app").expect_err("missing app");
        assert!(error.to_string().contains("app"), "{error}");
        assert!(error.to_string().contains("helper"), "{error}");
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
    fn ignores_build_and_editor_artifacts() {
        let app = Path::new("demo");
        assert!(is_ignored_build_event(
            Path::new("demo/target/debug/demo"),
            app
        ));
        assert!(is_ignored_build_event(Path::new("demo/.git/index"), app));
        assert!(is_ignored_build_event(Path::new("demo/src/main.rs~"), app));
        assert!(is_ignored_build_event(Path::new("demo/src/.#main.rs"), app));
        assert!(!is_ignored_build_event(Path::new("demo/src/main.rs"), app));
        assert!(!is_ignored_build_event(Path::new("demo/Auro.toml"), app));
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
    fn ignores_editor_temporary_files() {
        let paths = hot_paths(vec![
            PathBuf::from("/app/data/src/lib.rs"),
            PathBuf::from("/app/ui/src/lib.rs.swp"),
            PathBuf::from("/app/ui/src/.#lib.rs"),
            PathBuf::from("/app/ui/Cargo.toml"),
        ]);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/app/data/src/lib.rs"),
                PathBuf::from("/app/ui/Cargo.toml"),
            ]
        );
        assert!(hot_paths(vec![PathBuf::from("/app/Auro.toml.lock")]).is_empty());
    }

    #[test]
    fn classifies_saves_outside_the_boundary() {
        let dir = Path::new("/app");
        let boundary = Path::new("/app/data/src");
        assert_eq!(
            outside_boundary_change(
                &[PathBuf::from("/app/data/src/lib.rs")],
                dir,
                boundary,
                true
            ),
            None
        );
        assert_eq!(
            outside_boundary_change(&[PathBuf::from("/app/ui/src/lib.rs")], dir, boundary, true),
            Some("ui/src/lib.rs".to_owned())
        );
        assert_eq!(
            outside_boundary_change(
                &[PathBuf::from("/app/types/src/lib.rs")],
                dir,
                boundary,
                true
            ),
            Some("types/src/lib.rs".to_owned())
        );
        // A project without the generated data crate keeps the old behaviour.
        assert_eq!(
            outside_boundary_change(&[PathBuf::from("/app/ui/src/lib.rs")], dir, boundary, false),
            None
        );
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

    #[cfg(unix)]
    #[test]
    fn host_restart_keeps_the_previous_app_until_the_replacement_starts() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("auro-restart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fake_dx = dir.join("dx");
        std::fs::write(&fake_dx, "#!/bin/sh\nsleep 30\n").expect("fake dx");
        let mut permissions = std::fs::metadata(&fake_dx).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_dx, permissions).expect("chmod");

        let old_dx = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30")
            .spawn()
            .expect("old devserver");
        let slot = Arc::new(Mutex::new(Some(old_dx)));
        let mut app = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("old app");
        let app_id = app.id();
        let app_pid = Arc::new(Mutex::new(Some(app_id)));
        let (events, _received) = mpsc::channel();

        let previous = restart_devserver(&fake_dx, &dir, None, &slot, &app_pid, &events)
            .expect("restart spawns the replacement");
        assert_eq!(previous, Some(app_id));
        assert!(
            app.try_wait().expect("app status").is_none(),
            "the previous app must stay alive while the replacement builds"
        );
        assert_eq!(*app_pid.lock().expect("app pid"), Some(app_id));
        assert!(
            slot.lock().expect("dev slot").is_some(),
            "the replacement devserver must be installed"
        );

        stop(&slot);
        kill_process(app_id);
        let _ = app.wait();
        let _ = std::fs::remove_dir_all(&dir);
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
