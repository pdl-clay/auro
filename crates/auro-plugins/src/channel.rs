// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private local-socket endpoint for one plugin instance.
//!
//! The endpoint name contains an eight-hex id tag and sixteen hex digits of a 32-hex secret. The
//! full secret lives in a token file inside a per-user private directory; no globally predictable
//! name is ever bound. The short name keeps Unix `sun_path` within its 104-byte limit.
//! Stage 3 adds peer-credential checks on accepted connections.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::host::HostError;
use interprocess::local_socket::traits::{Listener as _, Stream as _};
#[cfg(not(windows))]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use interprocess::local_socket::{ListenerOptions, Stream};

/// Length in lowercase hex digits of a channel secret.
const SECRET_HEX_LEN: usize = 32;

/// Generates a fresh secret for one plugin channel.
///
/// `RandomState` is seeded from the operating system and is mixed with the current time and the
/// process id into two independent `u64` values. This avoids a new cryptographic dependency while
/// keeping the name unguessable to another local user; the private directory keeps the token away
/// from other users entirely. Stage 3 may replace the generator with a CSPRNG without changing the
/// wire contract.
pub fn generate_secret() -> String {
    let state = RandomState::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();

    let mut first = state.build_hasher();
    now.as_nanos().hash(&mut first);
    pid.hash(&mut first);
    let low = first.finish();

    let mut second = state.build_hasher();
    now.as_nanos().rotate_left(43).hash(&mut second);
    pid.rotate_left(17).hash(&mut second);
    let high = second.finish();

    format!(
        "{}{}",
        crate::hex::encode(low.to_le_bytes()),
        crate::hex::encode(high.to_le_bytes())
    )
}

/// Returns the per-user directory used for plugin tokens and sockets.
///
/// Unix uses `$XDG_RUNTIME_DIR/auro-plugin` when that variable is an absolute path to an already
/// safe directory, otherwise an `auro-<uid>` directory under the system temporary directory. The
/// inherited value is validated before it is trusted: a symlink, a foreign owner or group/other
/// write bits must not redirect the channel before [`PluginChannel::new`] runs.
#[cfg(unix)]
pub fn default_private_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        let runtime = PathBuf::from(runtime);
        if runtime.is_absolute() && validate_existing_private_dir(&runtime).is_ok() {
            return runtime.join("auro-plugin");
        }
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("auro-{uid}"))
}

/// Returns the per-user directory used for plugin tokens and sockets.
///
/// Windows uses `%LOCALAPPDATA%\auro\plugin`, falling back to the system temporary directory.
/// The directory is created and validated by [`PluginChannel::new`].
#[cfg(windows)]
pub fn default_private_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("auro")
        .join("plugin")
}

/// Returns the per-user directory used for plugin tokens and sockets.
///
/// Platforms other than Unix and Windows use a directory under the system temporary directory.
#[cfg(not(any(unix, windows)))]
pub fn default_private_dir() -> PathBuf {
    std::env::temp_dir().join("auro-plugin")
}

/// A bound endpoint and the private directory that guards it.
///
/// The listener is created before the child is spawned; the endpoint name is passed to the child
/// through an environment variable. Each instance gets a new secret, so a stale listener cannot
/// impersonate a later instance.
#[derive(Debug)]
pub struct PluginChannel {
    endpoint: String,
    dir: PathBuf,
    secret: String,
    listener: interprocess::local_socket::Listener,
}

impl PluginChannel {
    /// Creates the private directory, validates it, and binds a listener for `id` with `secret`.
    ///
    /// `secret` must be exactly [`SECRET_HEX_LEN`] lowercase hex digits, as produced by
    /// [`generate_secret`]. `dir` is a per-user directory supplied by the caller. A short or
    /// predictable name, an occupied name or a denied bind is a typed [`HostError`].
    pub fn new(dir: impl AsRef<Path>, id: &str, secret: &str) -> Result<Self, HostError> {
        crate::validate::plugin_id(id).map_err(|reason| HostError::InvalidPluginId {
            id: id.to_owned(),
            reason: reason.to_owned(),
        })?;
        if !is_secret(secret) {
            return Err(HostError::PredictableName);
        }

        let dir = dir.as_ref().to_path_buf();
        prepare_private_dir(&dir)?;
        let endpoint = endpoint_name(&dir, id, secret)?;
        write_token(&dir, id, secret)?;

        let name = name_for_endpoint(&endpoint)?;
        let listener = ListenerOptions::new()
            .name(name)
            .reclaim_name(false)
            .try_overwrite(false)
            .create_sync()
            .map_err(map_channel_error)?;
        #[cfg(unix)]
        restrict_socket_mode(&endpoint)?;

        Ok(Self {
            endpoint,
            dir,
            secret: secret.to_owned(),
            listener,
        })
    }

    /// The endpoint string to pass to the child through `AURO_PLUGIN_ENDPOINT`.
    pub fn endpoint_name(&self) -> &str {
        &self.endpoint
    }

    /// The validated private directory holding this instance's token and socket.
    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// The secret embedded in the endpoint name.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// Accepts one plugin connection, blocking until the child connects.
    ///
    /// Stage 3 adds timeouts and bounded admission; this stage accepts a single connection.
    pub fn accept(&self) -> Result<Stream, HostError> {
        self.listener.accept().map_err(HostError::ChannelIo)
    }
}

/// Connects a plugin-side fixture or SDK client to a host endpoint.
///
/// The name is opaque to callers: it is either a private filesystem socket path (Unix) or a
/// private named pipe (Windows). No global or predictable name is constructed here.
pub fn connect(endpoint_name: &str) -> Result<Stream, HostError> {
    if endpoint_name.len() < 24 {
        return Err(HostError::PredictableName);
    }
    let name = name_for_endpoint(endpoint_name)?;
    Stream::connect(name).map_err(HostError::ChannelIo)
}

fn is_secret(secret: &str) -> bool {
    secret.len() == SECRET_HEX_LEN
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn endpoint_name(
    #[cfg_attr(windows, allow(unused_variables))] dir: &Path,
    id: &str,
    secret: &str,
) -> Result<String, HostError> {
    // The id is hashed to keep the path within sun_path; sixteen secret hex digits still give
    // 64 bits of unpredictability to another local user.
    use sha2::{Digest as _, Sha256};
    let tag = format!(
        "{}-{}",
        &crate::hex::encode(Sha256::digest(id.as_bytes()))[..8],
        &secret[..16]
    );
    #[cfg(windows)]
    {
        Ok(format!(r"\\.\pipe\auro-{tag}"))
    }
    #[cfg(not(windows))]
    {
        let path = dir.join(format!("auro-{tag}.sock"));
        path.into_os_string()
            .into_string()
            .map_err(|_| HostError::InvalidChannelName)
    }
}

fn name_for_endpoint(endpoint: &str) -> Result<interprocess::local_socket::Name<'_>, HostError> {
    #[cfg(windows)]
    {
        endpoint
            .to_ns_name::<GenericNamespaced>()
            .map_err(HostError::ChannelIo)
    }
    #[cfg(not(windows))]
    {
        endpoint
            .to_fs_name::<GenericFilePath>()
            .map_err(HostError::ChannelIo)
    }
}

fn map_channel_error(error: io::Error) -> HostError {
    match error.kind() {
        io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied => {
            HostError::ChannelConflict(error)
        }
        _ => HostError::ChannelIo(error),
    }
}

/// Validates an existing private directory without changing it.
///
/// This runs before any `mkdir` or `chmod`: `create_dir_all` and `set_permissions` both follow a
/// pre-existing symlink, so mutating first would change the target even when the directory is later
/// rejected. The caller must use a path obtained from `symlink_metadata`.
#[cfg(unix)]
fn validate_existing_private_dir(path: &Path) -> Result<std::fs::Metadata, HostError> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(path).map_err(HostError::PrivateDirectory)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(HostError::InsecureDirectory(path.display().to_string()));
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(HostError::InsecureDirectory(path.display().to_string()));
    }
    Ok(meta)
}

/// Validates a platform-owned ancestor before any mutation of paths beneath it.
///
/// A shared ancestor such as `/tmp` is legitimately mode `1777` (sticky) or a group-readable
/// runtime directory. It is not the trust boundary: requiring owner/private bits here would make
/// the per-user fallback impossible. Only a real directory (no symlink or reparse point) is
/// required; the managed `auro-<uid>` directory is validated strictly below.
#[cfg(unix)]
fn validate_platform_ancestor(path: &Path) -> Result<(), HostError> {
    let meta = std::fs::symlink_metadata(path).map_err(HostError::PrivateDirectory)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(HostError::InsecureDirectory(path.display().to_string()));
    }
    Ok(())
}

fn prepare_private_dir(dir: &Path) -> Result<(), HostError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        // Find the deepest existing ancestor. It may be a shared platform directory (e.g. sticky
        // `/tmp`), so only symlinks/non-directories are rejected here. The managed directory below
        // it gets the strict owner/mode check.
        let mut ancestor = dir.to_path_buf();
        loop {
            match std::fs::symlink_metadata(&ancestor) {
                Ok(_) => {
                    validate_platform_ancestor(&ancestor)?;
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        ancestor = parent.to_path_buf();
                    }
                    _ => return Err(HostError::PrivateDirectory(error)),
                },
                Err(error) => return Err(HostError::PrivateDirectory(error)),
            }
        }
        let nearest = ancestor;
        let relative = dir.strip_prefix(&nearest).map_err(|_| {
            HostError::PrivateDirectory(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{}` is not below `{}`", dir.display(), nearest.display()),
            ))
        })?;
        let mut current = nearest;
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(_) => {
                    validate_existing_private_dir(&current)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(map_channel_error)?;
                    // Only directories created here are chmodded; an existing one was validated
                    // above and is never modified.
                    std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700))
                        .map_err(HostError::PrivateDirectory)?;
                    validate_existing_private_dir(&current)?;
                }
                Err(error) => return Err(HostError::PrivateDirectory(error)),
            }
        }

        let meta = validate_existing_private_dir(dir)?;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(dir)
            .map_err(HostError::PrivateDirectory)?;
        let opened = handle.metadata().map_err(HostError::PrivateDirectory)?;
        if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
            return Err(HostError::InsecureDirectory(dir.display().to_string()));
        }
    }
    #[cfg(windows)]
    {
        // Reject a reparse point before creating or chmodding anything under it.
        let mut ancestor = dir.to_path_buf();
        loop {
            match std::fs::symlink_metadata(&ancestor) {
                Ok(_) => {
                    if is_reparse_point(&ancestor).map_err(HostError::PrivateDirectory)? {
                        return Err(HostError::InsecureDirectory(ancestor.display().to_string()));
                    }
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        ancestor = parent.to_path_buf();
                    }
                    _ => return Err(HostError::PrivateDirectory(error)),
                },
                Err(error) => return Err(HostError::PrivateDirectory(error)),
            }
        }
        std::fs::create_dir_all(dir).map_err(map_channel_error)?;
        if is_reparse_point(dir).map_err(HostError::PrivateDirectory)? {
            return Err(HostError::InsecureDirectory(dir.display().to_string()));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::create_dir_all(dir).map_err(map_channel_error)?;
    }
    Ok(())
}

fn write_token(dir: &Path, id: &str, secret: &str) -> Result<(), HostError> {
    let path = dir.join(format!("{id}.{secret}.token"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(map_channel_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(HostError::PrivateDirectory)?;
    }
    file.write_all(secret.as_bytes())
        .map_err(HostError::ChannelIo)?;
    file.flush().map_err(HostError::ChannelIo)?;
    validate_token(&path, secret)
}

fn validate_token(path: &Path, expected: &str) -> Result<(), HostError> {
    let meta = std::fs::symlink_metadata(path).map_err(HostError::PrivateDirectory)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(HostError::InsecureToken(path.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(HostError::InsecureToken(path.display().to_string()));
        }
    }
    #[cfg(windows)]
    {
        if is_reparse_point(path).map_err(HostError::PrivateDirectory)? {
            return Err(HostError::InsecureToken(path.display().to_string()));
        }
    }
    let mut content = String::new();
    std::fs::File::open(path)
        .and_then(|mut file| file.read_to_string(&mut content))
        .map_err(HostError::PrivateDirectory)?;
    if content != expected {
        return Err(HostError::InsecureToken(path.display().to_string()));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_socket_mode(path: &str) -> Result<(), HostError> {
    use std::ffi::CString;
    use std::os::unix::fs::PermissionsExt;

    let c_path = CString::new(path.as_bytes()).map_err(|_| HostError::InvalidChannelName)?;
    // SAFETY: c_path is a live, NUL-terminated string and chmod has no other preconditions.
    let result = unsafe { libc::chmod(c_path.as_ptr(), 0o600) };
    if result != 0 {
        return Err(HostError::ChannelIo(io::Error::last_os_error()));
    }
    let meta = std::fs::symlink_metadata(path).map_err(HostError::ChannelIo)?;
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(HostError::InsecureDirectory(path.to_owned()));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(path: &Path) -> io::Result<bool> {
    use std::os::windows::fs::MetadataExt;

    // FILE_ATTRIBUTE_REPARSE_POINT is 0x400 in the Win32 API.
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    Ok(std::fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_32_lowercase_hex() {
        let secret = generate_secret();
        assert!(is_secret(&secret), "{secret}");
        assert!(!is_secret("short"));
        assert!(!is_secret(&"A".repeat(SECRET_HEX_LEN)));
    }
}
