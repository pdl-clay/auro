// SPDX-License-Identifier: MIT OR Apache-2.0

//! Desktop Entry escaping shared by deep-link registration and autostart.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::path::Path;

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
