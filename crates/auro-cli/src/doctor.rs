// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::Path;
use std::process::Command;

use auro_core::{AuroError, Config, Theme};

/// Prints what the app will actually get: toolchain, config, platform, native libraries, and the
/// config keys the current platform cannot honor.
pub fn run(dir: &Path) -> Result<(), AuroError> {
    let dir = dir.canonicalize()?;

    println!("auro {}", env!("CARGO_PKG_VERSION"));
    println!(
        "platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    for tool in ["rustc", "cargo"] {
        match Command::new(tool).arg("--version").output() {
            Ok(output) if output.status.success() => {
                println!("{tool}: {}", String::from_utf8_lossy(&output.stdout).trim());
            }
            _ => println!("{tool}: MISSING"),
        }
    }

    let config_path = dir.join(auro_core::config::CONFIG_FILE);
    let config = Config::load_from(&config_path)?;
    println!(
        "config: {} ok, {} window(s)",
        config_path.display(),
        config.app.windows.len()
    );

    for note in unhonored(&config) {
        println!("  note: {note}");
    }

    if cfg!(target_os = "linux") {
        println!("linux libraries:");
        for library in [
            "wayland-client",
            "xkbcommon",
            "x11",
            "xcb",
            "vulkan",
            "alsa",
            "fontconfig",
            "openssl",
        ] {
            let present = Command::new("pkg-config")
                .args(["--exists", library])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            println!("  {library}: {}", if present { "ok" } else { "MISSING" });
        }
    }

    Ok(())
}

/// Keys this build accepts and the renderer cannot express yet.
fn unhonored(config: &Config) -> Vec<String> {
    let mut notes = Vec::new();
    for window in &config.app.windows {
        if window.max_width.is_some() || window.max_height.is_some() {
            notes.push(format!(
                "app.windows.{}.max-width/max-height: accepted, not honored by GPUI 0.2",
                window.label
            ));
        }
        if window.always_on_top {
            notes.push(format!(
                "app.windows.{}.always-on-top: accepted, not honored by GPUI 0.2",
                window.label
            ));
        }
        if window.theme != Theme::System {
            notes.push(format!(
                "app.windows.{}.theme: accepted, not honored by GPUI 0.2",
                window.label
            ));
        }
        if !window.decorations && !cfg!(target_os = "linux") {
            notes.push(format!(
                "app.windows.{}.decorations = false: only honored on Wayland",
                window.label
            ));
        }
    }
    notes
}
