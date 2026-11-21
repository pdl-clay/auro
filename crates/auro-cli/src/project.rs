// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use auro_core::{AuroError, Config};

const CARGO_TOML: &str = include_str!("../templates/Cargo.toml.in");
const AURO_TOML: &str = include_str!("../templates/Auro.toml");
const DIOXUS_TOML: &str = include_str!("../templates/Dioxus.toml");
const MAIN_RS: &str = include_str!("../templates/main.rs");
const UI_CARGO_TOML: &str = include_str!("../templates/ui/Cargo.toml.in");
const UI_LIB_RS: &str = include_str!("../templates/ui/lib.rs");
const SERVICE_RS: &str = include_str!("../templates/ui/services.rs");
const DATA_CARGO_TOML: &str = include_str!("../templates/data/Cargo.toml.in");
const DATA_LIB_RS: &str = include_str!("../templates/data/lib.rs");
const TYPES_CARGO_TOML: &str = include_str!("../templates/types/Cargo.toml.in");
const TYPES_LIB_RS: &str = include_str!("../templates/types/lib.rs");
const ICON_PNG: &[u8] = include_bytes!("../templates/icon.png");
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
    let crate_name = name.replace('-', "_");
    let identifier = format!("com.example.{}", name.replace('-', ""));
    let auro = auro_dependency(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        env!("CARGO_PKG_VERSION"),
    );
    let patch = xattr_patch();
    let fill = |template: &str| {
        template
            .replace("{{name}}", &name)
            .replace("{{crate}}", &crate_name)
            .replace("{{identifier}}", &identifier)
            .replace("{{auro_dependency}}", &auro)
            .replace("{{patch}}", &patch)
    };

    write(&path.join("Cargo.toml"), &fill(CARGO_TOML))?;
    write(&path.join("Auro.toml"), &fill(AURO_TOML))?;
    write(&path.join("Dioxus.toml"), &fill(DIOXUS_TOML))?;
    write(&path.join("src/main.rs"), &fill(MAIN_RS))?;
    write(&path.join("ui/Cargo.toml"), &fill(UI_CARGO_TOML))?;
    write(&path.join("ui/src/lib.rs"), &fill(UI_LIB_RS))?;
    write(&path.join("ui/src/services.rs"), &fill(SERVICE_RS))?;
    write(&path.join("data/Cargo.toml"), &fill(DATA_CARGO_TOML))?;
    write(&path.join("data/src/lib.rs"), &fill(DATA_LIB_RS))?;
    write(&path.join("types/Cargo.toml"), &fill(TYPES_CARGO_TOML))?;
    write(&path.join("types/src/lib.rs"), &fill(TYPES_LIB_RS))?;
    write_bytes(&path.join("icons/icon.png"), ICON_PNG)?;
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
    ensure_bundle_active(&config)?;
    build_release(&dir, &config)?;

    let binary = binary_name(&dir)?;
    let target = target_dir(&dir)?;
    let artifacts = auro_bundle::bundle_in(&dir, &config, &binary, &target)?;
    if let Some(profile) = config.bundle.macos.notarize_profile.as_deref() {
        println!("notarizing macOS artifacts with profile `{profile}`");
        auro_bundle::MacNotarizer::new().notarize_artifacts(&artifacts, profile)?;
    }
    if artifacts.is_empty() {
        return Err(AuroError::bundle(
            "the bundler produced no artifacts; check [bundle].targets",
        ));
    }
    for artifact in artifacts {
        println!("{}", artifact.display());
    }
    Ok(())
}

/// `auro bundle` is an explicit request, so an app that opted out is an error rather than a
/// silent success.
fn ensure_bundle_active(config: &Config) -> Result<(), AuroError> {
    if config.bundle.active {
        Ok(())
    } else {
        Err(AuroError::bundle(
            "`[bundle] active = false` disables bundling for this app",
        ))
    }
}

fn build_release(dir: &Path, _config: &Config) -> Result<(), AuroError> {
    let target = target_dir(dir)?;
    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--target-dir")
        .arg(&target)
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .status()?;

    if !status.success() {
        return Err(AuroError::Message("cargo build failed".to_owned()));
    }
    Ok(())
}

/// The Cargo target directory, resolved the way cargo resolves `CARGO_TARGET_DIR`.
///
/// `auro build`/`auro bundle` pass an explicit `--target-dir` to cargo and hand the same path to
/// the bundler, so a relative `CARGO_TARGET_DIR` cannot make the bundler look under
/// `<app>/target` for a binary cargo wrote somewhere else.
pub(crate) fn target_dir(dir: &Path) -> Result<PathBuf, AuroError> {
    Ok(resolve_target_dir(
        dir,
        std::env::var_os("CARGO_TARGET_DIR"),
        &std::env::current_dir()?,
    ))
}

fn resolve_target_dir(dir: &Path, value: Option<std::ffi::OsString>, cwd: &Path) -> PathBuf {
    match value {
        Some(value) => {
            let target = PathBuf::from(value);
            // `Path::is_absolute` is false on Windows for a Unix-style `/tmp` value; a leading
            // separator is rooted and should not be joined to the invocation directory.
            let rooted = target.is_absolute() || target.to_string_lossy().starts_with(['/', '\\']);
            if rooted { target } else { cwd.join(target) }
        }
        None => dir.join("target"),
    }
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

/// The executable cargo places in `target/release`.
///
/// Uses `cargo metadata`, so explicit `[[bin]]` targets, `default-run`, auto-discovered
/// `src/bin/*.rs` binaries and `autobins = false` all resolve the way Cargo resolves them. Several
/// binaries without `default-run` are ambiguous, so they are rejected instead of guessing.
pub fn binary_name(dir: &Path) -> Result<String, AuroError> {
    binary_name_with_args(dir, &[])
}

/// Resolves the executable with the feature set `cargo build` will receive.
///
/// `build_args` is the app's `[dev].build-args`; `--features`, `--all-features` and
/// `--no-default-features` select which `required-features` targets are enabled.
pub fn binary_name_with_args(dir: &Path, build_args: &[String]) -> Result<String, AuroError> {
    let manifest = dir.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|error| {
            AuroError::config(
                &manifest,
                None,
                format!("cannot run cargo metadata: {error}"),
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AuroError::config(
            &manifest,
            None,
            format!("cargo metadata failed: {}", stderr.trim()),
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let canonical_manifest = manifest.canonicalize().unwrap_or_else(|_| manifest.clone());
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find(|package| {
                package["manifest_path"]
                    .as_str()
                    .map(Path::new)
                    .and_then(|path| path.canonicalize().ok())
                    .is_some_and(|path| path == canonical_manifest)
            })
        })
        .ok_or_else(|| {
            AuroError::config(
                &manifest,
                None,
                "cargo metadata did not describe this package",
            )
        })?;

    let enabled = enabled_features(
        package["name"].as_str().unwrap_or_default(),
        package["features"].as_object(),
        build_args,
    );
    let bins: Vec<(&str, Vec<&str>)> = package["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| {
            let name = target["name"].as_str()?;
            let required = target["required-features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect();
            Some((name, required))
        })
        .collect();
    let enabled_bins: Vec<&str> = bins
        .iter()
        .filter(|(_, required)| required.iter().all(|feature| enabled.contains(*feature)))
        .map(|(name, _)| *name)
        .collect();

    if let Some(requested_bin) = parse_bin_args(build_args)? {
        if let Some((_, required)) = bins.iter().find(|(name, _)| *name == requested_bin) {
            if !required.iter().all(|feature| enabled.contains(*feature)) {
                return Err(AuroError::config(
                    &manifest,
                    None,
                    format!(
                        "--bin `{requested_bin}` requires feature(s) {}; enable them in [dev].build-args",
                        required.join(", ")
                    ),
                ));
            }
            return Ok(requested_bin);
        }
        return Err(AuroError::config(
            &manifest,
            None,
            format!("--bin `{requested_bin}` is not a binary target of this package"),
        ));
    }

    if let Some(default_run) = package["default_run"].as_str() {
        if enabled_bins.contains(&default_run) {
            return Ok(default_run.to_owned());
        }
        return Err(AuroError::config(
            &manifest,
            Some("package.default-run"),
            match bins.iter().find(|(name, _)| *name == default_run) {
                Some((_, required)) if !required.is_empty() => format!(
                    "default-run `{default_run}` requires feature(s) {}; enable them in [dev].build-args",
                    required.join(", ")
                ),
                _ => format!("default-run `{default_run}` is not an enabled binary target"),
            },
        ));
    }

    match enabled_bins.as_slice() {
        [] => {
            let gated: Vec<String> = bins
                .iter()
                .filter(|(_, required)| {
                    !required.is_empty()
                        && !required.iter().all(|feature| enabled.contains(*feature))
                })
                .map(|(name, required)| format!("`{name}` requires {}", required.join(", ")))
                .collect();
            let message = if gated.is_empty() {
                "package has no binary target; add src/main.rs, src/bin/*.rs or [[bin]]".to_owned()
            } else {
                format!(
                    "no binary target is enabled; {} (enable the feature in [dev].build-args)",
                    gated.join("; ")
                )
            };
            Err(AuroError::config(&manifest, None, message))
        }
        [only] => Ok((*only).to_owned()),
        several => Err(AuroError::config(
            &manifest,
            Some("package.default-run"),
            format!(
                "multiple enabled binary targets ({}); set package.default-run to choose the app binary",
                several.join(", ")
            ),
        )),
    }
}

fn parse_bin_args(build_args: &[String]) -> Result<Option<String>, AuroError> {
    let mut bins = Vec::new();
    let mut arguments = build_args.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bin" => {
                let value = arguments.next().ok_or_else(|| {
                    AuroError::Message("--bin in [dev].build-args needs a target name".to_owned())
                })?;
                bins.push(value.clone());
            }
            _ => {
                if let Some(value) = argument.strip_prefix("--bin=") {
                    bins.push(value.to_owned());
                }
            }
        }
    }
    match bins.len() {
        0 => Ok(None),
        1 => Ok(bins.pop()),
        _ => Err(AuroError::Message(format!(
            "[dev].build-args selects multiple binaries ({}); auro dev runs one",
            bins.join(", ")
        ))),
    }
}

fn parse_feature_args(build_args: &[String]) -> (bool, bool, Vec<String>) {
    let mut all = false;
    let mut no_default = false;
    let mut requested = Vec::new();
    let mut arguments = build_args.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--all-features" => all = true,
            "--no-default-features" => no_default = true,
            "--features" | "-F" => {
                if let Some(value) = arguments.next() {
                    requested.extend(split_features(value));
                }
            }
            _ => {
                if let Some(value) = argument
                    .strip_prefix("--features=")
                    .or_else(|| argument.strip_prefix("-F"))
                {
                    let value = value.strip_prefix('=').unwrap_or(value);
                    requested.extend(split_features(value));
                }
            }
        }
    }
    (all, no_default, requested)
}

fn split_features(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .filter(|feature| !feature.is_empty())
        .map(str::to_owned)
        .collect()
}

fn enabled_features(
    package_name: &str,
    features: Option<&serde_json::Map<String, serde_json::Value>>,
    build_args: &[String],
) -> HashSet<String> {
    let (all, no_default, requested) = parse_feature_args(build_args);
    let mut enabled = HashSet::new();
    let mut pending: Vec<String> = requested
        .iter()
        .filter_map(|feature| local_feature(feature, package_name))
        .collect();
    if !no_default {
        pending.push("default".to_owned());
    }
    if all && let Some(features) = features {
        pending.extend(features.keys().cloned());
    }
    while let Some(feature) = pending.pop() {
        if !enabled.insert(feature.clone()) {
            continue;
        }
        if let Some(members) = features
            .and_then(|features| features.get(&feature))
            .and_then(serde_json::Value::as_array)
        {
            for member in members.iter().filter_map(serde_json::Value::as_str) {
                if let Some(member) = local_feature(member, package_name) {
                    pending.push(member);
                }
            }
        }
    }
    enabled
}

/// Maps a Cargo feature token to a local feature, dropping dependency features.
///
/// `app/tool` names the local `tool` feature when `app` is the selected package; `dep/feature`
/// on another package is not a local feature. `dep:foo` enables a dependency, not a feature.
fn local_feature(token: &str, package_name: &str) -> Option<String> {
    if let Some((owner, feature)) = token.split_once('/') {
        (owner == package_name).then(|| feature.to_owned())
    } else if token.starts_with("dep:") {
        None
    } else {
        Some(token.to_owned())
    }
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
    let declared = package_version(dir, &manifest, &value)?;

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

/// The concrete `package.version`, following `version.workspace = true` to the workspace root.
fn package_version(dir: &Path, manifest: &Path, value: &toml::Value) -> Result<String, AuroError> {
    let version = value
        .get("package")
        .and_then(|package| package.get("version"))
        .ok_or_else(|| AuroError::config(manifest, Some("package.version"), "missing version"))?;

    if let Some(version) = version.as_str() {
        return Ok(version.to_owned());
    }
    if version
        .get("workspace")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
    {
        return inherited_workspace_version(dir).ok_or_else(|| {
            AuroError::config(
                manifest,
                Some("package.version"),
                "version.workspace = true, but no [workspace.package] version was found",
            )
        });
    }
    Err(AuroError::config(
        manifest,
        Some("package.version"),
        "must be a string or `{ workspace = true }`",
    ))
}

fn inherited_workspace_version(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        let Ok(text) = fs::read_to_string(ancestor.join("Cargo.toml")) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        if let Some(version) = value
            .get("workspace")
            .and_then(|workspace| workspace.get("package"))
            .and_then(|package| package.get("version"))
            .and_then(toml::Value::as_str)
        {
            return Some(version.to_owned());
        }
    }
    None
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
    write_bytes(path, contents.as_bytes())
}

fn write_bytes(path: &Path, contents: &[u8]) -> Result<(), AuroError> {
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
            "ui/Cargo.toml",
            "ui/src/lib.rs",
            "ui/src/services.rs",
            "data/Cargo.toml",
            "data/src/lib.rs",
            "types/Cargo.toml",
            "types/src/lib.rs",
            "icons/icon.png",
        ] {
            assert!(target.join(file).is_file(), "{file} missing");
        }

        let icon = std::fs::read(target.join("icons/icon.png")).expect("icon");
        assert!(icon.starts_with(b"\x89PNG\r\n\x1a\n"), "icon is a PNG");
        assert_eq!(
            u32::from_be_bytes([icon[16], icon[17], icon[18], icon[19]]),
            512,
            "the AppImage preflight needs a square PNG"
        );

        let config = Config::load_from(&target.join("Auro.toml")).expect("valid config");
        assert_eq!(config.app.product_name, "demo");
        assert_eq!(
            config.default_window().map(|window| window.label.as_str()),
            Some("main")
        );

        let manifest =
            std::fs::read_to_string(target.join("Cargo.toml")).expect("generated manifest");
        assert!(
            manifest.contains("members = [\".\", \"ui\", \"data\", \"types\"]"),
            "the scaffold is a four-crate workspace: {manifest}"
        );
        assert!(
            manifest.contains("[workspace]"),
            "the app must be its own workspace: {manifest}"
        );
        assert!(
            manifest.contains("strip = \"symbols\""),
            "release symbols are stripped by default: {manifest}"
        );
        let ui =
            std::fs::read_to_string(target.join("ui/Cargo.toml")).expect("generated ui manifest");
        assert!(
            ui.contains("auro = { path ="),
            "in-repo scaffolds point at the workspace: {ui}"
        );
        assert!(
            !ui.contains("demo-data"),
            "the ui crate must not depend on the boundary: {ui}"
        );
        let data = std::fs::read_to_string(target.join("data/Cargo.toml"))
            .expect("generated data manifest");
        assert!(
            !data.contains("auro ="),
            "the boundary crate stays framework-free: {data}"
        );
        let main = std::fs::read_to_string(target.join("src/main.rs")).expect("main");
        assert!(
            main.contains("demo_ui::run(demo_data::view_data)"),
            "the bin wires the boundary: {main}"
        );
        let dioxus = std::fs::read_to_string(target.join("Dioxus.toml")).expect("Dioxus.toml");
        assert!(
            dioxus.contains("name = \"demo\""),
            "dx needs the app name: {dioxus}"
        );

        // A leftover `{{identifier}}` would not compile in the generated app.
        let view = std::fs::read_to_string(target.join("ui/src/lib.rs")).expect("view");
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
    fn resolves_the_cargo_target_directory() {
        let app = Path::new("/app");
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_target_dir(app, None, cwd),
            Path::new("/app").join("target")
        );
        assert_eq!(
            resolve_target_dir(app, Some(std::ffi::OsString::from("/tmp/auro-target")), cwd),
            PathBuf::from("/tmp/auro-target")
        );
        assert_eq!(
            resolve_target_dir(app, Some(std::ffi::OsString::from("local-target")), cwd),
            Path::new("/work").join("local-target")
        );
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
    fn refuses_to_bundle_when_the_config_disables_it() {
        let temp = app_with_versions("0.1.0", "0.1.0");
        let mut config = std::fs::read_to_string(temp.path().join("Auro.toml")).expect("config");
        config.push_str("\n[bundle]\nactive = false\n");
        std::fs::write(temp.path().join("Auro.toml"), config).expect("config");

        let error = bundle(temp.path()).expect_err("bundling is disabled");
        assert!(error.to_string().contains("active = false"), "{error}");
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

    fn write_source(dir: &Path, relative: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("source dir");
        }
        std::fs::write(path, "fn main() {}\n").expect("source file");
    }

    #[test]
    fn reads_the_binary_name_from_the_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_source(temp.path(), "src/main.rs");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        assert_eq!(binary_name(temp.path()).expect("name"), "demo");

        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\ndefault-run = \"server\"\n\n[[bin]]\nname = \"server\"\npath = \"src/main.rs\"\n",
        )
        .expect("manifest");
        assert_eq!(binary_name(temp.path()).expect("default-run"), "server");

        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"pkg-name\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"custom-app\"\npath = \"src/main.rs\"\n",
        )
        .expect("manifest");
        assert_eq!(
            binary_name(temp.path()).expect("explicit bin"),
            "custom-app"
        );
    }

    #[test]
    fn finds_an_auto_discovered_src_bin_binary() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"pkg-name\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        write_source(temp.path(), "src/bin/app.rs");
        assert_eq!(binary_name(temp.path()).expect("src/bin app"), "app");
    }

    #[test]
    fn required_features_are_honored_when_selecting_a_binary() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[package]
name = "rf-app"
version = "0.1.0"
edition = "2024"

[features]
tool = []

[[bin]]
name = "tool"
path = "src/bin/tool.rs"
required-features = ["tool"]
"#,
        )
        .expect("manifest");
        write_source(temp.path(), "src/main.rs");
        write_source(temp.path(), "src/bin/tool.rs");
        assert_eq!(binary_name(temp.path()).expect("app"), "rf-app");

        let error =
            binary_name_with_args(temp.path(), &["--features".to_owned(), "tool".to_owned()])
                .expect_err("two enabled binaries");
        assert!(error.to_string().contains("rf-app"), "{error}");
        assert!(error.to_string().contains("tool"), "{error}");
        assert!(!error.to_string().contains("required-features"), "{error}");
    }

    #[test]
    fn an_explicit_bin_build_arg_selects_the_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[package]
name = "two-bins"
version = "0.1.0"
edition = "2024"
default-run = "one"

[[bin]]
name = "one"
path = "src/one.rs"

[[bin]]
name = "two"
path = "src/two.rs"
"#,
        )
        .expect("manifest");
        write_source(temp.path(), "src/one.rs");
        write_source(temp.path(), "src/two.rs");

        assert_eq!(binary_name(temp.path()).expect("default-run"), "one");
        assert_eq!(
            binary_name_with_args(temp.path(), &["--bin".to_owned(), "two".to_owned()])
                .expect("explicit bin"),
            "two"
        );
        assert_eq!(
            binary_name_with_args(temp.path(), &["--bin=two".to_owned()]).expect("inline bin"),
            "two"
        );
        assert!(
            binary_name_with_args(
                temp.path(),
                &[
                    "--bin".to_owned(),
                    "one".to_owned(),
                    "--bin".to_owned(),
                    "two".to_owned()
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn a_sole_gated_binary_requires_its_feature() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[package]
name = "gated"
version = "0.1.0"

[features]
tool = []

[[bin]]
name = "tool"
path = "src/bin/tool.rs"
required-features = ["tool"]
"#,
        )
        .expect("manifest");
        write_source(temp.path(), "src/bin/tool.rs");

        let error = binary_name(temp.path()).expect_err("feature disabled");
        assert!(error.to_string().contains("tool"), "{error}");
        assert!(error.to_string().contains("requires"), "{error}");

        assert_eq!(
            binary_name_with_args(temp.path(), &["--features=tool".to_owned()])
                .expect("feature enabled"),
            "tool"
        );
        assert_eq!(
            binary_name_with_args(temp.path(), &["--all-features".to_owned()])
                .expect("all features"),
            "tool"
        );
        for args in [
            vec!["-F".to_owned(), "tool".to_owned()],
            vec!["-Ftool".to_owned()],
            vec!["-F=tool".to_owned()],
            vec!["--features".to_owned(), "gated/tool".to_owned()],
        ] {
            assert_eq!(
                binary_name_with_args(temp.path(), &args).expect("feature alias"),
                "tool",
                "{args:?}"
            );
        }
        assert!(
            binary_name_with_args(
                temp.path(),
                &["--features".to_owned(), "other-package/tool".to_owned()]
            )
            .is_err(),
            "a dependency feature must not enable a local required-feature"
        );
    }

    #[test]
    fn refuses_to_guess_between_several_binaries() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"one\"\npath = \"src/main.rs\"\n\n[[bin]]\nname = \"two\"\npath = \"src/two.rs\"\n",
        )
        .expect("manifest");
        write_source(temp.path(), "src/main.rs");
        write_source(temp.path(), "src/two.rs");
        let error = binary_name(temp.path()).expect_err("ambiguous");
        assert!(error.to_string().contains("one"), "{error}");
        assert!(error.to_string().contains("two"), "{error}");
        assert!(error.to_string().contains("default-run"), "{error}");
    }

    #[test]
    fn accepts_a_workspace_inherited_package_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Auro.toml"),
            r#"
[app]
identifier = "com.example.inherited"
product-name = "Inherited"
version = "0.1.0"

[[app.windows]]
label = "main"
"#,
        )
        .expect("config");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[package]
name = "inherited"
version.workspace = true

[workspace]
members = ["."]

[workspace.package]
version = "0.1.0"
"#,
        )
        .expect("manifest");

        load_checked(temp.path()).expect("workspace version matches");
    }
}
