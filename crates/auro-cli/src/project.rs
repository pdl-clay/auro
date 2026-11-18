// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use auro_core::{AuroError, Config};

const CARGO_TOML: &str = include_str!("../templates/Cargo.toml");
const AURO_TOML: &str = include_str!("../templates/Auro.toml");
const DIOXUS_TOML: &str = include_str!("../templates/Dioxus.toml");
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
            .replace(
                "{{auro_dependency}}",
                &auro_dependency(
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    env!("CARGO_PKG_VERSION"),
                ),
            )
            .replace("{{patch}}", &xattr_patch()),
    )?;
    write(
        &path.join("Auro.toml"),
        &AURO_TOML
            .replace("{{name}}", &name)
            .replace("{{identifier}}", &identifier),
    )?;
    write(
        &path.join("Dioxus.toml"),
        &DIOXUS_TOML.replace("{{name}}", &name),
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
    let config = load_checked(&dir)?;
    build_release(&dir, &config)
}

/// Builds the release binary and hands it to `auro-bundle`.
pub fn bundle(dir: &Path) -> Result<(), AuroError> {
    let dir = plain_path(dir.canonicalize()?);
    let config = load_checked(&dir)?;
    build_release(&dir, &config)?;

    let binary = binary_name(&dir)?;
    let artifacts = auro_bundle::bundle(&dir, &config, &binary)?;
    for artifact in artifacts {
        println!("{}", artifact.display());
    }
    Ok(())
}

fn build_release(dir: &Path, _config: &Config) -> Result<(), AuroError> {
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

/// Windows `canonicalize` returns a `\\?\` verbatim path; WiX/NSIS tools reject it.
fn plain_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// The executable cargo places in `target/release`: `default-run`, or the package name.
pub fn binary_name(dir: &Path) -> Result<String, AuroError> {
    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    let value: toml::Value = toml::from_str(&text).map_err(|error: toml::de::Error| {
        AuroError::config(&manifest, None, format!("invalid Cargo.toml: {error}"))
    })?;
    let package = value
        .get("package")
        .ok_or_else(|| AuroError::config(&manifest, None, "missing [package]"))?;
    let name = package
        .get("default-run")
        .or_else(|| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| AuroError::config(&manifest, Some("package.name"), "missing name"))?;
    Ok(name.to_owned())
}

/// Loads `Auro.toml` and rejects a version that disagrees with the manifest (spec 0002).
///
/// Both `auro build` and `auro dev` go through here: the running app cannot see its manifest.
pub fn load_checked(dir: &Path) -> Result<Config, AuroError> {
    let config = Config::load_from(&dir.join(auro_core::config::CONFIG_FILE))?;
    check_version(dir, &config)?;
    Ok(config)
}

/// The app cannot see its manifest at runtime, so the version check belongs to the CLI.
pub fn check_version(dir: &Path, config: &Config) -> Result<(), AuroError> {
    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    let value: toml::Value = toml::from_str(&text).map_err(|error: toml::de::Error| {
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

/// Whether the app's manifest declares the `hot-reload` feature `dx` enables.
///
/// `dx serve --features hot-reload` fails deep in cargo when the feature does not exist; this makes
/// the CLI name the missing manifest entry before it starts a build.
pub fn declares_hot_feature(dir: &Path) -> Result<bool, AuroError> {
    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    let value: toml::Value = toml::from_str(&text).map_err(|error: toml::de::Error| {
        AuroError::config(&manifest, None, format!("invalid Cargo.toml: {error}"))
    })?;
    Ok(value
        .get("features")
        .and_then(|features| features.get("hot-reload"))
        .is_some())
}

/// TOML basic strings treat `\` as an escape, and a Windows path full of backslashes breaks the
/// generated manifest (`\U` is a unicode escape). Cargo accepts forward slashes on every platform.
fn toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
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

/// In-repo, the generated app points at the workspace. An installed CLI emits the matching
/// crates.io version instead.
fn auro_dependency(manifest_dir: &Path, version: &str) -> String {
    if let Some(repo) = manifest_dir.parent().and_then(Path::parent) {
        let candidate = repo.join("crates").join("auro");
        if candidate.is_dir() {
            return format!("{{ path = \"{}\" }}", toml_path(&candidate));
        }
    }
    format!("\"{version}\"")
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
        toml_path(&vendored)
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
            "Dioxus.toml",
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

        let manifest =
            std::fs::read_to_string(target.join("Cargo.toml")).expect("generated manifest");
        assert!(
            manifest.contains("auro = { path ="),
            "in-repo scaffolds point at the workspace: {manifest}"
        );
        assert!(
            manifest.contains("[workspace]"),
            "the app must be its own workspace: {manifest}"
        );
        let dioxus = std::fs::read_to_string(target.join("Dioxus.toml")).expect("Dioxus.toml");
        assert!(
            dioxus.contains("name = \"demo\""),
            "dx needs the app name: {dioxus}"
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
    fn detects_the_hot_reload_feature() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[features]\nhot-reload = [\"auro/hot-reload\"]\n",
        )
        .expect("write");
        assert!(declares_hot_feature(temp.path()).expect("reads"));

        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .expect("write");
        assert!(!declares_hot_feature(temp.path()).expect("reads"));
    }

    #[test]
    fn generated_manifest_paths_use_forward_slashes() {
        assert_eq!(toml_path(Path::new(r"C:\Users\a\auro")), "C:/Users/a/auro");
        assert_eq!(toml_path(Path::new("/home/a/auro")), "/home/a/auro");
    }

    #[test]
    fn normalises_names() {
        assert_eq!(package_name(Path::new("/tmp/My App!")), "my-app");
        assert_eq!(package_name(Path::new("/tmp/2cool")), "app-2cool");
    }

    #[test]
    fn published_cli_uses_the_crate_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = temp.path().join("registry").join("auro-cli-0.1.0-alpha.1");
        std::fs::create_dir_all(&manifest).expect("manifest dir");
        assert_eq!(
            auro_dependency(&manifest, "0.1.0-alpha.1"),
            "\"0.1.0-alpha.1\""
        );
    }

    #[test]
    fn in_repo_cli_uses_the_workspace_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = temp.path().join("crates").join("auro-cli");
        std::fs::create_dir_all(temp.path().join("crates").join("auro")).expect("workspace crate");
        std::fs::create_dir_all(&manifest).expect("manifest dir");
        let dependency = auro_dependency(&manifest, "0.1.0-alpha.1");
        assert!(dependency.starts_with("{ path = \""), "{dependency}");
    }

    fn app_with_versions(config_version: &str, manifest_version: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Auro.toml"),
            format!(
                r#"
[app]
identifier = "com.example.versioned"
product-name = "Versioned"
version = "{config_version}"

[[app.windows]]
label = "main"
"#
            ),
        )
        .expect("config");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            format!(
                r#"
[package]
name = "versioned"
version = "{manifest_version}"
"#
            ),
        )
        .expect("manifest");
        temp
    }

    #[test]
    fn refuses_a_config_version_that_disagrees_with_the_manifest() {
        let temp = app_with_versions("9.9.9", "0.1.0");
        let error = load_checked(temp.path()).expect_err("version mismatch");
        let message = error.to_string();
        assert!(message.contains("9.9.9"), "{message}");
        assert!(message.contains("0.1.0"), "{message}");
    }

    #[test]
    fn accepts_matching_versions() {
        let temp = app_with_versions("0.1.0", "0.1.0");
        load_checked(temp.path()).expect("matching versions");
    }

    #[cfg(windows)]
    #[test]
    fn strips_the_windows_verbatim_prefix() {
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\C:\foo")),
            PathBuf::from(r"C:\foo")
        );
    }

    #[test]
    fn reads_the_binary_name_from_the_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        assert_eq!(binary_name(temp.path()).expect("name"), "demo");

        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\ndefault-run = \"server\"\n",
        )
        .expect("manifest");
        assert_eq!(binary_name(temp.path()).expect("default-run"), "server");
    }
}
