// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deep links / URL schemes.
//!
//! Auro accepts a limited set of schemes, parses links into their parts, and queues them until the
//! app asks. Scheme registration is delegated to the OS; the packager remains responsible for the
//! macOS `CFBundleURLTypes` entry.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

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
    /// The percent-decoded host, validated as a DNS-style label sequence or IPv6 literal.
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
///
/// Clones share the same queue, so an app can keep one handle for draining while the facade
/// feeds launch/instance arguments into another.
#[derive(Clone, Default)]
pub struct AppLinks {
    queue: Arc<Mutex<VecDeque<DeepLink>>>,
    schemes: Arc<[String]>,
}

impl AppLinks {
    /// Creates a queue that accepts the given schemes.
    pub fn new(schemes: Vec<String>) -> Self {
        AppLinks {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            schemes: schemes.into(),
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

    // Clones intentionally share `queue`; this is what lets `App::deep_links` feed the app's
    // handle without a second consumer.
}

fn validate_scheme(scheme: &str) -> Result<(), DeepLinkError> {
    // RFC 3986: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ). `_` is not allowed, `+` is.
    let mut characters = scheme.chars();
    let starts_with_letter = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic());
    if !starts_with_letter
        || !characters.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
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
    if !valid_host(&host) {
        return None;
    }

    let mut segments = Vec::new();
    for segment in parts.filter(|segment| !segment.is_empty()) {
        let segment = percent_decode(segment)?;
        if !valid_segment(&segment) {
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

/// Whether a percent-decoded deep-link path segment is safe to hand to an app.
///
/// A colon is either an NTFS alternate-data-stream separator or a Windows drive prefix; on Unix it
/// is harmless, but rejecting it keeps the cross-platform contract. Windows also trims trailing
/// dots and spaces from each component (without the `\\?\` prefix), so `.. ` and `%2e%2e%20` would
/// resolve as `..` in a downstream app; such segments are rejected before the app sees them, as are
/// the reserved device names that Windows cannot create in a directory (#101).
fn valid_segment(segment: &str) -> bool {
    if segment.is_empty()
        || segment.ends_with('.')
        || segment.ends_with(' ')
        || segment.contains('/')
        || segment.contains('\\')
        || segment.contains(':')
    {
        return false;
    }
    let trimmed = segment.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return false;
    }
    if std::path::Path::new(segment).components().any(|component| {
        matches!(
            component,
            std::path::Component::Prefix(_)
                | std::path::Component::RootDir
                | std::path::Component::ParentDir
        )
    }) {
        return false;
    }
    !is_windows_reserved(trimmed)
}

/// Whether `name` is a reserved Windows device name (`CON`, `NUL`, `COM1`-`COM9`, ...).
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

/// Whether a percent-decoded deep-link host is a single DNS-style label sequence or IPv6 literal.
///
/// Deep links are external input. A host that carries path separators, traversal components or
/// non-host characters would let an app that uses it as a path or file key escape its directory
/// (#85). Unicode IDNs are rejected; callers can percent-encode/punycode at the boundary.
fn valid_host(host: &str) -> bool {
    if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        // Only a real IPv6 literal is acceptable. `[..]`, `[C:]` and `[deadbeef]` are bracketed
        // text, not addresses, and an app that strips the brackets would receive a traversal or
        // drive-prefixed host (#95).
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && !host.starts_with('.')
        && !host.contains("..")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        })
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
    let data_home = linux_data_base()?;
    register_linux_in(
        &data_home,
        scheme,
        executable,
        std::path::Path::new("xdg-mime"),
    )
}

/// The XDG data base the `applications` directory lives under.
#[cfg(target_os = "linux")]
fn linux_data_base() -> Result<std::path::PathBuf, DeepLinkError> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .ok_or_else(|| DeepLinkError::Register("HOME and XDG_DATA_HOME are unset".to_owned()))
}

#[cfg(target_os = "linux")]
fn register_linux_in(
    data_home: &std::path::Path,
    scheme: &str,
    executable: &std::path::Path,
    program: &std::path::Path,
) -> Result<(), DeepLinkError> {
    // Create and validate `applications` before the registration program runs: a group-writable
    // directory would let another local user replace the handler `.desktop` file (#92).
    let applications = crate::desktop_entry::ensure_user_directory(data_home, "applications")
        .map_err(|error| DeepLinkError::Register(error.to_string()))?;
    install_linux_registration(&applications, scheme, executable, program)
}

#[cfg(target_os = "linux")]
fn install_linux_registration(
    applications: &std::path::Path,
    scheme: &str,
    executable: &std::path::Path,
    program: &std::path::Path,
) -> Result<(), DeepLinkError> {
    let desktop_name = format!("auro-{scheme}.desktop");
    let desktop_path = applications.join(&desktop_name);

    // Keep the previous handler so a failed rewrite can put it back. A symlink is rejected
    // rather than read or followed (#84, #87).
    let previous = match std::fs::symlink_metadata(&desktop_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(DeepLinkError::Register(format!(
                "`{}` is a symlink",
                desktop_path.display()
            )));
        }
        Ok(metadata) if metadata.is_file() => Some(
            std::fs::read_to_string(&desktop_path)
                .map_err(|error| DeepLinkError::Register(error.to_string()))?,
        ),
        Ok(_) => {
            return Err(DeepLinkError::Register(format!(
                "`{}` is not a regular file",
                desktop_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(DeepLinkError::Register(error.to_string())),
    };

    let entry = linux_desktop_entry(scheme, executable)?;
    crate::desktop_entry::write_atomic(&desktop_path, &entry)
        .map_err(|error| DeepLinkError::Register(error.to_string()))?;

    if let Err(error) = finish_linux_registration(&desktop_name, scheme, program) {
        let restored = match previous {
            Some(previous) => crate::desktop_entry::write_atomic(&desktop_path, &previous)
                .map_err(|restore| restore.to_string()),
            None => std::fs::remove_file(&desktop_path).map_err(|restore| restore.to_string()),
        };
        if let Err(restore) = restored {
            return Err(DeepLinkError::Register(format!(
                "{error}; restoring the previous handler failed: {restore}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn finish_linux_registration(
    desktop_name: &str,
    scheme: &str,
    program: &std::path::Path,
) -> Result<(), DeepLinkError> {
    let output = std::process::Command::new(program)
        .args([
            "default",
            desktop_name,
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

#[cfg(target_os = "linux")]
fn linux_desktop_entry(
    scheme: &str,
    executable: &std::path::Path,
) -> Result<String, DeepLinkError> {
    let exec = crate::desktop_entry::quote_exec_arg(&executable.to_string_lossy())
        .map_err(DeepLinkError::Register)?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Auro {scheme}\nExec={exec} %u\nTerminal=false\nMimeType=x-scheme-handler/{scheme};\n"
    ))
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
    fn clones_share_the_same_queue() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        let other = links.clone();
        links.push("auro://example.com/from-primary");
        other.push("auro://example.com/from-instance");
        let drained: Vec<_> = links.drain().into_iter().map(|link| link.raw).collect();
        assert_eq!(
            drained,
            [
                "auro://example.com/from-primary",
                "auro://example.com/from-instance"
            ]
        );
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
        for scheme in ["", "auro /x", "1bad", "bad_scheme"] {
            assert!(validate_scheme(scheme).is_err(), "{scheme}");
        }
    }

    #[test]
    fn accepts_a_scheme_with_a_plus() {
        validate_scheme("probe+app").expect("+ is legal in a scheme");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_desktop_entry_escapes_field_codes_and_rejects_newlines() {
        let entry = linux_desktop_entry("probe", std::path::Path::new("/opt/100%fapp"))
            .expect("desktop entry");
        assert!(entry.contains("Exec=\"/opt/100%%fapp\" %u"), "{entry}");

        assert!(
            linux_desktop_entry("probe", std::path::Path::new("/opt/evil\nHidden=true")).is_err()
        );
    }

    #[test]
    fn rejects_a_traversal_host() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        for raw in [
            "auro://..%2F..%2Fetc%2Fpasswd",
            "auro://..",
            "auro://.",
            "auro://example..com",
            "auro://a%5Cb",
        ] {
            links.push(raw);
        }
        assert!(links.drain().is_empty());
    }

    #[test]
    fn accepts_dns_and_ipv6_hosts() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        links.push("auro://example.com/path");
        links.push("auro://example.com./path");
        links.push("auro://[2001:db8::1]/path");
        assert_eq!(links.drain().len(), 3);
    }

    #[test]
    fn rejects_non_ipv6_bracketed_hosts() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        for raw in [
            "auro://[..]",
            "auro://[C:]",
            "auro://[C:..]",
            "auro://[deadbeef]",
        ] {
            links.push(raw);
        }
        assert!(links.drain().is_empty());
    }

    #[test]
    fn accepts_ipv6_literal_hosts() {
        let links = AppLinks::new(vec!["auro".to_owned()]);
        links.push("auro://[::1]/path");
        links.push("auro://[2001:db8::1]/path");
        assert_eq!(links.drain().len(), 2);
    }

    #[test]
    fn rejects_drive_prefixed_and_encoded_separator_segments() {
        let links = AppLinks::new(vec!["probe".to_owned()]);
        for raw in [
            "probe://host/C:/Windows/win.ini",
            "probe://host/a%3Ab",
            "probe://host/..%2fetc",
        ] {
            links.push(raw);
        }
        assert!(links.drain().is_empty());
    }

    #[test]
    fn accepts_ordinary_path_segments() {
        let links = AppLinks::new(vec!["probe".to_owned()]);
        links.push("probe://host/docs/v1.2/file_name");
        let links = links.drain();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].segments, ["docs", "v1.2", "file_name"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn applications_directory_is_created_private_under_umask() {
        use std::os::unix::fs::PermissionsExt as _;

        let _lock = crate::test_support::lock();
        // Create the base before changing the mask: the managed base is validated, and a tempdir
        // created under `umask 002` would be 0775 and correctly refused.
        let data_home = tempfile::tempdir().expect("tempdir");
        let _umask = crate::test_support::UmaskGuard::set(0o002);
        let error = register_linux_in(
            data_home.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("missing xdg-mime");
        assert!(matches!(error, DeepLinkError::Register(_)));

        let applications = data_home.path().join("applications");
        let mode = std::fs::metadata(&applications)
            .expect("applications directory")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "the applications directory must not inherit the umask"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn registration_refuses_a_group_writable_applications_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let _lock = crate::test_support::lock();
        let data_home = tempfile::tempdir().expect("tempdir");
        let applications = data_home.path().join("applications");
        std::fs::create_dir(&applications).expect("applications");
        std::fs::set_permissions(&applications, std::fs::Permissions::from_mode(0o775))
            .expect("mode");

        let error = register_linux_in(
            data_home.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("a group-writable directory must be refused");
        assert!(
            error
                .to_string()
                .contains(&applications.display().to_string()),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn registration_refuses_a_symlinked_applications_directory() {
        use std::os::unix::fs::symlink;

        let _lock = crate::test_support::lock();
        let data_home = tempfile::tempdir().expect("tempdir");
        let victim = tempfile::tempdir().expect("tempdir");
        let applications = data_home.path().join("applications");
        symlink(victim.path(), &applications).expect("symlink");

        let error = register_linux_in(
            data_home.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("a symlinked directory must be refused");
        assert!(
            error
                .to_string()
                .contains(&applications.display().to_string()),
            "{error}"
        );
        assert!(
            std::fs::read_dir(victim.path())
                .expect("victim directory")
                .next()
                .is_none(),
            "must not write through the symlink"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn registration_refuses_a_group_writable_data_base() {
        use std::os::unix::fs::PermissionsExt as _;

        let _lock = crate::test_support::lock();
        let parent = tempfile::tempdir().expect("tempdir");
        let data_home = parent.path().join("data");
        std::fs::create_dir(&data_home).expect("data base");
        std::fs::set_permissions(&data_home, std::fs::Permissions::from_mode(0o775)).expect("mode");

        let error = register_linux_in(
            &data_home,
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("a group-writable managed base must be refused");
        assert!(
            error.to_string().contains(&data_home.display().to_string()),
            "{error}"
        );
        assert!(
            error.to_string().contains("mode 0775"),
            "the error must name the mode: {error}"
        );
    }

    #[test]
    fn rejects_segments_that_windows_normalizes_to_traversal() {
        let links = AppLinks::new(vec!["probe".to_owned()]);
        for raw in [
            "probe://host/..%20/x",
            "probe://host/%2e%2e%20/x",
            "probe://host/.../x",
            "probe://host/foo./x",
            "probe://host/foo%20/x",
        ] {
            links.push(raw);
        }
        assert!(links.drain().is_empty());
    }

    #[test]
    fn rejects_windows_reserved_segments() {
        let links = AppLinks::new(vec!["probe".to_owned()]);
        for raw in [
            "probe://host/CON",
            "probe://host/nul.txt",
            "probe://host/COM1",
            "probe://host/lpt9.log",
            "probe://host/Aux ",
        ] {
            links.push(raw);
        }
        assert!(links.drain().is_empty());
    }

    #[test]
    fn accepts_segments_with_internal_dots() {
        let links = AppLinks::new(vec!["probe".to_owned()]);
        links.push("probe://host/v1.2/report.final/.hidden");
        let links = links.drain();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].segments, ["v1.2", "report.final", ".hidden"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_xdg_mime_removes_a_new_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let desktop = temp.path().join("auro-probe.desktop");
        let error = install_linux_registration(
            temp.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("missing xdg-mime");
        assert!(matches!(error, DeepLinkError::Register(_)));
        assert!(
            !desktop.exists(),
            "a failed first registration must not leave a handler"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_reregistration_restores_the_previous_handler() {
        let temp = tempfile::tempdir().expect("tempdir");
        let desktop = temp.path().join("auro-probe.desktop");
        std::fs::write(&desktop, b"previous handler").expect("previous");
        install_linux_registration(
            temp.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("missing xdg-mime");
        assert_eq!(
            std::fs::read(&desktop).expect("desktop"),
            b"previous handler"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn registration_rejects_a_symlinked_desktop_entry() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let victim = temp.path().join("victim");
        std::fs::write(&victim, b"SAFE").expect("victim");
        let desktop = temp.path().join("auro-probe.desktop");
        symlink(&victim, &desktop).expect("symlink");
        install_linux_registration(
            temp.path(),
            "probe",
            std::path::Path::new("/tmp/app"),
            std::path::Path::new("/definitely/missing/xdg-mime"),
        )
        .expect_err("symlinked handler");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"SAFE");
        assert!(
            std::fs::symlink_metadata(&desktop)
                .expect("desktop")
                .file_type()
                .is_symlink()
        );
    }
}
