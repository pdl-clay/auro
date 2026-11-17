// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Auro.toml`: parsing, platform merge, validation.
//!
//! See `docs/src/specs/0002-config.md` for the contract.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use toml::Value;

use auro_ipc::AuroError;

/// The file an app author edits.
pub const CONFIG_FILE: &str = "Auro.toml";
/// Points at a different config file.
pub const CONFIG_ENV: &str = "AURO_CONFIG";
/// JSON Merge Patch set by `auro dev` and `auro build`.
pub const OVERRIDES_ENV: &str = "AURO_CONFIG_OVERRIDES";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    /// Editor hint; never read at runtime.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub app: AppConfig,
    #[serde(default)]
    pub dev: DevConfig,
    #[serde(default)]
    pub bundle: BundleConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppConfig {
    pub identifier: String,
    pub product_name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub default_window: Option<String>,
    #[serde(default)]
    pub windows: Vec<WindowConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowConfig {
    pub label: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub min_width: Option<u32>,
    #[serde(default)]
    pub min_height: Option<u32>,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub max_height: Option<u32>,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default = "default_true")]
    pub decorations: bool,
    #[serde(default)]
    pub transparent: bool,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_true")]
    pub center: bool,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub theme: Theme,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DevConfig {
    #[serde(default = "default_watch")]
    pub watch: Vec<String>,
    #[serde(default)]
    pub run_args: Vec<String>,
    #[serde(default)]
    pub build_args: Vec<String>,
    /// Run the hot-patch devserver (`dx serve --hot-patch`) instead of rebuilding and relaunching.
    /// Requires `dx` on `PATH` (or `AURO_DX`) and the app's `hot-reload` feature.
    #[serde(default)]
    pub hot: bool,
}

impl Default for DevConfig {
    fn default() -> Self {
        DevConfig {
            watch: default_watch(),
            run_args: Vec::new(),
            build_args: Vec::new(),
            hot: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BundleConfig {
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub targets: BundleTargets,
    #[serde(default)]
    pub icon: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub short_description: String,
    #[serde(default)]
    pub long_description: String,
    #[serde(default)]
    pub copyright: String,
    #[serde(default)]
    pub license_file: String,
}

impl Default for BundleConfig {
    fn default() -> Self {
        BundleConfig {
            active: true,
            targets: BundleTargets::default(),
            icon: Vec::new(),
            resources: Vec::new(),
            category: default_category(),
            short_description: String::new(),
            long_description: String::new(),
            copyright: String::new(),
            license_file: String::new(),
        }
    }
}

/// `"all"` or an explicit list of package formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BundleTargets {
    Keyword(String),
    List(Vec<String>),
}

impl Default for BundleTargets {
    fn default() -> Self {
        BundleTargets::Keyword("all".to_owned())
    }
}

impl BundleTargets {
    /// The formats this configuration asks for, or `None` for "all".
    pub fn explicit(&self) -> Option<&[String]> {
        match self {
            BundleTargets::Keyword(_) => None,
            BundleTargets::List(list) => Some(list),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_width() -> u32 {
    960
}

fn default_height() -> u32 {
    640
}

fn default_watch() -> Vec<String> {
    vec!["src".to_owned(), CONFIG_FILE.to_owned()]
}

fn default_category() -> String {
    "DeveloperTool".to_owned()
}

impl Config {
    /// Loads the config for an app directory: `AURO_CONFIG` or `Auro.toml`, then the platform
    /// file, then `AURO_CONFIG_OVERRIDES`.
    ///
    /// Without `AURO_CONFIG`, the app directory is tried first and the running executable's
    /// directory second (spec 0002, D2).
    pub fn load(dir: &Path) -> Result<Config, AuroError> {
        if let Some(path) = std::env::var_os(CONFIG_ENV) {
            return Config::load_from(Path::new(&path));
        }

        let beside_executable = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join(CONFIG_FILE)));
        load_from_candidates(dir, beside_executable.as_deref())
    }

    /// Loads an explicit path, merging the sibling platform file and the override patch.
    pub fn load_from(path: &Path) -> Result<Config, AuroError> {
        let mut merged = read_toml(path)?;

        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let platform = directory.join(format!("Auro.{}.toml", std::env::consts::OS));
        if platform.exists() {
            merge(&mut merged, read_toml(&platform)?);
        }

        if let Some(raw) = std::env::var_os(OVERRIDES_ENV) {
            let patch: Value = serde_json::from_str(&raw.to_string_lossy()).map_err(|error| {
                AuroError::config(
                    path,
                    None,
                    format!("{OVERRIDES_ENV} is not valid JSON: {error}"),
                )
            })?;
            merge(&mut merged, patch);
        }

        let config: Config = merged
            .try_into()
            .map_err(|error: toml::de::Error| AuroError::config(path, None, error.to_string()))?;
        config.validate(path)?;
        Ok(config)
    }

    pub fn default_window(&self) -> Option<&WindowConfig> {
        match &self.app.default_window {
            Some(label) => self
                .app
                .windows
                .iter()
                .find(|window| &window.label == label),
            None => self.app.windows.first(),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), AuroError> {
        let app = &self.app;

        if !is_identifier(&app.identifier) {
            return Err(AuroError::config(
                path,
                Some("app.identifier"),
                format!("`{}` is not a reverse-DNS identifier", app.identifier),
            ));
        }

        semver::Version::parse(&app.version)
            .map_err(|error| AuroError::config(path, Some("app.version"), error.to_string()))?;

        if app.windows.is_empty() {
            return Err(AuroError::config(
                path,
                Some("app.windows"),
                "declare at least one window",
            ));
        }

        let mut labels = BTreeSet::new();
        for window in &app.windows {
            if !is_window_label(&window.label) {
                return Err(AuroError::config(
                    path,
                    Some(&format!("app.windows.{}", window.label)),
                    format!("`{}` is not a valid window label", window.label),
                ));
            }
            if !labels.insert(window.label.clone()) {
                return Err(AuroError::config(
                    path,
                    Some(&format!("app.windows.{}", window.label)),
                    format!("duplicate window label `{}`", window.label),
                ));
            }
        }

        if let Some(default) = &app.default_window
            && !labels.contains(default)
        {
            return Err(AuroError::config(
                path,
                Some("app.default-window"),
                format!("`{default}` does not name a declared window"),
            ));
        }

        for watch in &self.dev.watch {
            ensure_relative(path, "dev.watch", watch)?;
        }
        for resource in &self.bundle.resources {
            ensure_relative(path, "bundle.resources", resource)?;
        }

        if let BundleTargets::Keyword(word) = &self.bundle.targets
            && word != "all"
        {
            return Err(AuroError::config(
                path,
                Some("bundle.targets"),
                format!("`{word}` is not a target set; use \"all\" or a list"),
            ));
        }

        if let BundleTargets::List(targets) = &self.bundle.targets {
            let host = host_bundle_targets();
            for target in targets {
                if !BUNDLE_TARGETS.contains(&target.as_str()) {
                    return Err(AuroError::config(
                        path,
                        Some("bundle.targets"),
                        format!(
                            "`{target}` is not a package format; use one of {}",
                            BUNDLE_TARGETS.join(", ")
                        ),
                    ));
                }
                if !host.contains(&target.as_str()) {
                    return Err(AuroError::config(
                        path,
                        Some("bundle.targets"),
                        format!(
                            "`{target}` cannot be produced on {}; this host supports {}",
                            std::env::consts::OS,
                            host.join(", ")
                        ),
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Tries the app directory first, then the executable's directory (spec 0002, D2).
fn load_from_candidates(dir: &Path, beside_executable: Option<&Path>) -> Result<Config, AuroError> {
    let local = dir.join(CONFIG_FILE);
    if local.exists() {
        return Config::load_from(&local);
    }
    if let Some(path) = beside_executable
        && path.exists()
    {
        return Config::load_from(path);
    }

    let message = match beside_executable {
        Some(path) => format!("file not found; also looked in {}", path.display()),
        None => "file not found".to_owned(),
    };
    Err(AuroError::config(&local, None, message))
}

/// Every package format the bundler knows (spec 0002, `[bundle]`).
const BUNDLE_TARGETS: [&str; 7] = ["app", "dmg", "msi", "nsis", "appimage", "deb", "rpm"];

/// The formats this host can actually produce; anything else is an error, not a skip.
fn host_bundle_targets() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    return &["app", "dmg"];
    #[cfg(target_os = "windows")]
    return &["msi", "nsis"];
    #[cfg(target_os = "linux")]
    return &["appimage", "deb", "rpm"];
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return &[];
}

fn read_toml(path: &Path) -> Result<Value, AuroError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| AuroError::config(path, None, format!("cannot read the file: {error}")))?;
    toml::from_str::<Value>(&text).map_err(|error: toml::de::Error| {
        AuroError::config(path, None, format!("invalid TOML: {error}"))
    })
}

/// RFC 7396 merge: tables merge recursively, everything else replaces.
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Table(base), Value::Table(patch)) => {
            for (key, value) in patch {
                match base.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

fn is_identifier(value: &str) -> bool {
    if value.is_empty() || value.starts_with('.') || value.ends_with('.') {
        return false;
    }
    if !value.contains('.') {
        return false;
    }
    value
        .split('.')
        .all(|part| !part.is_empty() && is_identifier_part(part))
}

fn is_identifier_part(part: &str) -> bool {
    part.chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn is_window_label(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn ensure_relative(path: &Path, key: &str, value: &str) -> Result<(), AuroError> {
    let candidate = Path::new(value);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AuroError::config(
            path,
            Some(key),
            format!("`{value}` must be relative to the app directory and stay inside it"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[app]
identifier = "com.example.demo"
product-name = "Demo"
version = "0.1.0"

[[app.windows]]
label = "main"
"#;

    fn parse(text: &str) -> Result<Config, AuroError> {
        let value: Value = toml::from_str(text).expect("valid TOML");
        let config: Config = value.try_into().map_err(|error: toml::de::Error| {
            AuroError::config(Path::new("Auro.toml"), None, error.to_string())
        })?;
        config.validate(Path::new("Auro.toml"))?;
        Ok(config)
    }

    #[test]
    fn parses_the_minimal_file_and_applies_defaults() {
        let config = parse(MINIMAL).expect("parses");
        let window = config.default_window().expect("a default window");
        assert_eq!(window.label, "main");
        assert_eq!(window.width, 960);
        assert_eq!(window.height, 640);
        assert!(window.resizable);
        assert!(window.center);
        assert_eq!(config.dev.watch, ["src", "Auro.toml"]);
        assert!(config.bundle.active);
    }

    #[test]
    fn hot_dev_mode_is_opt_in() {
        assert!(!parse(MINIMAL).expect("parses").dev.hot);
        let text = format!("{MINIMAL}\n[dev]\nhot = true\n");
        assert!(parse(&text).expect("parses").dev.hot);
    }

    #[test]
    fn rejects_unknown_keys() {
        let text = format!("{MINIMAL}\n[app.extra]\nnope = true\n");
        let error = parse(&text).expect_err("unknown key");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn platform_file_replaces_arrays_and_merges_tables() {
        let base: Value = toml::from_str(MINIMAL).expect("valid TOML");
        let patch: Value = toml::from_str(
            r#"
[app]
product-name = "Patched"

[[app.windows]]
label = "secondary"
width = 320
"#,
        )
        .expect("valid TOML");

        let mut merged = base;
        merge(&mut merged, patch);
        let config: Config = merged.try_into().expect("valid config");

        assert_eq!(config.app.product_name, "Patched");
        assert_eq!(config.app.windows.len(), 1);
        assert_eq!(config.app.windows[0].label, "secondary");
        assert_eq!(config.app.windows[0].width, 320);
    }

    #[test]
    fn rejects_a_non_reverse_dns_identifier() {
        let text = MINIMAL.replace("com.example.demo", "demo");
        let error = parse(&text).expect_err("identifier");
        assert!(error.to_string().contains("reverse-DNS"), "{error}");
    }

    #[test]
    fn rejects_duplicate_window_labels() {
        let text = format!("{MINIMAL}\n[[app.windows]]\nlabel = \"main\"\n");
        let error = parse(&text).expect_err("duplicate");
        assert!(error.to_string().contains("duplicate"), "{error}");
    }

    #[test]
    fn rejects_a_non_semver_version() {
        let text = MINIMAL.replace("0.1.0", "one");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn rejects_a_default_window_that_does_not_exist() {
        let text = r#"
[app]
identifier = "com.example.demo"
product-name = "Demo"
version = "0.1.0"
default-window = "nope"

[[app.windows]]
label = "main"
"#;
        let error = parse(text).expect_err("missing window");
        assert!(error.to_string().contains("does not name"), "{error}");
    }

    #[test]
    fn rejects_paths_that_escape_the_app_directory() {
        let text = format!("{MINIMAL}\n[dev]\nwatch = [\"../secrets\"]\n");
        let error = parse(&text).expect_err("escape");
        assert!(error.to_string().contains("relative"), "{error}");
    }

    #[test]
    fn missing_config_names_both_places_searched() {
        let app = tempfile::tempdir().expect("tempdir");
        let exe = tempfile::tempdir().expect("tempdir");
        let beside_binary = exe.path().join(CONFIG_FILE);

        let error =
            load_from_candidates(app.path(), Some(&beside_binary)).expect_err("missing config");
        let message = error.to_string();

        let local = app.path().join(CONFIG_FILE);
        assert!(message.contains(&local.display().to_string()), "{message}");
        assert!(
            message.contains(&beside_binary.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn falls_back_to_the_executable_directory() {
        let app = tempfile::tempdir().expect("tempdir");
        let exe = tempfile::tempdir().expect("tempdir");
        std::fs::write(exe.path().join(CONFIG_FILE), MINIMAL).expect("config");

        let config = load_from_candidates(app.path(), Some(&exe.path().join(CONFIG_FILE)))
            .expect("the executable's config is used");
        assert_eq!(config.app.product_name, "Demo");
    }

    #[test]
    fn prefers_the_app_directory_over_the_executable_directory() {
        let app = tempfile::tempdir().expect("tempdir");
        let exe = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            app.path().join(CONFIG_FILE),
            MINIMAL.replace("Demo", "Local"),
        )
        .expect("config");
        std::fs::write(
            exe.path().join(CONFIG_FILE),
            MINIMAL.replace("Demo", "Beside binary"),
        )
        .expect("config");

        let config = load_from_candidates(app.path(), Some(&exe.path().join(CONFIG_FILE)))
            .expect("the app directory wins");
        assert_eq!(config.app.product_name, "Local");
    }

    #[test]
    fn rejects_an_unknown_bundle_target() {
        let text = format!("{MINIMAL}\n[bundle]\ntargets = [\"exe\"]\n");
        let error = parse(&text).expect_err("unknown target");
        assert!(
            error.to_string().contains("is not a package format"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_bundle_target_this_host_cannot_produce() {
        let host = host_bundle_targets();
        let foreign = BUNDLE_TARGETS
            .iter()
            .find(|target| !host.contains(target))
            .expect("every supported host has a foreign package format");
        let text = format!("{MINIMAL}\n[bundle]\ntargets = [\"{foreign}\"]\n");
        let error = parse(&text).expect_err("foreign target");
        assert!(error.to_string().contains("cannot be produced"), "{error}");
    }

    #[test]
    fn every_documented_key_survives_a_round_trip() {
        let host = host_bundle_targets()
            .first()
            .expect("supported hosts have at least one bundle target");
        let text = format!(
            r#"
"$schema" = "https://example.test/auro.schema.json"

[app]
identifier = "com.example.roundtrip"
product-name = "Roundtrip"
version = "1.2.3"
description = "round-trip fixture"
authors = ["Ana <ana@example.test>"]
homepage = "https://example.test"
license = "MIT OR Apache-2.0"
default-window = "secondary"

[[app.windows]]
label = "main"
title = "Main"
width = 800
height = 600
min-width = 320
min-height = 240
max-width = 1600
max-height = 1200
resizable = false
decorations = false
transparent = true
always-on-top = true
visible = false
center = false
fullscreen = true
theme = "dark"

[[app.windows]]
label = "secondary"
title = "Second"
width = 400
height = 300
min-width = 200
min-height = 150
max-width = 800
max-height = 600
resizable = true
decorations = true
transparent = false
always-on-top = false
visible = true
center = true
fullscreen = false
theme = "light"

[dev]
watch = ["src", "views", "Auro.toml"]
run-args = ["--verbose"]
build-args = ["--features", "dev"]
hot = true

[bundle]
active = false
targets = ["{host}"]
icon = ["icons/icon.png"]
resources = ["assets/**"]
category = "Utility"
short-description = "short"
long-description = "long"
copyright = "2026 The Auro Authors"
license-file = "LICENSE"
"#
        );

        let base: Value = toml::from_str(&text).expect("valid TOML");
        let config: Config = base.clone().try_into().expect("valid config");
        config.validate(Path::new("Auro.toml")).expect("valid");

        let serialized = toml::to_string(&config).expect("serializes");
        let round: Value = toml::from_str(&serialized).expect("round-trip TOML");
        assert_eq!(round, base);
    }
}
