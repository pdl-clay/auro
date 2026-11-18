// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process-level benchmark harness for an Auro app.
//!
//! Startup is measured from `Command::spawn` to the app's `auro:startup` marker (first-frame
//! request). Idle RSS is sampled after the requested quiet period, then the process is killed. The
//! release binary size is reported alongside the medians.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use auro_core::AuroError;
use sysinfo::{Pid, ProcessesToUpdate, System};

const STARTUP_MARKER: &str = "auro:startup";
const STARTUP_TRACE_ENV: &str = "AURO_STARTUP_TRACE";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

struct Run {
    startup_ms: f64,
    rss_mb: f64,
}

/// Builds the app in release mode and measures it.
pub fn run(dir: &Path, runs: u32, idle_seconds: u64, json: bool) -> Result<(), AuroError> {
    if runs == 0 {
        return Err(AuroError::Message("runs must be at least 1".to_owned()));
    }
    let dir = dir.canonicalize()?;
    crate::project::build(&dir)?;
    let binary = binary_path(&dir)?;
    let binary_bytes = std::fs::metadata(&binary)?.len();
    let mut measurements = Vec::with_capacity(runs as usize);
    for _ in 0..runs {
        measurements.push(run_once(&binary, &dir, idle_seconds)?);
    }
    let startup_samples: Vec<f64> = measurements.iter().map(|run| run.startup_ms).collect();
    let rss_samples: Vec<f64> = measurements.iter().map(|run| run.rss_mb).collect();
    let startup = summarize(&startup_samples);
    let rss = summarize(&rss_samples);
    let app = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());

    if json {
        let value = serde_json::json!({
            "app": app,
            "binary": binary,
            "binary_bytes": binary_bytes,
            "runs": runs,
            "idle_seconds": idle_seconds,
            "startup_ms_min": startup.min,
            "startup_ms_median": startup.median,
            "startup_ms_p95": startup.p95,
            "startup_ms_max": startup.max,
            "startup_ms_samples": startup_samples,
            "rss_mb_min": rss.min,
            "rss_mb_median": rss.median,
            "rss_mb_p95": rss.p95,
            "rss_mb_max": rss.max,
            "rss_mb_samples": rss_samples,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("app: {app}");
        println!("binary: {} ({binary_bytes} bytes)", binary.display());
        println!("runs: {runs} (idle {idle_seconds}s)");
        println!(
            "startup_ms: min {:.1}, median {:.1}, p95 {:.1}, max {:.1}",
            startup.min, startup.median, startup.p95, startup.max
        );
        println!(
            "rss_mb: min {:.1}, median {:.1}, p95 {:.1}, max {:.1}",
            rss.min, rss.median, rss.p95, rss.max
        );
    }
    Ok(())
}

fn binary_path(dir: &Path) -> Result<std::path::PathBuf, AuroError> {
    let name = crate::project::binary_name(dir)?;
    let mut binary = crate::project::target_dir(dir)?.join("release").join(name);
    if cfg!(windows) && binary.extension().is_none() {
        binary.set_extension("exe");
    }
    if !binary.is_file() {
        return Err(AuroError::Message(format!(
            "release binary not found at {}",
            binary.display()
        )));
    }
    Ok(binary)
}

fn run_once(binary: &Path, dir: &Path, idle_seconds: u64) -> Result<Run, AuroError> {
    let start = Instant::now();
    let mut child = Command::new(binary)
        .current_dir(dir)
        .env(STARTUP_TRACE_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AuroError::Message("app stdout was not captured".to_owned()))?;
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if line == STARTUP_MARKER {
                let _ = ready_tx.send(());
                break;
            }
        }
    });

    let startup = match ready_rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(()) => start.elapsed(),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AuroError::Message(format!(
                "app did not print `{STARTUP_MARKER}` within {STARTUP_TIMEOUT:?}"
            )));
        }
    };
    if let Some(status) = child.try_wait()? {
        return Err(AuroError::Message(format!(
            "app exited with {status} after `{STARTUP_MARKER}`; no idle sample"
        )));
    }
    thread::sleep(Duration::from_secs(idle_seconds));
    if let Some(status) = child.try_wait()? {
        return Err(AuroError::Message(format!(
            "app exited with {status} during the {idle_seconds}s idle window; no idle sample"
        )));
    }
    let pid = Pid::from_u32(child.id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let rss_bytes = system
        .process(pid)
        .map(sysinfo::Process::memory)
        .filter(|bytes| *bytes > 0);
    let Some(rss_bytes) = rss_bytes else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AuroError::Message(format!(
            "could not read RSS for process {} after the idle window",
            child.id()
        )));
    };
    let _ = child.kill();
    let _ = child.wait();

    Ok(Run {
        startup_ms: startup.as_secs_f64() * 1000.0,
        rss_mb: rss_bytes as f64 / (1024.0 * 1024.0),
    })
}

struct Summary {
    min: f64,
    median: f64,
    p95: f64,
    max: f64,
}

fn summarize(values: &[f64]) -> Summary {
    assert!(
        !values.is_empty(),
        "a benchmark run yields at least one sample"
    );
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Summary {
        min: sorted[0],
        median: median(&sorted),
        p95: percentile(&sorted, 95.0),
        max: sorted[sorted.len() - 1],
    }
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

/// Nearest-rank percentile; the input must be sorted.
fn percentile(sorted: &[f64], percent: f64) -> f64 {
    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_odd_and_even_samples() {
        let summary = summarize(&[3.0, 1.0, 2.0]);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.median, 2.0);
        assert_eq!(summary.p95, 3.0);
        assert_eq!(summary.max, 3.0);

        let summary = summarize(&[4.0, 1.0, 3.0, 2.0]);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.median, 2.5);
        assert_eq!(summary.p95, 4.0);
        assert_eq!(summary.max, 4.0);
    }

    #[test]
    fn percentile_uses_the_nearest_rank() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&values, 50.0), 5.0);
        assert_eq!(percentile(&values, 95.0), 10.0);
        assert_eq!(percentile(&[1.0, 2.0], 95.0), 2.0);
    }
}
