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
    #[serde(default)]
    pub plugins: PluginConfig,
    /// Opt-in release channel configuration; absent means no channel is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updater: Option<UpdaterConfig>,
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
    #[serde(default)]
    pub macos: MacOsBundleConfig,
    #[serde(default)]
    pub windows: WindowsBundleConfig,
}

/// macOS signing and notarization inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MacOsBundleConfig {
    /// Code-signing identity passed to `codesign`.
    #[serde(default)]
    pub signing_identity: Option<String>,
    /// Path to the entitlements plist, relative to the app directory.
    #[serde(default)]
    pub entitlements: Option<String>,
    /// Enable the hardened runtime; default true for distribution.
    #[serde(default = "default_true")]
    pub hardened_runtime: bool,
    /// Do not wait for/stapling the notarization ticket.
    #[serde(default)]
    pub skip_stapling: bool,
    /// Keychain profile passed to `xcrun notarytool` after `auro bundle` produces an artifact.
    ///
    /// The profile is created ahead of time with `xcrun notarytool store-credentials`; Auro never
    /// stores Apple credentials. Ignored on hosts that cannot produce macOS artifacts.
    #[serde(default)]
    pub notarize_profile: Option<String>,
}

impl Default for MacOsBundleConfig {
    fn default() -> Self {
        MacOsBundleConfig {
            signing_identity: None,
            entitlements: None,
            hardened_runtime: true,
            skip_stapling: false,
            notarize_profile: None,
        }
    }
}

/// Windows Authenticode signing inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowsBundleConfig {
    /// SHA-1 hash of the signing certificate.
    #[serde(default)]
    pub certificate_thumbprint: Option<String>,
    /// File digest algorithm, e.g. `sha256`.
    #[serde(default)]
    pub digest_algorithm: Option<String>,
    /// Timestamp server URL.
    #[serde(default)]
    pub timestamp_url: Option<String>,
    /// Use RFC 3161 timestamping.
    #[serde(default)]
    pub tsp: bool,
    /// Custom sign command; `%1` in the arguments is replaced with the binary path.
    #[serde(default)]
    pub sign_command: Option<String>,
    /// Arguments for `sign-command`.
    #[serde(default)]
    pub sign_args: Vec<String>,
    /// Allow an older version to replace a newer one.
    #[serde(default = "default_true")]
    pub allow_downgrades: bool,
}

impl Default for WindowsBundleConfig {
    fn default() -> Self {
        WindowsBundleConfig {
            certificate_thumbprint: None,
            digest_algorithm: None,
            timestamp_url: None,
            tsp: false,
            sign_command: None,
            sign_args: Vec::new(),
            allow_downgrades: true,
        }
    }
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
            macos: MacOsBundleConfig::default(),
            windows: WindowsBundleConfig::default(),
        }
    }
}

/// Signed in-process plugin host settings (`[plugins]`).
///
/// This is host configuration only: an empty allowlist means no plugin is loaded, even when
/// `enabled` is true. Entries are id/version pins; the host resolves nothing while parsing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PluginConfig {
    /// Whether plugin loading is enabled; off by default.
    #[serde(default)]
    pub enabled: bool,
    /// Plugins the host may load (`[[plugins.allow]]` entries).
    #[serde(default)]
    pub allow: Vec<AllowedPluginConfig>,
}

/// One `[[plugins.allow]]` pin: an exact plugin id and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AllowedPluginConfig {
    /// Plugin id, matched against the subject derived from the signed index.
    pub id: String,
    /// Exact plugin version; no ranges or prefixes.
    pub version: String,
}

/// GitHub Releases update channel settings (`[updater]`).
///
/// The section is parsed without the facade's `updater` feature so release-side tools read the
/// same file; the channel adapter consumes the values when the feature is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UpdaterConfig {
    /// Release provider; `"github"` is the only provider in this alpha.
    #[serde(default)]
    pub provider: UpdateProvider,
    /// Public repository as `owner/repo`.
    pub repository: String,
    /// Base64 Ed25519 public key printed by `auro generate-key`.
    pub public_key: String,
    /// Named signed manifest asset in the latest release.
    #[serde(default = "default_manifest_asset")]
    pub manifest_asset: String,
    /// Minimum seconds between automatic checks.
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// Allow a check before any timestamp has been recorded.
    #[serde(default = "default_true")]
    pub check_on_start: bool,
    /// Allow prereleases by resolving the release list instead of `/releases/latest`.
    #[serde(default)]
    pub include_prereleases: bool,
}

/// The only supported release provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateProvider {
    /// Public GitHub Releases.
    #[default]
    Github,
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

fn default_manifest_asset() -> String {
    "auro-update.json".to_owned()
}

fn default_interval_secs() -> u64 {
    3600
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
            if window.always_on_top {
                return Err(AuroError::config(
                    path,
                    Some(&format!("app.windows.{}.always-on-top", window.label)),
                    "not supported by GPUI 0.2; remove the key or upgrade the renderer",
                ));
            }
            validate_window_dimensions(path, window)?;
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
        for icon in &self.bundle.icon {
            ensure_relative(path, "bundle.icon", icon)?;
        }
        if !self.bundle.license_file.is_empty() {
            ensure_relative(path, "bundle.license-file", &self.bundle.license_file)?;
        }
        for resource in &self.bundle.resources {
            ensure_relative(path, "bundle.resources", resource)?;
        }
        if let Some(entitlements) = &self.bundle.macos.entitlements {
            ensure_relative(path, "bundle.macos.entitlements", entitlements)?;
        }
        if self
            .bundle
            .macos
            .notarize_profile
            .as_deref()
            .is_some_and(|profile| profile.trim().is_empty())
        {
            return Err(AuroError::config(
                path,
                Some("bundle.macos.notarize-profile"),
                "must not be empty",
            ));
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
            if targets.is_empty() {
                return Err(AuroError::config(
                    path,
                    Some("bundle.targets"),
                    "must contain at least one package format; use \"all\" or list the formats",
                ));
            }
            let mut seen = BTreeSet::new();
            let host = host_bundle_targets();
            for target in targets {
                if !seen.insert(target) {
                    return Err(AuroError::config(
                        path,
                        Some("bundle.targets"),
                        format!("duplicate package format `{target}`"),
                    ));
                }
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

        let mut allowed_ids = BTreeSet::new();
        for allowed in &self.plugins.allow {
            if allowed.id.trim().is_empty() {
                return Err(AuroError::config(
                    path,
                    Some("plugins.allow.id"),
                    "plugin id must not be empty",
                ));
            }
            if allowed.version.trim().is_empty() {
                return Err(AuroError::config(
                    path,
                    Some(&format!("plugins.allow.{}.version", allowed.id)),
                    format!("plugin `{}` has an empty version", allowed.id),
                ));
            }
            if !allowed_ids.insert(allowed.id.as_str()) {
                return Err(AuroError::config(
                    path,
                    Some(&format!("plugins.allow.{}", allowed.id)),
                    format!("duplicate allowed plugin `{}`", allowed.id),
                ));
            }
        }

        if let Some(updater) = &self.updater {
            let repository = updater
                .repository
                .split_once('/')
                .is_some_and(|(owner, repo)| is_github_name(owner) && is_github_name(repo));
            if !repository {
                return Err(AuroError::config(
                    path,
                    Some("updater.repository"),
                    format!("`{}` is not `owner/repo`", updater.repository),
                ));
            }
            if updater.public_key.trim().is_empty() {
                return Err(AuroError::config(
                    path,
                    Some("updater.public-key"),
                    "the release public key must not be empty",
                ));
            }
            if !is_asset_name(&updater.manifest_asset) {
                return Err(AuroError::config(
                    path,
                    Some("updater.manifest-asset"),
                    format!("`{}` is not a plain asset name", updater.manifest_asset),
                ));
            }
            if updater.interval_secs == 0 {
                return Err(AuroError::config(
                    path,
                    Some("updater.interval-secs"),
                    "the check interval must be positive",
                ));
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

fn is_github_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn is_asset_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// Rejects dimensions a window backend cannot represent or that contradict each other.
fn validate_window_dimensions(path: &Path, window: &WindowConfig) -> Result<(), AuroError> {
    let key = |field: &str| format!("app.windows.{}.{field}", window.label);
    let reject = |field: &str, message: &str| {
        Err(AuroError::config(
            path,
            Some(&key(field)),
            message.to_owned(),
        ))
    };

    if window.width == 0 || window.height == 0 {
        return reject(
            "width",
            "initial width and height must be greater than zero",
        );
    }
    for (field, value) in [
        ("min-width", window.min_width),
        ("min-height", window.min_height),
        ("max-width", window.max_width),
        ("max-height", window.max_height),
    ] {
        if value == Some(0) {
            return reject(field, "must be greater than zero when set");
        }
    }
    if let (Some(min), Some(max)) = (window.min_width, window.max_width)
        && min > max
    {
        return reject("min-width", "min-width is greater than max-width");
    }
    if let (Some(min), Some(max)) = (window.min_height, window.max_height)
        && min > max
    {
        return reject("min-height", "min-height is greater than max-height");
    }
    if let Some(min) = window.min_width
        && window.width < min
    {
        return reject("width", "width is less than min-width");
    }
    if let Some(max) = window.max_width
        && window.width > max
    {
        return reject("width", "width is greater than max-width");
    }
    if let Some(min) = window.min_height
        && window.height < min
    {
        return reject("height", "height is less than min-height");
    }
    if let Some(max) = window.max_height
        && window.height > max
    {
        return reject("height", "height is greater than max-height");
    }
    Ok(())
}

fn ensure_relative(path: &Path, key: &str, value: &str) -> Result<(), AuroError> {
    let candidate = Path::new(value);
    if value.trim().is_empty() {
        return Err(AuroError::config(
            path,
            Some(key),
            "must not be empty".to_owned(),
        ));
    }
    if candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
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

    fn named_config_error(error: &AuroError) -> (&str, &str) {
        match error {
            AuroError::Config {
                key: Some(key),
                message,
                ..
            } => (key.as_str(), message.as_str()),
            other => panic!("expected a named config error, got {other:?}"),
        }
    }

    const TWO_PLUGINS: &str = r#"
[plugins]
enabled = true

[[plugins.allow]]
id = "com.example.alpha"
version = "1.0.0"

[[plugins.allow]]
id = "com.example.beta"
version = "2.3.4"
"#;

    #[test]
    fn plugins_are_disabled_with_an_empty_allowlist_by_default() {
        let config = parse(MINIMAL).expect("parses");
        assert!(!config.plugins.enabled);
        assert!(config.plugins.allow.is_empty());
    }

    #[test]
    fn parses_two_allowed_plugins() {
        let text = format!("{MINIMAL}{TWO_PLUGINS}");
        let config = parse(&text).expect("parses");
        assert!(config.plugins.enabled);
        assert_eq!(config.plugins.allow.len(), 2);
        assert_eq!(config.plugins.allow[0].id, "com.example.alpha");
        assert_eq!(config.plugins.allow[0].version, "1.0.0");
        assert_eq!(config.plugins.allow[1].id, "com.example.beta");
        assert_eq!(config.plugins.allow[1].version, "2.3.4");
    }

    #[test]
    fn enabled_plugins_with_no_allowlist_are_allowed() {
        let text = format!("{MINIMAL}\n[plugins]\nenabled = true\n");
        let config = parse(&text).expect("parses");
        assert!(config.plugins.enabled);
        assert!(config.plugins.allow.is_empty());
    }

    #[test]
    fn rejects_duplicate_plugin_ids_with_a_named_error() {
        let text = format!(
            "{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"com.example.alpha\"\nversion = \"1.0.0\"\n\n[[plugins.allow]]\nid = \"com.example.alpha\"\nversion = \"2.0.0\"\n"
        );
        let error = parse(&text).expect_err("duplicate plugin id");
        let (key, message) = named_config_error(&error);
        assert_eq!(key, "plugins.allow.com.example.alpha");
        assert!(message.contains("duplicate"), "{message}");
        assert!(message.contains("com.example.alpha"), "{message}");
    }

    #[test]
    fn rejects_empty_plugin_ids_and_versions_with_named_errors() {
        let no_id =
            format!("{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"\"\nversion = \"1.0.0\"\n");
        let error = parse(&no_id).expect_err("empty id");
        let (key, message) = named_config_error(&error);
        assert_eq!(key, "plugins.allow.id");
        assert!(message.contains("empty"), "{message}");

        let blank_id = format!(
            "{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"  \"\nversion = \"1.0.0\"\n"
        );
        let error = parse(&blank_id).expect_err("blank id");
        let (key, message) = named_config_error(&error);
        assert_eq!(key, "plugins.allow.id");
        assert!(message.contains("empty"), "{message}");

        let no_version = format!(
            "{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"com.example.alpha\"\nversion = \"\"\n"
        );
        let error = parse(&no_version).expect_err("empty version");
        let (key, message) = named_config_error(&error);
        assert_eq!(key, "plugins.allow.com.example.alpha.version");
        assert!(message.contains("empty"), "{message}");

        let blank_version = format!(
            "{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"com.example.alpha\"\nversion = \"   \"\n"
        );
        let error = parse(&blank_version).expect_err("blank version");
        let (key, message) = named_config_error(&error);
        assert_eq!(key, "plugins.allow.com.example.alpha.version");
        assert!(message.contains("empty"), "{message}");
    }

    #[test]
    fn rejects_unknown_keys_in_plugin_tables() {
        let table = format!("{MINIMAL}\n[plugins]\nenabled = true\nnope = true\n");
        let error = parse(&table).expect_err("unknown plugins key");
        assert!(error.to_string().contains("unknown field"), "{error}");

        let entry = format!(
            "{MINIMAL}\n[plugins]\n\n[[plugins.allow]]\nid = \"com.example.alpha\"\nversion = \"1.0.0\"\nnope = true\n"
        );
        let error = parse(&entry).expect_err("unknown allow key");
        assert!(error.to_string().contains("unknown field"), "{error}");
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
    fn notarize_profile_is_optional_but_not_empty() {
        let text = format!("{MINIMAL}\n[bundle.macos]\nnotarize-profile = \"auro-release\"\n");
        let config = parse(&text).expect("parses");
        assert_eq!(
            config.bundle.macos.notarize_profile.as_deref(),
            Some("auro-release")
        );

        let text = format!("{MINIMAL}\n[bundle.macos]\nnotarize-profile = \"  \"\n");
        assert!(matches!(parse(&text), Err(AuroError::Config { .. })));
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
    fn rejects_zero_and_contradictory_window_dimensions() {
        let zero = format!("{MINIMAL}\nwidth = 0\n");
        let error = parse(&zero).expect_err("zero width");
        assert!(error.to_string().contains("greater than zero"), "{error}");

        let zero_min = format!("{MINIMAL}\nmin-height = 0\n");
        let error = parse(&zero_min).expect_err("zero minimum");
        assert!(error.to_string().contains("greater than zero"), "{error}");

        let inverted = format!("{MINIMAL}\nmin-width = 1280\nmax-width = 320\n");
        let error = parse(&inverted).expect_err("min above max");
        assert!(error.to_string().contains("min-width"), "{error}");
        assert!(error.to_string().contains("max-width"), "{error}");

        let valid = format!("{MINIMAL}\nmin-width = 320\nmax-width = 1280\n");
        let config = parse(&valid).expect("valid bounds");
        assert_eq!(config.app.windows[0].min_width, Some(320));
        assert_eq!(config.app.windows[0].max_width, Some(1280));
    }

    #[test]
    fn rejects_always_on_top_until_the_renderer_supports_it() {
        let text = format!("{MINIMAL}\nalways-on-top = true\n");
        let error = parse(&text).expect_err("unsupported always-on-top");
        assert!(error.to_string().contains("always-on-top"), "{error}");
        assert!(error.to_string().contains("not supported"), "{error}");
    }

    #[test]
    fn rejects_an_initial_size_outside_its_own_bounds() {
        let too_wide = format!("{MINIMAL}\nwidth = 500\nmax-width = 320\n");
        let error = parse(&too_wide).expect_err("width above max-width");
        assert!(error.to_string().contains("max-width"), "{error}");

        let too_narrow = format!("{MINIMAL}\nwidth = 100\nmin-width = 320\n");
        let error = parse(&too_narrow).expect_err("width below min-width");
        assert!(error.to_string().contains("min-width"), "{error}");

        let too_tall = format!("{MINIMAL}\nheight = 900\nmax-height = 640\n");
        let error = parse(&too_tall).expect_err("height above max-height");
        assert!(error.to_string().contains("max-height"), "{error}");
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

        let icon = format!("{MINIMAL}\n[bundle]\nicon = [\"/etc/passwd\"]\n");
        let error = parse(&icon).expect_err("absolute icon");
        assert!(error.to_string().contains("relative"), "{error}");

        let license = format!("{MINIMAL}\n[bundle]\nlicense-file = \"../LICENSE\"\n");
        let error = parse(&license).expect_err("escaping license");
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
    fn rejects_a_duplicate_bundle_target() {
        let host = host_bundle_targets()
            .first()
            .expect("supported hosts have at least one bundle target");
        let text = format!("{MINIMAL}\n[bundle]\ntargets = [\"{host}\", \"{host}\"]\n");
        let error = parse(&text).expect_err("duplicate target");
        assert!(error.to_string().contains("duplicate"), "{error}");
        assert!(error.to_string().contains(host), "{error}");
    }

    #[test]
    fn rejects_an_empty_bundle_target_list() {
        let text = format!("{MINIMAL}\n[bundle]\ntargets = []\n");
        let error = parse(&text).expect_err("empty target list");
        assert!(error.to_string().contains("at least one"), "{error}");
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
    fn updater_section_parses_and_validates() {
        let section = r#"
[updater]
repository = "pdl-clay/auri"
public-key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
"#;
        let config = parse(&format!("{MINIMAL}\n{section}")).expect("valid updater config");
        let updater = config.updater.as_ref().expect("updater section");
        assert_eq!(updater.provider, UpdateProvider::Github);
        assert_eq!(updater.manifest_asset, "auro-update.json");
        assert_eq!(updater.interval_secs, 3600);
        assert!(!updater.include_prereleases);

        let explicit = format!(
            "{MINIMAL}\n[updater]\nprovider = \"github\"\nrepository = \"pdl-clay/auri\"\npublic-key = \"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"\nmanifest-asset = \"nightly.json\"\ninterval-secs = 90\ncheck-on-start = false\ninclude-prereleases = true\n"
        );
        let config = parse(&explicit).expect("explicit updater config");
        let updater = config.updater.as_ref().expect("updater section");
        assert_eq!(updater.manifest_asset, "nightly.json");
        assert_eq!(updater.interval_secs, 90);
        assert!(updater.include_prereleases);

        let invalid = [
            "provider = \"gitlab\"\nrepository = \"pdl-clay/auri\"\npublic-key = \"key\"",
            "repository = \"pdl-clay/auri/extra\"\npublic-key = \"key\"",
            "repository = \"pdl-clay\"\npublic-key = \"key\"",
            "repository = \"pdl-clay/auri\"\npublic-key = \"\"",
            "repository = \"pdl-clay/auri\"\npublic-key = \"key\"\nmanifest-asset = \"../latest.json\"",
            "repository = \"pdl-clay/auri\"\npublic-key = \"key\"\ninterval-secs = 0",
        ];
        for section in invalid {
            let text = format!("{MINIMAL}\n[updater]\n{section}\n");
            assert!(parse(&text).is_err(), "accepted invalid section: {section}");
        }
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
always-on-top = false
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

[plugins]
enabled = true

[[plugins.allow]]
id = "com.example.toolkit"
version = "1.2.3"

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

[bundle.macos]
signing-identity = "Developer ID Application: Auro"
entitlements = "entitlements.plist"
hardened-runtime = false
skip-stapling = true
notarize-profile = "auro-release"

[bundle.windows]
certificate-thumbprint = "ABC123"
digest-algorithm = "sha256"
timestamp-url = "https://timestamp.example.test"
tsp = true
sign-command = "signtool"
sign-args = ["sign", "%1"]
allow-downgrades = false

[updater]
provider = "github"
repository = "pdl-clay/auri"
public-key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
manifest-asset = "custom.json"
interval-secs = 120
check-on-start = false
include-prereleases = true
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
