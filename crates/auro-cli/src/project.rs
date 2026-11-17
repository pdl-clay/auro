// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use auro_core::{AuroError, Config};

const CARGO_TOML: &str = include_str!("../templates/Cargo.toml");
const AURO_TOML: &str = include_str!("../templates/Auro.toml");
const MAIN_RS: &str = include_str!("../templates/main.rs");
const VIEW_RS: &str = include_str!("../templates/view.rs");
const SERVICE_RS: &str = include_str!("../templates/service.rs");
const GITIGNORE: &str = "/target\n";

/// Scaffolds an app. Refuses a directory that already has content.
pub fn new(path: &Path) -> Result<(), AuroError> {
    if path.exists() && fs::read_dir(path)?.next().is_some() {
        return Err(AuroError::Message(format!(
            "`{}` is not empty; refusing to scaffold over it",
            path.display()
        )));
    }

    fs::create_dir_all(path)?;
    let name = package_name(path);
    let identifier = format!("com.example.{}", name.replace('-', ""));

    write(
        &path.join("Cargo.toml"),
        &CARGO_TOML
            .replace("{{name}}", &name)
            .replace("{{auro_path}}", &auro_path())
            .replace("{{patch}}", &xattr_patch()),
    )?;
    write(
        &path.join("Auro.toml"),
        &AURO_TOML
            .replace("{{name}}", &name)
            .replace("{{identifier}}", &identifier),
    )?;
    write(&path.join("src/main.rs"), MAIN_RS)?;
    write(
        &path.join("src/views/mod.rs"),
        &VIEW_RS.replace("{{identifier}}", &identifier),
    )?;
    write(&path.join("src/services/mod.rs"), SERVICE_RS)?;
    write(&path.join(".gitignore"), GITIGNORE)?;

    println!("Created {} at {}", name, path.display());
    println!("Next: cd {} && auro dev", path.display());
    Ok(())
}

/// `cargo build --release`, after checking the config's version against the manifest.
pub fn build(dir: &Path) -> Result<(), AuroError> {
    let dir = dir.canonicalize()?;
    let config = Config::load_from(&dir.join(auro_core::config::CONFIG_FILE))?;
    check_version(&dir, &config)?;

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .status()?;

    if !status.success() {
        return Err(AuroError::Message("cargo build failed".to_owned()));
    }
    Ok(())
}

/// The app cannot see its manifest at runtime, so the version check belongs to the CLI.
pub fn check_version(dir: &Path, config: &Config) -> Result<(), AuroError> {
    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    let value: toml::Value = text.parse().map_err(|error: toml::de::Error| {
        AuroError::config(&manifest, None, format!("invalid Cargo.toml: {error}"))
    })?;
    let declared = value
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(|version| version.as_str())
        .unwrap_or_default();

    if declared != config.app.version {
        return Err(AuroError::config(
            &manifest,
            Some("package.version"),
            format!(
                "Cargo.toml says `{declared}` but Auro.toml app.version says `{}`",
                config.app.version
            ),
        ));
    }
    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<(), AuroError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn package_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "app".to_owned());

    let mut name: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    while name.contains("--") {
        name = name.replace("--", "-");
    }
    let name = name.trim_matches('-').to_owned();
    let name = if name.is_empty() {
        "app".to_owned()
    } else {
        name
    };

    if name.starts_with(|character: char| character.is_ascii_digit()) {
        format!("app-{name}")
    } else {
        name
    }
}

/// In-repo, the generated app points at the workspace. A published CLI would emit a version
/// requirement instead; that lands with the first release.
fn auro_path() -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo) = manifest.parent().and_then(Path::parent) {
        let candidate = repo.join("crates").join("auro");
        if candidate.is_dir() {
            return candidate.display().to_string();
        }
    }
    "0.0.0".to_owned()
}

/// The generated app is its own workspace, so it does not inherit this repository's
/// `[patch.crates-io]`. Emitted only while the checkout carries the patched crate.
fn xattr_patch() -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(repo) = manifest.parent().and_then(Path::parent) else {
        return String::new();
    };
    let vendored = repo.join("vendor").join("xattr");
    if !vendored.is_dir() {
        return String::new();
    }

    format!(
        "\n# Temporary: gpui -> gpui_http_client -> zed-async-tar -> xattr 0.2.3 does not\n\
         # compile against libc 0.2.191 on Linux (`ENOATTR` was removed). Drop this when\n\
         # the dependency chain stops pulling xattr 0.2.\n\
         [patch.crates-io]\n\
         xattr = {{ path = \"{}\" }}\n",
        vendored.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_a_new_app() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("demo");
        new(&target).expect("scaffolds");

        for file in [
            "Cargo.toml",
            "Auro.toml",
            "src/main.rs",
            "src/views/mod.rs",
            "src/services/mod.rs",
        ] {
            assert!(target.join(file).is_file(), "{file} missing");
        }

        let config = Config::load_from(&target.join("Auro.toml")).expect("valid config");
        assert_eq!(config.app.product_name, "demo");
        assert_eq!(
            config.default_window().map(|window| window.label.as_str()),
            Some("main")
        );

        // A leftover `{{identifier}}` would not compile in the generated app.
        let view = std::fs::read_to_string(target.join("src/views/mod.rs")).expect("view");
        assert!(
            view.contains("com.example.demo"),
            "identifier must reach the view: {view}"
        );
        assert!(
            !view.contains("{{identifier}}"),
            "placeholder must be substituted: {view}"
        );
    }

    #[test]
    fn refuses_a_non_empty_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("keep.txt"), "x").expect("write");
        let error = new(temp.path()).expect_err("refuses");
        assert!(error.to_string().contains("not empty"), "{error}");
    }

    #[test]
    fn normalises_names() {
        assert_eq!(package_name(Path::new("/tmp/My App!")), "my-app");
        assert_eq!(package_name(Path::new("/tmp/2cool")), "app-2cool");
    }
}
