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

    println!("gpui: {}", gpui_pin(&dir));

    let config_path = dir.join(auro_core::config::CONFIG_FILE);
    let config = Config::load_from(&config_path)?;
    println!("hot reload: {}", hot_status(&dir, &config));
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

/// Whether `[dev] hot = true` can work here: the manifest feature and the devserver it drives.
fn hot_status(dir: &Path, config: &Config) -> String {
    match crate::project::declares_hot_feature(dir) {
        Ok(true) => {}
        Ok(false) => {
            return "not available: Cargo.toml has no `hot-reload` feature".to_owned();
        }
        Err(error) => return format!("unknown: {error}"),
    }

    let dx = std::env::var_os("AURO_DX")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("dx"));
    let version = Command::new(&dx)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());

    match (version, config.dev.hot) {
        (Some(version), true) => format!("enabled ({version})"),
        (Some(version), false) => format!("available ({version}); set `[dev] hot = true`"),
        (None, _) => format!(
            "not available: {} is missing (install dioxus-cli 0.7.10 or set AURO_DX)",
            dx.display()
        ),
    }
}

/// Keys this build accepts and the renderer cannot express yet.
fn unhonored(config: &Config) -> Vec<String> {
    let mut notes = Vec::new();
    for window in &config.app.windows {
        if window.max_width.is_some() || window.max_height.is_some() {
            notes.push(format!(
                "app.windows.{}.max-width/max-height: constrains the initial size only; GPUI 0.2 does not enforce runtime limits",
                window.label
            ));
        }
        if window.theme != Theme::System {
            notes.push(format!(
                "app.windows.{}.theme: accepted; apply it in App::setup (the chosen toolkit owns themes)",
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

/// The `gpui` version the app resolves, read from its lockfile.
///
/// The CLI cannot link GPUI (and should not); the lockfile is what cargo will actually build.
fn gpui_pin(dir: &Path) -> String {
    let lock = dir.join("Cargo.lock");
    let Ok(text) = std::fs::read_to_string(&lock) else {
        return format!(
            "not resolved ({} missing; run `cargo build`)",
            lock.display()
        );
    };
    let Ok(lock) = toml::from_str::<toml::Value>(&text) else {
        return format!("unknown ({} is not valid TOML)", lock.display());
    };

    let mut versions: Vec<&str> = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some("gpui"))
        .filter_map(|package| package.get("version").and_then(toml::Value::as_str))
        .collect();
    versions.sort_unstable();
    versions.dedup();

    match versions.as_slice() {
        [] => "not in Cargo.lock".to_owned(),
        [version] => (*version).to_owned(),
        versions => versions.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_non_system_theme_as_toolkit_owned() {
        let config: Config = toml::from_str(
            r#"
[app]
identifier = "com.example.theme"
product-name = "Theme"
version = "0.1.0"

[[app.windows]]
label = "main"
theme = "dark"
"#,
        )
        .expect("config");

        let notes = unhonored(&config);
        assert!(
            notes.iter().any(|note| note.contains("App::setup")),
            "{notes:?}"
        );
    }

    #[test]
    fn reads_the_gpui_pin_from_the_lockfile() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.lock"),
            r#"
version = 4

[[package]]
name = "gpui"
version = "0.2.2"

[[package]]
name = "serde"
version = "1.0.0"
"#,
        )
        .expect("lockfile");
        assert_eq!(gpui_pin(temp.path()), "0.2.2");
    }

    #[test]
    fn reports_a_missing_lockfile_instead_of_guessing() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(gpui_pin(temp.path()).contains("Cargo.lock"));
    }
}
