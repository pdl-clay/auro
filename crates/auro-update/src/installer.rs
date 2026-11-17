// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command execution used by platform installers.
//!
//! The installer constructs an argument vector; it never invokes a shell. Tests inject a
//! [`InstallerRunner`] so command construction and exit-code handling are exercised without
//! starting a real installer.

use std::ffi::OsString;
use std::path::Path;

use crate::UpdateError;

/// Runs an installer command and returns its exit code.
pub trait InstallerRunner {
    /// Runs `program` with `args`, with no shell interpretation.
    ///
    /// `None` means the process ended without an exit code (for example, a signal on Unix).
    fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, UpdateError>;
}

/// Runs installers with the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemInstallerRunner;

impl InstallerRunner for SystemInstallerRunner {
    fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, UpdateError> {
        let status = std::process::Command::new(program).args(args).status()?;
        Ok(status.code())
    }
}
