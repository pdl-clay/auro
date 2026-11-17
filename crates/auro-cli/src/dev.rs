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
    let config_path = dir.join(auro_core::config::CONFIG_FILE);
    let config = Config::load_from(&config_path)?;
    let patch = overrides_json(overrides)?;

    let first = build(&dir, &config)?;
    let mut generation = 1u64;
    let mut running = deploy(&first, &dir, generation)?;
    println!("auro: running {}", running.display());
    let (child_tx, child_rx) = mpsc::channel::<ChildEvent>();
    let slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(spawn(
        &running,
        &dir,
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
                                let child =
                                    spawn(&running, &dir, patch.as_deref(), child_tx.clone())?;
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

fn spawn(
    binary: &Path,
    dir: &Path,
    patch: Option<&str>,
    events: mpsc::Sender<ChildEvent>,
) -> Result<Child, AuroError> {
    let mut command = Command::new(binary);
    command.current_dir(dir);
    if let Some(patch) = patch {
        command.env(auro_core::config::OVERRIDES_ENV, patch);
    }
    command.env(TRACE_ENV, "1");
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
    if line.starts_with("auro: state restored") {
        return Some("restarted (state restored)".to_owned());
    }
    let reason = line.strip_prefix("auro: state reset: ")?;
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
}
