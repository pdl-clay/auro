// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use auro_core::{AuroError, Config};
use notify::{RecursiveMode, Watcher};

/// Builds, spawns and watches the app. A change rebuilds and relaunches; Ctrl-C leaves nothing
/// behind.
pub fn run(dir: PathBuf, overrides: &[String]) -> Result<(), AuroError> {
    let dir = dir.canonicalize()?;
    let config_path = dir.join(auro_core::config::CONFIG_FILE);
    let config = Config::load_from(&config_path)?;
    let patch = overrides_json(overrides)?;

    let mut binary = build(&dir, &config)?;
    println!("auro: running {}", binary.display());
    let slot: Arc<Mutex<Option<Child>>> =
        Arc::new(Mutex::new(Some(spawn(&binary, &dir, patch.as_deref())?)));

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
        match received.recv_timeout(Duration::from_millis(250)) {
            Ok(_) => {
                // Coalesce the burst of events one save produces.
                std::thread::sleep(Duration::from_millis(200));
                while received.try_recv().is_ok() {}

                println!("auro: change detected, rebuilding");
                stop(&slot);
                match build(&dir, &config) {
                    Ok(next) => {
                        binary = next;
                        let child = spawn(&binary, &dir, patch.as_deref())?;
                        println!("auro: rebuilt, running {}", binary.display());
                        *slot.lock().expect("dev slot poisoned") = Some(child);
                    }
                    Err(error) => eprintln!("auro: build failed, window closed: {error}"),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
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
        return Err(AuroError::Message(
            "cargo build failed; see the output above".to_owned(),
        ));
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

fn spawn(binary: &Path, dir: &Path, patch: Option<&str>) -> Result<Child, AuroError> {
    let mut command = Command::new(binary);
    command.current_dir(dir);
    if let Some(patch) = patch {
        command.env(auro_core::config::OVERRIDES_ENV, patch);
    }
    command
        .spawn()
        .map_err(|error| AuroError::Message(format!("cannot start {}: {error}", binary.display())))
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
}
