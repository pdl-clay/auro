// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deep links / URL schemes.
//!
//! Auro accepts a limited set of schemes, parses links into their parts, and queues them until the
//! app asks. Scheme registration is delegated to the OS; the packager remains responsible for the
//! macOS `CFBundleURLTypes` entry.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

/// Why a deep link operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeepLinkError {
    #[error("scheme registration failed: {0}")]
    Register(String),

    #[error("invalid scheme `{0}`")]
    InvalidScheme(String),
}

/// A parsed deep link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepLink {
    /// The lowercased scheme.
    pub scheme: String,
    /// The percent-decoded host.
    pub host: String,
    /// The percent-decoded path segments.
    pub segments: Vec<String>,
    /// The percent-decoded query parameters.
    pub query: BTreeMap<String, String>,
    /// The original argument.
    pub raw: String,
}

/// Registers a URL scheme for the current user.
///
/// Windows writes an `HKCU\Software\Classes\<scheme>` entry. Linux writes an XDG desktop entry and
/// asks `xdg-mime` to make it the default handler. macOS expects the bundle's `CFBundleURLTypes`
/// to be set by the packager and returns `Ok(())`.
pub fn register_scheme(scheme: &str, executable: &std::path::Path) -> Result<(), DeepLinkError> {
    validate_scheme(scheme)?;
    #[cfg(windows)]
    register_windows(scheme, executable)?;
    #[cfg(target_os = "linux")]
    register_linux(scheme, executable)?;
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = executable;
    }
    Ok(())
}

/// Links wait here until the app asks for them.
#[derive(Default)]
pub struct AppLinks {
    queue: Mutex<VecDeque<DeepLink>>,
    schemes: Vec<String>,
}

impl AppLinks {
    /// Creates a queue that accepts the given schemes.
    pub fn new(schemes: Vec<String>) -> Self {
        AppLinks {
            queue: Mutex::new(VecDeque::new()),
            schemes,
        }
    }

    /// Adds one raw argument to the queue if it is an accepted scheme.
    pub fn push(&self, raw: &str) {
        if let Some(link) = parse(raw, &self.schemes) {
            self.queue.lock().expect("deep link queue").push_back(link);
        }
    }

    /// Adds several raw arguments to the queue.
    pub fn push_raw(&self, raw: &[String]) {
        for value in raw {
            self.push(value);
        }
    }

    /// Drains the queued links in arrival order.
    pub fn drain(&self) -> Vec<DeepLink> {
        self.queue
            .lock()
            .expect("deep link queue")
            .drain(..)
            .collect()
    }

    /// The schemes this queue accepts.
    pub fn schemes(&self) -> &[String] {
        &self.schemes
    }
}

fn validate_scheme(scheme: &str) -> Result<(), DeepLinkError> {
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(DeepLinkError::InvalidScheme(scheme.to_owned()));
    }
    Ok(())
}

fn parse(raw: &str, schemes: &[String]) -> Option<DeepLink> {
    const MAX_LEN: usize = 4096;
    let raw = raw.trim();
    if raw.len() > MAX_LEN || raw.chars().any(char::is_control) {
        return None;
    }

    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !schemes
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(&scheme))
    {
        return None;
    }

    let rest = rest.split('#').next()?;
    let (path_part, query_part) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let mut parts = path_part.split('/');
    let host = percent_decode(parts.next()?)?;
    if host.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    for segment in parts.filter(|segment| !segment.is_empty()) {
        let segment = percent_decode(segment)?;
        if segment == "." || segment == ".." || segment.contains('/') || segment.contains('\\') {
            return None;
        }
        segments.push(segment);
    }

    let mut query = BTreeMap::new();
    if let Some(query_text) = query_part {
        for pair in query_text.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            query.insert(
                percent_decode(&key.replace('+', " "))?,
                percent_decode(&value.replace('+', " "))?,
            );
        }
    }

    Some(DeepLink {
        scheme,
        host,
        segments,
        query,
        raw: raw.to_owned(),
    })
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = (bytes[index + 1] as char).to_digit(16)?;
            let low = (bytes[index + 2] as char).to_digit(16)?;
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.chars().any(char::is_control)).then_some(decoded)
}

#[cfg(windows)]
fn register_windows(scheme: &str, executable: &std::path::Path) -> Result<(), DeepLinkError> {
    let base = format!(r"HKCU\Software\Classes\{scheme}");
    let command = format!("\"{}\" \"%1\"", executable.display());
    let commands: [&[&str]; 3] = [
        &["ADD", &base, "/ve", "/d", &format!("URL:{scheme}"), "/f"],
        &["ADD", &base, "/v", "URL Protocol", "/d", "", "/f"],
        &[
            "ADD",
            &format!(r"{base}\shell\open\command"),
            "/ve",
            "/d",
            &command,
            "/f",
        ],
    ];
    for args in commands {
        let output = std::process::Command::new("reg.exe")
            .args(args)
            .output()
            .map_err(|error| DeepLinkError::Register(error.to_string()))?;
        if !output.status.success() {
            return Err(DeepLinkError::Register(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn register_linux(scheme: &str, executable: &std::path::Path) -> Result<(), DeepLinkError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .ok_or_else(|| DeepLinkError::Register("HOME and XDG_DATA_HOME are unset".to_owned()))?;
    let applications = data_home.join("applications");
    std::fs::create_dir_all(&applications)
        .map_err(|error| DeepLinkError::Register(error.to_string()))?;

    let desktop_name = format!("auro-{scheme}.desktop");
    let desktop_path = applications.join(&desktop_name);
    let exec = executable.to_string_lossy().replace('"', "\\\"");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Auro {scheme}\nExec=\"{exec}\" %u\nTerminal=false\nMimeType=x-scheme-handler/{scheme};\n"
    );
    std::fs::write(&desktop_path, entry)
        .map_err(|error| DeepLinkError::Register(error.to_string()))?;

    let output = std::process::Command::new("xdg-mime")
        .args([
            "default",
            &desktop_name,
            &format!("x-scheme-handler/{scheme}"),
        ])
        .output()
        .map_err(|error| DeepLinkError::Register(error.to_string()))?;
    if !output.status.success() {
        return Err(DeepLinkError::Register(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_deep_link() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        links.push("auro://example.com/path?token=abc");
        let links = links.drain();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].scheme, "auro");
        assert_eq!(links[0].host, "example.com");
        assert_eq!(links[0].segments, ["path"]);
        assert_eq!(links[0].query.get("token"), Some(&"abc".to_owned()));
    }

    #[test]
    fn rejects_an_unknown_scheme() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        links.push("other://example.com");
        assert!(links.drain().is_empty());
    }

    #[test]
    fn rejects_a_traversal_path() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        links.push("auro://example.com/../secret");
        assert!(links.drain().is_empty());
    }

    #[test]
    fn rejects_an_empty_or_bad_scheme() {
        assert!(matches!(
            register_scheme("", std::path::Path::new("/tmp/app")),
            Err(DeepLinkError::InvalidScheme(_))
        ));
        assert!(matches!(
            register_scheme("auro /x", std::path::Path::new("/tmp/app")),
            Err(DeepLinkError::InvalidScheme(_))
        ));
    }
}
