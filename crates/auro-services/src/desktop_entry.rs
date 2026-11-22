// SPDX-License-Identifier: MIT OR Apache-2.0

//! Desktop Entry escaping shared by deep-link registration and autostart.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

/// Writes one desktop entry without following a symlink at `path`.
///
/// The entry goes to a sibling temporary file and is renamed into place. A pre-created symlink
/// is replaced, never followed, so `$XDG_CONFIG_HOME`/`$XDG_DATA_HOME` cannot redirect the write
/// into an arbitrary user file.
#[cfg(target_os = "linux")]
pub(crate) fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "desktop path has no file name")
    })?;
    for _ in 0..16 {
        let temporary = parent.join(format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot create a unique desktop-entry temporary file",
    ))
}

/// Creates `base/child` and returns it, refusing unsafe directories along the way.
///
/// `create_dir_all` would inherit the process umask, so under `umask 002` the directory would be
/// group-writable and another local user could drop a `.desktop` file that runs as this user
/// (#92). Every directory created here is explicitly set to `0o755`, and every pre-existing
/// component from `base` down to `child` is checked for symlinks, non-directories, a foreign
/// owner and group/other write bits.
///
/// `base` itself may be several components deep and mostly missing on a fresh account (for
/// example `~/.local/share` when `~/.local` does not exist yet). Missing intermediates are created
/// below the deepest existing ancestor; that anchor is not inspected because it belongs to the
/// platform (`/home`, `/tmp`, ...) and the caller chose the base.
#[cfg(target_os = "linux")]
pub(crate) fn ensure_user_directory(base: &Path, child: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Component;

    fn validate(directory: &Path) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = fs::symlink_metadata(directory)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "`{}` is a symlink; refusing to use an unsafe directory",
                    directory.display()
                ),
            ));
        }
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("`{}` is not a directory", directory.display()),
            ));
        }
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "`{}` is owned by uid {}, not the effective uid {}",
                    directory.display(),
                    metadata.uid(),
                    effective_uid
                ),
            ));
        }
        let mode = metadata.mode() & 0o777;
        if mode & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "`{}` has mode {mode:04o}; it must not be writable by group or other",
                    directory.display()
                ),
            ));
        }
        Ok(())
    }

    /// Creates or validates every component of `relative` below `anchor`.
    fn descend(anchor: &Path, relative: &Path) -> io::Result<()> {
        let mut current = anchor.to_path_buf();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{}` is not a plain directory path", relative.display()),
                ));
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(_) => validate(&current)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                    // `mkdir` itself is masked by the umask, so the mode is corrected afterwards.
                    // Only the directory just created is touched; a pre-existing one is never
                    // chmodded.
                    fs::set_permissions(&current, fs::Permissions::from_mode(0o755))?;
                    validate(&current)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    // The deepest existing ancestor is the anchor. Nothing above it is ours to validate, but the
    // base itself and every existing component down to the child are.
    let mut anchor = base.to_path_buf();
    loop {
        match fs::symlink_metadata(&anchor) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "`{}` is a symlink; refusing to use an unsafe directory",
                        anchor.display()
                    ),
                ));
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("`{}` is not a directory", anchor.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => match anchor.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    anchor = parent.to_path_buf();
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "`{}` has no existing ancestor to create it under",
                            base.display()
                        ),
                    ));
                }
            },
            Err(error) => return Err(error),
        }
    }
    let relative = base.strip_prefix(&anchor).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`{}` is not below its existing ancestor `{}`",
                base.display(),
                anchor.display()
            ),
        )
    })?;
    descend(&anchor, relative)?;
    // `descend` is a no-op when the base already exists (`anchor == base`), which is exactly the
    // case where a group-writable `~/.config` or `~/.local/share` would otherwise go unchecked.
    // Validating the base unconditionally also covers a base created by `descend` above.
    validate(base)?;

    let child_path = Path::new(child);
    if !matches!(
        child_path.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{child}` is not a single directory name"),
        ));
    }
    let target = base.join(child_path);
    match fs::symlink_metadata(&target) {
        Ok(_) => validate(&target)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&target)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
            validate(&target)?;
        }
        Err(error) => return Err(error),
    }
    Ok(target)
}

/// Quotes one `Exec` argument and escapes the characters the Desktop Entry spec reserves.
///
/// The value is always quoted, so spaces and empty arguments survive. `%` becomes `%%` because a
/// field code is expanded even inside quotes, and `"`, `` ` ``, `$` and `\` are escaped inside a
/// quoted argument. Control characters (including `\n`) are rejected: they cannot appear in a
/// desktop-entry value and would inject keys.
#[cfg(target_os = "linux")]
pub(crate) fn quote_exec_arg(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters".to_owned());
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '%' => quoted.push_str("%%"),
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '`' | '$' => {
                quoted.push('\\');
                quoted.push(character);
            }
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

/// Whether `name` is safe to use as one desktop-entry file name.
///
/// A single path component keeps the file inside the platform's autostart directory; control
/// characters and `.`/`..` are rejected alongside separators.
pub(crate) fn validate_app_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err("must be a single file name, not a path".to_owned());
    }
    if name.chars().any(char::is_control) {
        return Err("must not contain control characters".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn quotes_spaces_and_escapes_field_codes() {
        assert_eq!(
            quote_exec_arg("/opt/My App/app").unwrap(),
            "\"/opt/My App/app\""
        );
        assert_eq!(
            quote_exec_arg("/opt/100%fapp").unwrap(),
            "\"/opt/100%%fapp\""
        );
        assert_eq!(
            quote_exec_arg(r#"/opt/a"b\c$d`e"#).unwrap(),
            "\"/opt/a\\\"b\\\\c\\$d\\`e\""
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_newlines_that_would_inject_a_key() {
        assert!(quote_exec_arg("/opt/evil\nHidden=true").is_err());
    }

    #[test]
    fn app_names_cannot_escape_the_autostart_directory() {
        assert!(validate_app_name("com.example.App").is_ok());
        assert!(validate_app_name("../escaped").is_err());
        assert!(validate_app_name("nested/name").is_err());
        assert!(validate_app_name("..").is_err());
        assert!(validate_app_name("").is_err());
    }
}
