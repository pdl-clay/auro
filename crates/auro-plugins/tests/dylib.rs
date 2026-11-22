// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the opt-in in-process dylib backend.
//!
//! In the default configuration the feature is off, so loading must refuse without touching the
//! path. With `plugins-dylib`, a small `cdylib` fixture is compiled by `rustc` into a temporary
//! directory. The fixture exports the three required symbols with configurable values and either
//! echoes the request bytes or panics in the entry point.

#[cfg(feature = "plugins-dylib")]
use auro_plugins::dylib::{ABI_SYMBOL, ABI_VERSION};
use auro_plugins::dylib::{DylibError, DylibPlugin};
#[cfg(feature = "plugins-dylib")]
use auro_plugins::sdk_version;
#[cfg(feature = "plugins-dylib")]
use std::path::PathBuf;
#[cfg(feature = "plugins-dylib")]
use std::process::Command;
#[cfg(feature = "plugins-dylib")]
use tempfile::TempDir;

#[cfg(not(feature = "plugins-dylib"))]
#[test]
fn load_refuses_when_feature_is_disabled() {
    let error = DylibPlugin::load("/not/a/real/plugin.so").unwrap_err();
    assert!(matches!(error, DylibError::FeatureDisabled), "{error:?}");
}

#[cfg(feature = "plugins-dylib")]
mod enabled {
    use super::*;

    #[derive(Clone, Copy)]
    enum Entry {
        Echo,
    }

    struct Fixture {
        abi: u32,
        sdk: String,
        entry: Entry,
        abi_symbol: bool,
    }

    impl Fixture {
        fn valid() -> Self {
            Self {
                abi: ABI_VERSION,
                sdk: sdk_version().to_owned(),
                entry: Entry::Echo,
                abi_symbol: true,
            }
        }

        fn source(&self) -> String {
            let abi = if self.abi_symbol {
                format!(
                    "#[unsafe(no_mangle)]\n#[used]\npub static {ABI_SYMBOL}: u32 = {value};\n",
                    value = self.abi
                )
            } else {
                String::new()
            };
            let body = match self.entry {
                Entry::Echo => concat!(
                    "let count = input_len.min(output_capacity);\n",
                    "unsafe { std::ptr::copy_nonoverlapping(input, output, count); }\n",
                    "count as i32"
                ),
            };
            format!(
                concat!(
                    "use std::ffi::c_char;\n",
                    "{abi}\n",
                    "#[unsafe(no_mangle)]\n",
                    "pub extern \"C\" fn {SDK_SYMBOL}() -> *const c_char {{\n",
                    "    concat!({sdk:?}, \"\\0\").as_ptr().cast()\n",
                    "}}\n",
                    "#[unsafe(no_mangle)]\n",
                    "pub unsafe extern \"C-unwind\" fn auro_plugin_entry(\n",
                    "    input: *const u8,\n",
                    "    input_len: usize,\n",
                    "    output: *mut u8,\n",
                    "    output_capacity: usize,\n",
                    ") -> i32 {{\n",
                    "    {body}\n",
                    "}}\n",
                ),
                abi = abi,
                sdk = self.sdk,
                body = body,
                SDK_SYMBOL = auro_plugins::dylib::SDK_SYMBOL,
            )
        }
    }

    fn build(fixture: &Fixture) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("temporary directory for fixture");
        let source = dir.path().join("fixture.rs");
        std::fs::write(&source, fixture.source()).expect("write fixture source");
        let file_name = format!(
            "{}{}{}",
            std::env::consts::DLL_PREFIX,
            "auro_dylib_fixture",
            std::env::consts::DLL_SUFFIX
        );
        let library = dir.path().join(file_name);
        let status = Command::new("rustc")
            .args([
                "--edition=2024",
                "--crate-type=cdylib",
                "--crate-name=auro_dylib_fixture",
                "--cap-lints=allow",
                "-Cpanic=unwind",
                "-Cforce-unwind-tables=yes",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&library)
            .status()
            .expect("rustc on PATH");
        assert!(status.success(), "fixture compilation failed");
        (dir, library)
    }

    #[test]
    fn valid_fixture_loads_and_echoes_raw_bytes() {
        let (_dir, path) = build(&Fixture::valid());
        let plugin = DylibPlugin::load(&path).expect("valid fixture loads");
        assert_eq!(plugin.sdk_version(), sdk_version());

        let mut output = [0u8; 8];
        let written = plugin.call(b"hello", &mut output).expect("echo call");
        assert_eq!(written, 5);
        assert_eq!(&output[..5], b"hello");
    }

    #[test]
    fn abi_mismatch_is_rejected_before_the_entry_point() {
        let mut fixture = Fixture::valid();
        fixture.abi = ABI_VERSION + 1;
        let (_dir, path) = build(&fixture);

        let error = DylibPlugin::load(&path).expect_err("foreign ABI is rejected");
        assert!(
            matches!(
                error,
                DylibError::AbiMismatch { expected: ABI_VERSION, found }
                    if found == ABI_VERSION + 1
            ),
            "{error:?}"
        );
    }

    #[test]
    fn sdk_mismatch_is_rejected_before_the_entry_point() {
        let mut fixture = Fixture::valid();
        fixture.sdk = "0.0.0-old-sdk".to_owned();
        let (_dir, path) = build(&fixture);

        let error = DylibPlugin::load(&path).expect_err("foreign SDK is rejected");
        assert!(
            matches!(
                error,
                DylibError::SdkMismatch { ref expected, ref found }
                    if expected == sdk_version() && found == "0.0.0-old-sdk"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn panic_at_the_entry_point_is_caught() {
        // A separately compiled cdylib carries its own panic runtime and cannot unwind into the
        // host's `catch_unwind` on every platform. Test the boundary directly with an entry point
        // from this crate so the caught panic is deterministic; the cdylib tests above still cover
        // loading, ABI/SDK gates and a real foreign entry call.
        unsafe extern "C-unwind" fn panicking_entry(
            _input: *const u8,
            _input_len: usize,
            _output: *mut u8,
            _output_len: usize,
        ) -> i32 {
            panic!("fixture entry panic")
        }

        let mut output = [0u8; 4];
        let error = unsafe { auro_plugins::dylib::call_entry(panicking_entry, b"x", &mut output) }
            .expect_err("panic is caught at the ABI boundary");
        assert!(matches!(error, DylibError::Panic), "{error:?}");
    }

    #[test]
    fn missing_abi_symbol_is_a_typed_error() {
        let mut fixture = Fixture::valid();
        fixture.abi_symbol = false;
        let (_dir, path) = build(&fixture);

        let error = DylibPlugin::load(&path).expect_err("missing symbol is rejected");
        assert!(
            matches!(error, DylibError::MissingSymbol { name, .. } if name == ABI_SYMBOL),
            "{error:?}"
        );
    }

    #[test]
    fn missing_library_is_a_typed_error() {
        let dir = TempDir::new().expect("temporary directory");
        let error = DylibPlugin::load(dir.path().join("no-such-plugin")).unwrap_err();
        assert!(matches!(error, DylibError::Open(_)), "{error:?}");
    }
}
