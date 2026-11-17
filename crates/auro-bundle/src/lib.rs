// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform packaging for Auro apps.
//!
//! This crate maps `Auro.toml`'s `[bundle]` section onto `tauri-bundler` 2.10 and delegates the
//! platform formats to it. It does not build the release binary; `auro bundle` does that first.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use auro_core::{AuroError, BundleTargets, Config};
use tauri_bundler::bundle::{
    AppCategory, BundleBinary, BundleSettings, PackageSettings, PackageType, Settings,
    SettingsBuilder, WindowsSettings,
};
use tauri_utils::config::WebviewInstallMode;

/// The formats this host can produce. Mirrors spec 0002 validation.
pub fn host_targets() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    return &["app", "dmg"];
    #[cfg(target_os = "windows")]
    return &["msi", "nsis"];
    #[cfg(target_os = "linux")]
    return &["appimage", "deb", "rpm"];
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return &[];
}

/// The package types requested by the config, or the host's set for `"all"`.
pub fn package_types(config: &Config) -> Result<Vec<PackageType>, AuroError> {
    let requested: Vec<String> = match &config.bundle.targets {
        BundleTargets::Keyword(_) => host_targets()
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        BundleTargets::List(list) => list.clone(),
    };

    requested
        .iter()
        .map(|name| {
            PackageType::from_short_name(name)
                .ok_or_else(|| AuroError::bundle(format!("unknown package format `{name}`")))
        })
        .collect()
}

/// Bundles the already-built release binary.
///
/// `dir` is the app directory; `binary` is the executable file name produced by
/// `cargo build --release` (using the manifest's `default-run` or package name).
/// Returns every produced artifact path.
pub fn bundle(dir: &Path, config: &Config, binary: &str) -> Result<Vec<PathBuf>, AuroError> {
    preflight(dir, config)?;
    let settings = settings(dir, config, binary)?;

    // tauri-bundler resolves icons/resources and runs its tools relative to the project cwd.
    // `auro bundle` may be invoked from anywhere, so make the app directory the cwd for the
    // bundler call and restore it afterwards.
    let previous = std::env::current_dir()?;
    std::env::set_current_dir(dir)?;
    let result = tauri_bundler::bundle_project(&settings)
        .map_err(|error| AuroError::bundle(error.to_string()));
    std::env::set_current_dir(previous)?;

    let bundles = result?;
    Ok(bundles
        .into_iter()
        .flat_map(|bundle| bundle.bundle_paths)
        .collect())
}

/// Checks the bundler inputs that it validates by panicking instead of returning an error.
fn preflight(dir: &Path, config: &Config) -> Result<(), AuroError> {
    let types = package_types(config)?;
    if types.contains(&PackageType::AppImage)
        && !config
            .bundle
            .icon
            .iter()
            .any(|icon| is_square_png(&dir.join(icon)))
    {
        return Err(AuroError::bundle(
            "AppImage requires a square PNG in [bundle].icon",
        ));
    }
    Ok(())
}

fn is_square_png(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return false;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    width > 0 && width == height
}

fn settings(dir: &Path, config: &Config, binary: &str) -> Result<Settings, AuroError> {
    let out_dir = dir.join("target").join("release");
    let category = AppCategory::from_str(&config.bundle.category).map_err(|suggestion| {
        let hint = suggestion
            .map(|value| format!("; did you mean `{value}`?"))
            .unwrap_or_default();
        AuroError::bundle(format!(
            "`{}` is not a known bundle category{hint}",
            config.bundle.category
        ))
    })?;

    let icon = (!config.bundle.icon.is_empty()).then(|| {
        config
            .bundle
            .icon
            .iter()
            .map(|path| dir.join(path).display().to_string())
            .collect()
    });
    let resources = (!config.bundle.resources.is_empty()).then(|| {
        config
            .bundle
            .resources
            .iter()
            .map(|path| dir.join(path).display().to_string())
            .collect()
    });
    let description = if config.app.description.is_empty() {
        config.app.product_name.clone()
    } else {
        config.app.description.clone()
    };

    let bundle_settings = BundleSettings {
        identifier: Some(config.app.identifier.clone()),
        homepage: (!config.app.homepage.is_empty()).then(|| config.app.homepage.clone()),
        icon,
        resources,
        copyright: (!config.bundle.copyright.is_empty()).then(|| config.bundle.copyright.clone()),
        license: (!config.app.license.is_empty()).then(|| config.app.license.clone()),
        license_file: (!config.bundle.license_file.is_empty())
            .then(|| dir.join(&config.bundle.license_file)),
        category: Some(category),
        short_description: (!config.bundle.short_description.is_empty())
            .then(|| config.bundle.short_description.clone()),
        long_description: (!config.bundle.long_description.is_empty())
            .then(|| config.bundle.long_description.clone()),
        windows: WindowsSettings {
            // Auro has no webview in its app layer; do not download or install WebView2.
            webview_install_mode: WebviewInstallMode::Skip,
            ..WindowsSettings::default()
        },
        ..BundleSettings::default()
    };

    let package = PackageSettings {
        product_name: config.app.product_name.clone(),
        version: config.app.version.clone(),
        description,
        homepage: (!config.app.homepage.is_empty()).then(|| config.app.homepage.clone()),
        authors: (!config.app.authors.is_empty()).then(|| config.app.authors.clone()),
        default_run: Some(binary.to_owned()),
    };

    SettingsBuilder::new()
        .project_out_directory(out_dir)
        .package_settings(package)
        .bundle_settings(bundle_settings)
        .binaries(vec![BundleBinary::new(binary.to_owned(), true)])
        .package_types(package_types(config)?)
        .build()
        .map_err(|error| AuroError::bundle(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn config(extra: &str) -> (tempfile::TempDir, Config) {
        let temp = tempfile::tempdir().expect("tempdir");
        let text = format!(
            r#"
[app]
identifier = "com.example.bundle"
product-name = "Bundle Demo"
version = "1.2.3"
description = "bundles for the test"
authors = ["Ana <ana@example.test>"]
homepage = "https://example.test"
license = "MIT OR Apache-2.0"

[[app.windows]]
label = "main"

[bundle]
category = "DeveloperTool"
short-description = "short"
long-description = "long"
copyright = "2026 Auro"
license-file = "LICENSE"
icon = ["icons/icon.png"]
resources = ["assets/**"]
{extra}
"#
        );
        fs::write(temp.path().join("Auro.toml"), text).expect("config");
        let config = Config::load_from(&temp.path().join("Auro.toml")).expect("valid config");
        (temp, config)
    }

    #[test]
    fn explicit_targets_map_to_package_types() {
        let host = host_targets();
        let Some(name) = host.first() else {
            return;
        };
        let (_temp, config) = config(&format!("targets = [\"{name}\"]"));
        assert_eq!(
            package_types(&config).expect("types"),
            [PackageType::from_short_name(name).expect("known target")]
        );
    }

    #[test]
    fn all_uses_the_host_set() {
        let (_temp, config) = config("");
        let types = package_types(&config).expect("types");
        assert_eq!(types.len(), host_targets().len());
    }

    #[test]
    fn maps_the_manifest_metadata() {
        let (temp, config) = config("");
        let settings = settings(temp.path(), &config, "bundle-demo").expect("settings");
        assert_eq!(
            settings.project_out_directory(),
            temp.path().join("target").join("release")
        );
        assert_eq!(settings.icons().map(Vec::len), Some(1));
        assert!(settings.icons().unwrap()[0].ends_with("icons/icon.png"));
        assert_eq!(settings.binaries()[0].name(), "bundle-demo");
        assert!(!settings.package_types().expect("types").is_empty());
    }

    #[test]
    fn appimage_requires_a_square_png() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let (_temp, config) = config("targets = [\"appimage\"]");
        let error = bundle(Path::new("."), &config, "demo").expect_err("icon");
        assert!(error.to_string().contains("square PNG"), "{error}");
    }

    #[test]
    fn a_square_png_satisfies_the_appimage_preflight() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let (temp, config) = config("targets = [\"appimage\"]");
        let icon = temp.path().join("icons/icon.png");
        fs::create_dir_all(icon.parent().unwrap()).expect("icons");
        // 1x1 PNG: signature + IHDR (width=1, height=1).
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0,
            0, 1, 8, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        fs::write(icon, png).expect("icon");
        assert!(preflight(temp.path(), &config).is_ok());
    }

    #[test]
        if !cfg!(target_os = "linux") {
            return;
        }
        if !cfg!(target_os = "linux") {
            return;
        }
    fn rejects_an_unknown_category() {
        let (_temp, mut config) = config("");
        config.bundle.category = "not a real category".to_owned();
        let error = settings(Path::new("."), &config, "demo").expect_err("category");
        assert_eq!(error.kind(), "bundle");
    }
}
