// SPDX-License-Identifier: MIT OR Apache-2.0

//! Opt-in in-process backend for native dynamic-library plugins.
//!
//! The `plugins-dylib` Cargo feature is off by default. Without it no dynamic-loading code is
//! compiled and [`DylibPlugin::load`] returns [`DylibError::FeatureDisabled`]. With it, a library
//! is opened once at startup, its exported ABI and SDK versions must match this host exactly, and
//! only then is its entry point called.
//!
//! # Boundaries and limits
//!
//! - The C ABI carries raw byte pointers and lengths only. No GPUI `Entity`, `Window`, `App`,
//!   `serde_json::Value` or other Rust high-level type crosses the boundary; the host owns both
//!   buffers for the duration of a call.
//! - **No hot reload.** Neither plugin code nor UI code is unloaded or patched by this backend.
//!   ADR 0008 (`docs/src/decisions/0008-hot-reload-mechanism-revision.md`) rejects `cdylib` swapping
//!   of GPUI view code for hot reload; this backend is a separate non-view extension loaded at
//!   startup and never swapped. ADR 0025 (`docs/src/decisions/0025-in-process-plugin-dylib.md`)
//!   records the distinction; spec 0036 (`docs/src/specs/0036-plugin-supervision.md`) states the
//!   contract.
//! - **Not a sandbox.** The library shares the app's address space, user, statics and file
//!   descriptors. Capability grants scope effects that pass through the host; a plugin can call
//!   the OS directly. There is no process or privilege isolation.
//! - The entry point uses `extern "C-unwind"` so a Rust unwind can be caught by [`call_entry`] and
//!   reported as [`DylibError::Panic`]. A plugin built with `panic = "abort"`, `abort()`, `exit()`
//!   or a foreign exception still terminates the process: `catch_unwind` is not a sandbox.
//! - Libraries are never unloaded or replaced while a [`DylibPlugin`] exists.

#[cfg(feature = "plugins-dylib")]
use std::ffi::{CStr, c_char};
use std::path::Path;
use thiserror::Error;

/// The only ABI version this host accepts from a plugin.
pub const ABI_VERSION: u32 = 1;

/// Name of the required `u32` ABI version symbol.
pub const ABI_SYMBOL: &str = "auro_plugin_abi_version";

/// Name of the required `*const c_char` SDK version symbol; it must be NUL-terminated UTF-8.
pub const SDK_SYMBOL: &str = "auro_plugin_sdk_version";

/// Name of the required entry-point symbol.
pub const ENTRY_SYMBOL: &str = "auro_plugin_entry";

/// Raw-byte plugin entry point: request pointer, request length, output pointer, output capacity.
///
/// It returns the number of bytes written to `output`, or a plugin-defined negative code. The
/// `C-unwind` ABI lets [`call_entry`] catch a Rust panic; a plugin compiled with `panic = "abort"`
/// cannot be caught.
pub type PluginEntry = unsafe extern "C-unwind" fn(*const u8, usize, *mut u8, usize) -> i32;

/// Every failure while loading or calling an in-process plugin.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DylibError {
    /// The `plugins-dylib` feature was not enabled.
    #[error("the plugins-dylib feature is disabled")]
    FeatureDisabled,

    /// The library file could not be opened.
    #[error("plugin library could not be opened: {0}")]
    Open(String),

    /// A required symbol was absent from the library.
    #[error("plugin symbol `{name}` is missing: {detail}")]
    MissingSymbol {
        /// Symbol the host looked up.
        name: &'static str,
        /// Loader detail.
        detail: String,
    },

    /// The SDK version symbol was null, unterminated or not UTF-8.
    #[error("plugin SDK version symbol is null, unterminated or not UTF-8")]
    InvalidSdkVersion,

    /// The plugin exports an ABI this host does not implement.
    #[error("plugin ABI version mismatch: host {expected}, plugin {found}")]
    AbiMismatch {
        /// ABI version this host implements.
        expected: u32,
        /// ABI version the plugin exported.
        found: u32,
    },

    /// The plugin was built against a different SDK release.
    #[error("plugin SDK version mismatch: host {expected}, plugin {found}")]
    SdkMismatch {
        /// SDK version this host implements.
        expected: String,
        /// SDK version the plugin exported.
        found: String,
    },

    /// The plugin panicked and the unwind was caught at the boundary.
    #[error("plugin entry point panicked")]
    Panic,
}

/// One verified in-process plugin library, loaded once and never swapped.
///
/// Build one with [`DylibPlugin::load`]. All three exported symbols are checked before the entry
/// point can be called. The library stays mapped until this value is dropped.
#[derive(Debug)]
pub struct DylibPlugin {
    /// Keeps the library mapped while `entry` may be called.
    #[cfg(feature = "plugins-dylib")]
    #[allow(dead_code)] // Held only to keep the library mapped; no method needs to read it.
    library: libloading::Library,
    /// Checked entry point from the loaded library.
    #[cfg(feature = "plugins-dylib")]
    entry: PluginEntry,
    /// Exact SDK version exported by the loaded plugin.
    #[cfg(feature = "plugins-dylib")]
    sdk_version: String,
}

#[cfg(not(feature = "plugins-dylib"))]
impl DylibPlugin {
    /// Refuses to load a plugin because `plugins-dylib` is disabled.
    ///
    /// The path is not inspected: no dynamic loading code is compiled in this configuration.
    pub fn load(_path: impl AsRef<Path>) -> Result<Self, DylibError> {
        Err(DylibError::FeatureDisabled)
    }

    /// Refuses to call a plugin because `plugins-dylib` is disabled.
    pub fn call(&self, _request: &[u8], _output: &mut [u8]) -> Result<i32, DylibError> {
        Err(DylibError::FeatureDisabled)
    }
}

#[cfg(feature = "plugins-dylib")]
impl DylibPlugin {
    /// Opens a plugin library and verifies its exported ABI and SDK versions.
    ///
    /// The library must export [`ABI_SYMBOL`] as `u32`, [`SDK_SYMBOL`] as a NUL-terminated UTF-8
    /// string and [`ENTRY_SYMBOL`] as a [`PluginEntry`]. A missing symbol, a bad SDK string or any
    /// version mismatch is a typed error and the library is dropped without calling the entry.
    ///
    /// Loading runs the library's initializers. This is explicitly trusted native code, not a
    /// sandbox (see the module docs).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DylibError> {
        // SAFETY: loading executes plugin initializers; the app opted into this backend and
        // accepts the documented in-process risk. No isolation is claimed.
        let library = unsafe { libloading::Library::new(path.as_ref()) }
            .map_err(|error| DylibError::Open(error.to_string()))?;

        // SAFETY: the plugin SDK declares this symbol as a `u32`. libloading reads data symbols
        // through a function-pointer-sized slot, so request a pointer to the value and deref it.
        let abi_ptr =
            unsafe { library.get::<*const u32>(ABI_SYMBOL.as_bytes()) }.map_err(|error| {
                DylibError::MissingSymbol {
                    name: ABI_SYMBOL,
                    detail: error.to_string(),
                }
            })?;
        // SAFETY: the symbol points at the SDK's static `u32`, alive while the library is loaded.
        let abi = unsafe { **abi_ptr };

        // SAFETY: the SDK exports a function returning a pointer to a NUL-terminated C string.
        // A function symbol avoids data-symbol size and Sync pitfalls across platforms.
        type SdkVersionFn = unsafe extern "C" fn() -> *const c_char;
        let sdk_fn =
            unsafe { library.get::<SdkVersionFn>(SDK_SYMBOL.as_bytes()) }.map_err(|error| {
                DylibError::MissingSymbol {
                    name: SDK_SYMBOL,
                    detail: error.to_string(),
                }
            })?;
        let sdk_ptr = unsafe { sdk_fn() };
        if sdk_ptr.is_null() {
            return Err(DylibError::InvalidSdkVersion);
        }
        // SAFETY: the pointer is non-null and, by the SDK contract, points at a NUL-terminated
        // string that stays valid while the library is loaded. It is copied into an owned String.
        let sdk = unsafe { CStr::from_ptr(sdk_ptr) };
        let sdk = sdk.to_str().map_err(|_| DylibError::InvalidSdkVersion)?;

        verify_versions(abi, sdk)?;

        // SAFETY: the plugin SDK declares this symbol as a `PluginEntry`. Checking the ABI and
        // SDK versions first is what makes the function pointer type trustworthy.
        let entry =
            unsafe { library.get::<PluginEntry>(ENTRY_SYMBOL.as_bytes()) }.map_err(|error| {
                DylibError::MissingSymbol {
                    name: ENTRY_SYMBOL,
                    detail: error.to_string(),
                }
            })?;

        let entry = *entry;
        Ok(Self {
            library,
            entry,
            sdk_version: sdk.to_owned(),
        })
    }

    /// Calls the verified entry point with raw byte buffers.
    ///
    /// The plugin may read at most `request.len()` bytes and write at most `output.len()` bytes.
    /// A caught panic becomes [`DylibError::Panic`]; `abort`, `exit` and foreign exceptions are not
    /// catchable.
    pub fn call(&self, request: &[u8], output: &mut [u8]) -> Result<i32, DylibError> {
        // SAFETY: `self.library` is owned by `self` and stays mapped while `entry` is used; the
        // slices are valid for the duration of the call and describe the buffer boundaries.
        unsafe { call_entry(self.entry, request, output) }
    }

    /// Exact SDK version exported by this plugin.
    pub fn sdk_version(&self) -> &str {
        &self.sdk_version
    }
}

/// Checks a plugin's exported ABI and SDK versions against this host.
///
/// Comparison is exact equality in both cases: no semver compatibility, downgrade or partial
/// match is accepted. This is the same gate [`DylibPlugin::load`] applies after reading symbols.
pub fn verify_versions(abi_version: u32, sdk_version: &str) -> Result<(), DylibError> {
    if abi_version != ABI_VERSION {
        return Err(DylibError::AbiMismatch {
            expected: ABI_VERSION,
            found: abi_version,
        });
    }

    let expected = crate::sdk_version();
    if sdk_version != expected {
        return Err(DylibError::SdkMismatch {
            expected: expected.to_owned(),
            found: sdk_version.to_owned(),
        });
    }

    Ok(())
}

/// Calls a plugin entry point with raw byte buffers, mapping a caught panic to
/// [`DylibError::Panic`].
///
/// # Safety
///
/// `entry` must come from a library that stays loaded for the whole call, and the plugin must
/// honour the pointer contract: read at most `request.len()` bytes from `request` and write at
/// most `output.len()` bytes to `output`. [`DylibPlugin::call`] upholds this for a verified plugin.
pub unsafe fn call_entry(
    entry: PluginEntry,
    request: &[u8],
    output: &mut [u8],
) -> Result<i32, DylibError> {
    // SAFETY: `AssertUnwindSafe` is used because the buffers are ordinary byte slices: after an
    // unwind the caller still owns them, and no shared invariant is observed across the call.
    // The pointers themselves are valid for the call by the caller's guarantee above.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        entry(
            request.as_ptr(),
            request.len(),
            output.as_mut_ptr(),
            output.len(),
        )
    }));
    outcome.map_err(|_| DylibError::Panic)
}
