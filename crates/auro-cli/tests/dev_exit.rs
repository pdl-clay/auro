// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const DEATH_TIMEOUT: Duration = Duration::from_secs(10);

/// Kills every process the test started, even when an assertion panics first.
struct Cleanup {
    cli: Option<Child>,
    pids: Vec<u32>,
    pid_files: Vec<PathBuf>,
}

impl Cleanup {
    fn new(cli: Child, pid_files: Vec<PathBuf>) -> Self {
        Self {
            cli: Some(cli),
            pids: Vec::new(),
            pid_files,
        }
    }

    fn signal(&self, name: &str) {
        if let Some(child) = &self.cli {
            let _ = Command::new("kill")
                .arg(format!("-{name}"))
                .arg(child.id().to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    fn reap_within(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.cli.as_mut()?.try_wait().expect("try_wait");
            if status.is_some() {
                self.cli = None;
                return status;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.signal("KILL");
        if let Some(mut child) = self.cli.take() {
            let _ = child.wait();
        }
        for file in &self.pid_files {
            if let Some(pid) = read_pid(file) {
                kill(pid);
            }
        }
        for pid in self.pids.drain(..) {
            kill(pid);
        }
    }
}

fn kill(pid: u32) {
    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A zombie has already exited; its pid cannot hold a live app window.
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z");
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps");
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        !state.is_empty() && !state.starts_with('Z') && !state.starts_with('U')
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn wait_until(deadline: Instant, mut condition: impl FnMut() -> bool) {
    while Instant::now() < deadline && !condition() {
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn collect_lines(stream: impl Read + Send + 'static, log: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            log.lock().expect("log lock").push(line);
        }
    });
}

fn write_project(root: &Path) {
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("Auro.toml"),
        r#"
[app]
identifier = "com.example.dxexit"
product-name = "DxExit"
version = "0.1.0"

[[app.windows]]
label = "main"

[dev]
hot = true
"#,
    )
    .expect("Auro.toml");
    std::fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "exit-fixture"
version = "0.1.0"
edition = "2021"

[features]
hot-reload = ["auro/hot-reload"]
"#,
    )
    .expect("Cargo.toml");
}

/// Stands in for `dx`: answers the version probe, then owns a child app like a real devserver.
fn write_fake_dx(path: &Path) {
    let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "dioxus 0.7.10 (57d6794)"
    exit 0
fi

/bin/sleep 30 &
app=$!
printf '%s\n' "$$" > "$FAKE_DX_PID_FILE"
printf '%s\n' "$app" > "$FAKE_DX_APP_PID_FILE"
echo "auro:trace pid $app"
wait "$app"
"#;
    std::fs::write(path, script).expect("write fake dx");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod fake dx");
}

#[test]
fn sigint_kills_dx_and_app_and_exits_zero() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_project(root);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let dx = bin.join("dx");
    write_fake_dx(&dx);
    let dx_pid_file = root.join("dx.pid");
    let app_pid_file = root.join("app.pid");

    let path = {
        let mut entries = vec![bin];
        if let Some(existing) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(entries).expect("PATH")
    };

    let mut child = Command::new(env!("CARGO_BIN_EXE_auro"))
        .arg("dev")
        .arg("--dir")
        .arg(root)
        .env("PATH", path)
        .env("FAKE_DX_PID_FILE", &dx_pid_file)
        .env("FAKE_DX_APP_PID_FILE", &app_pid_file)
        .env_remove("AURO_CONFIG")
        .env_remove("AURO_CONFIG_OVERRIDES")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn auro dev");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let mut cleanup = Cleanup::new(child, vec![dx_pid_file.clone(), app_pid_file.clone()]);
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    collect_lines(stdout, Arc::clone(&log));
    collect_lines(stderr, Arc::clone(&log));

    // Wait until the real dev loop has echoed the devserver's pid trace, not merely until the
    // fake dx wrote its files: the app pid must be in the slot the signal handler reads.
    let deadline = Instant::now() + START_TIMEOUT;
    let (dx_pid, app_pid) = loop {
        let recorded = match (read_pid(&dx_pid_file), read_pid(&app_pid_file)) {
            (Some(dx), Some(app)) => Some((dx, app)),
            _ => None,
        };
        if let Some((dx, app)) = recorded
            && log
                .lock()
                .expect("log lock")
                .iter()
                .any(|line| line.contains(&format!("auro:trace pid {app}")))
        {
            break (dx, app);
        }
        assert!(
            Instant::now() < deadline,
            "auro never recorded the app pid; log: {:?}",
            log.lock().expect("log lock")
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    cleanup.pids = vec![dx_pid, app_pid];

    assert!(alive(dx_pid), "devserver {dx_pid} was not running");
    assert!(alive(app_pid), "app {app_pid} was not running");

    cleanup.signal("INT");
    let status = cleanup
        .reap_within(EXIT_TIMEOUT)
        .expect("auro must exit after SIGINT");
    assert_eq!(
        status.code(),
        Some(0),
        "auro did not exit cleanly; log: {:?}",
        log.lock().expect("log lock")
    );

    let deadline = Instant::now() + DEATH_TIMEOUT;
    wait_until(deadline, || !alive(dx_pid) && !alive(app_pid));
    assert!(!alive(dx_pid), "devserver {dx_pid} survived SIGINT");
    assert!(!alive(app_pid), "app {app_pid} was orphaned by SIGINT");
}

#[test]
fn zombie_is_not_alive() {
    let mut child = Command::new("sleep").arg("30").spawn().expect("sleep");
    let pid = child.id();
    kill(pid);
    let deadline = Instant::now() + DEATH_TIMEOUT;
    wait_until(deadline, || !alive(pid));
    assert!(!alive(pid), "a killed zombie must not count as alive");
    let _ = child.wait();
}
