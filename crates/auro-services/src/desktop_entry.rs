// SPDX-License-Identifier: MIT OR Apache-2.0

//! Desktop Entry escaping shared by deep-link registration and autostart.

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
