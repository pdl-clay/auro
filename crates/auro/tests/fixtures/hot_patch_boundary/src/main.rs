// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal stdio-driven hot-patch fixture for `hot_patch_e2e`.
//!
//! `frame` is the reloadable boundary: it returns plain data and is called through
//! `auro::hot::hot`. `src/main_patched.rs` is identical except for the value returned by `frame`;
//! dx compiles that version into a dylib after the baseline fat binary is built.

use std::error::Error;
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn frame() -> String {
    "one".to_owned()
}

fn emit(line: impl std::fmt::Display) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn show_frame() {
    emit(format_args!("title-{}", auro::hot::hot(frame)));
}

fn main() -> Result<(), Box<dyn Error>> {
    // The test driver sets AURO_HOT_RELOAD=1; set_var is unsafe in edition 2024.
    auro::hot::install();

    let mut counter = 0_u64;
    let mut registration: Option<(u64, Arc<AtomicBool>)> = None;
    let mut callback: Option<Box<dyn Fn()>> = None;

    show_frame();
    emit(format_args!("counter-{counter}"));
    emit(format_args!("revision-{}", auro::hot::revision()));
    emit("ready");

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let (command, argument) = line.split_once(':').unwrap_or((&line, ""));
        match command {
            "bump" => {
                counter += 1;
                emit(format_args!("counter-{counter}"));
            }
            "frame" => show_frame(),
            "state" => emit(format_args!("counter-{counter}")),
            "register" => {
                let old_state = Arc::new(AtomicBool::new(false));
                let captured = Arc::clone(&old_state);
                callback = Some(Box::new(move || {
                    captured.store(true, Ordering::SeqCst);
                }));
                registration = Some((auro::hot::revision(), old_state));
                emit("registered");
            }
            "invoke-stale" => {
                let Some((revision, old_state)) = registration
                    .as_ref()
                    .map(|(revision, state)| (*revision, Arc::clone(state)))
                else {
                    emit("no-registration");
                    continue;
                };
                let stale = auro::hot::is_stale(revision);
                if stale {
                    // Old-boundary code must not run against state it owned.
                    emit("stale-cleared");
                    emit(if old_state.load(Ordering::SeqCst) {
                        "old-state-touched"
                    } else {
                        "old-state-untouched"
                    });
                } else if let Some(callback) = callback.as_ref() {
                    callback();
                    emit("callback-ran");
                } else {
                    emit("callback-missing");
                }
                callback = None;
            }
            "patch" => match auro::hot::apply_patch_from_path(Path::new(argument)) {
                Ok(_) => {
                    emit("patch-applied");
                    emit(format_args!("revision-{}", auro::hot::revision()));
                    show_frame();
                    emit(format_args!("counter-{counter}"));
                }
                Err(error) => emit(format_args!("patch-error:{error}")),
            },
            "exit" => break,
            other => emit(format_args!("unknown-{other}")),
        }
    }
    Ok(())
}
