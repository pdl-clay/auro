// SPDX-License-Identifier: MIT OR Apache-2.0

//! A process-wide single-instance guard and forwarding channel.
//!
//! The guard is an advisory file lock in a per-user runtime directory. The OS releases the lock
//! when the process exits, including a crash; the lock file itself is left in place. The primary
//! instance listens on a local socket; a secondary instance forwards its arguments and working
//! directory before exiting.

#[cfg(unix)]
use std::fs;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{ListenerOptions, Name, prelude::*};
#[cfg(target_os = "linux")]
use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
use serde::{Deserialize, Serialize};

use crate::AuroError;

/// Maximum bytes read from one forwarding connection.
const MAX_MESSAGE_BYTES: u64 = 64 * 1024;

/// The message a secondary instance sends to the primary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceMessage {
    /// Arguments after the executable name.
    pub args: Vec<String>,
    /// The secondary process's working directory.
    pub cwd: PathBuf,
}

impl InstanceMessage {
    /// Creates a forwarding message.
    pub fn new(args: Vec<String>, cwd: impl Into<PathBuf>) -> Self {
        InstanceMessage {
            args,
            cwd: cwd.into(),
        }
    }
}

/// Holds the primary-instance lock and, when listening, the forwarding thread.
#[derive(Debug)]
pub struct AppInstance {
    file: File,
    path: PathBuf,
    name: String,
    primary: bool,
}

impl AppInstance {
    /// Tries to become the primary instance for `name`.
    ///
    /// `name` is normally the app identifier from `Auro.toml`. It is sanitised into a lock-file
    /// and local-socket name; the lock lives under `XDG_RUNTIME_DIR` on Linux when available,
    /// otherwise the per-user temporary directory. A secondary instance gets
    /// `is_primary() == false` rather than an error.
    pub fn acquire(name: &str) -> Result<Self, AuroError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AuroError::instance("", "empty application identifier"));
        }

        let path = lock_path(name)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path)?;

        let primary = match file.try_lock() {
            Ok(()) => true,
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Error(error)) => {
                return Err(AuroError::instance(
                    name,
                    format!("cannot lock `{}`: {error}", path.display()),
                ));
            }
        };

        Ok(AppInstance {
            file,
            path,
            name: name.to_owned(),
            primary,
        })
    }

    /// Whether this process owns the lock.
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// The application identifier this guard was acquired for.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The lock file. It is intentionally left on disk after the process exits.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Forwards arguments and the working directory to the primary instance.
    ///
    /// Only a secondary instance may forward. The primary must have called [`Self::listen`]
    /// first; if it has not, the OS returns a connection error.
    pub fn forward(&self, message: &InstanceMessage) -> Result<(), AuroError> {
        if self.primary {
            return Err(AuroError::instance(
                &self.name,
                "the primary instance cannot forward to itself",
            ));
        }

        let name = socket_name(&self.name)?;
        let mut stream = LocalSocketStream::connect(name).map_err(|error| {
            AuroError::instance(
                &self.name,
                format!("cannot connect to the primary instance: {error}"),
            )
        })?;
        serde_json::to_writer(&mut stream, message)?;
        stream.write_all(b"\n")?;
        Ok(())
    }

    /// Starts the primary's forwarding listener and hands each message to `handler`.
    ///
    /// The returned [`InstanceListener`] owns the lock and the listener thread. Drop it when the
    /// app exits; the thread is detached and the process exit releases the socket.
    pub fn listen(
        self,
        handler: impl Fn(InstanceMessage) + Send + 'static,
    ) -> Result<InstanceListener, AuroError> {
        if !self.primary {
            return Err(AuroError::instance(
                &self.name,
                "a secondary instance cannot listen",
            ));
        }

        let name = socket_name(&self.name)?;
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut options = ListenerOptions::new().name(name).try_overwrite(true);
        #[cfg(target_os = "linux")]
        {
            // An abstract socket cannot be permission-checked. The filesystem socket lives in a
            // private runtime directory; mode 0600 also protects the `/tmp` fallback.
            options = options.mode(0o600);
        }
        let listener = options.create_sync().map_err(|error| {
            AuroError::instance(
                &self.name,
                format!("cannot listen for a second instance: {error}"),
            )
        })?;
        let thread_name = self.name.clone();
        let thread = thread::Builder::new()
            .name(format!("auro-instance-{thread_name}"))
            .spawn(move || {
                for connection in listener.incoming() {
                    let stream = match connection {
                        Ok(stream) => stream,
                        Err(error) => {
                            tracing::warn!(%error, "instance listener accept failed");
                            continue;
                        }
                    };
                    let mut reader = BufReader::new(stream).take(MAX_MESSAGE_BYTES);
                    let mut line = String::new();
                    if let Err(error) = reader.read_line(&mut line) {
                        tracing::warn!(%error, "instance listener read failed");
                        continue;
                    }
                    match serde_json::from_str::<InstanceMessage>(line.trim()) {
                        Ok(message) => handler(message),
                        Err(error) => {
                            tracing::warn!(%error, "instance listener received invalid JSON");
                        }
                    }
                }
            })
            .map_err(|error| {
                AuroError::instance(
                    &self.name,
                    format!("cannot start the instance listener: {error}"),
                )
            })?;

        Ok(InstanceListener {
            _guard: self,
            _thread: thread,
        })
    }
}

/// Keeps the primary lock and forwarding thread alive for the app's lifetime.
pub struct InstanceListener {
    _guard: AppInstance,
    _thread: JoinHandle<()>,
}

impl Drop for AppInstance {
    fn drop(&mut self) {
        // Closing the file releases the OS lock on every supported platform. Unlock explicitly
        // first so the secondary handle never sees a short window where the old lock still exists.
        let _ = self.file.unlock();
    }
}

#[cfg(unix)]
fn socket_name(name: &str) -> Result<Name<'static>, AuroError> {
    let path = lock_dir(name)?.join(format!("{}.sock", socket_label(name)));
    path.as_os_str()
        .to_fs_name::<GenericFilePath>()
        .map(Name::into_owned)
        .map_err(|error| {
            AuroError::instance(
                name,
                format!("cannot create the instance socket name: {error}"),
            )
        })
}

#[cfg(windows)]
fn socket_name(name: &str) -> Result<Name<'static>, AuroError> {
    let socket = socket_label(name);
    socket
        .as_str()
        .to_ns_name::<GenericNamespaced>()
        .map(Name::into_owned)
        .map_err(|error| {
            AuroError::instance(
                name,
                format!("cannot create the instance socket name: {error}"),
            )
        })
}

/// A short, filesystem-safe socket name that keeps long identifiers distinct.
///
/// The identifier is truncated to fit `sun_path`, so a stable hash of the full identifier is
/// appended; two apps whose sanitised prefixes match cannot share a socket and steal each
/// other's forwarded launches.
fn socket_label(name: &str) -> String {
    format!(
        "auro-{}-{:016x}",
        sanitized_name(name, 40),
        stable_hash(name)
    )
}

/// FNV-1a over the full identifier. This is a name-collision guard, not a security primitive.
fn stable_hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn sanitized_name(name: &str, limit: usize) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .take(limit)
        .collect();
    if sanitized.is_empty() {
        "app".to_owned()
    } else {
        sanitized
    }
}

fn lock_path(name: &str) -> Result<PathBuf, AuroError> {
    Ok(lock_dir(name)?.join(format!("auro-{}.lock", sanitized_name(name, 128))))
}

/// The directory that holds the lock and socket.
///
/// `XDG_RUNTIME_DIR` is used only when it is a real directory owned by this user and not
/// group/other-writable. Otherwise the process uses a private per-user directory under the
/// temporary directory. On a shared `/tmp`, the predictable name alone is not enough: another
/// local user can create a directory or socket before the app starts, so ownership and mode are
/// checked and a private subdirectory is created with mode `0700`.
fn lock_dir(name: &str) -> Result<PathBuf, AuroError> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if is_private_existing_dir(&runtime) {
            return Ok(runtime);
        }
        tracing::warn!(
            path = %runtime.display(),
            "ignoring XDG_RUNTIME_DIR because it is not a private directory for this user"
        );
    }
    private_temp_dir(name)
}

#[cfg(unix)]
fn private_temp_dir(name: &str) -> Result<PathBuf, AuroError> {
    let dir = std::env::temp_dir().join(format!("auro-{}", effective_uid()));
    ensure_private_dir(&dir).map_err(|message| AuroError::instance(name, message))?;
    Ok(dir)
}

/// The effective user id.
///
/// `geteuid` takes no arguments, has no preconditions and cannot fail.
#[cfg(unix)]
fn effective_uid() -> libc::uid_t {
    // SAFETY: see the doc comment; the call reads the kernel's effective uid for this process.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt as _;

    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "cannot create the private runtime directory `{}`: {error}",
                dir.display()
            ));
        }
    }
    if !is_private_existing_dir(dir) {
        return Err(format!(
            "`{}` is not a private directory owned by this user; refusing to place the instance lock there",
            dir.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_temp_dir(_name: &str) -> Result<PathBuf, AuroError> {
    Ok(std::env::temp_dir())
}

#[cfg(unix)]
fn is_private_existing_dir(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.is_dir()
        && metadata.uid() == effective_uid()
        && metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn is_private_existing_dir(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique_name() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        format!("com.example.auro-instance-{}-{stamp}", std::process::id())
    }

    #[test]
    fn first_guard_is_primary_and_second_is_not() {
        let name = unique_name();
        let first = AppInstance::acquire(&name).expect("first");
        assert!(first.is_primary());
        assert_eq!(first.name(), name);
        assert!(first.path().is_file());

        let second = AppInstance::acquire(&name).expect("second");
        assert!(!second.is_primary());

        drop(first);
        let third = AppInstance::acquire(&name).expect("third");
        assert!(third.is_primary());
    }

    #[test]
    fn forwards_a_message_to_the_primary_listener() {
        let name = unique_name();
        let (tx, rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        let secondary = AppInstance::acquire(&name).expect("secondary");
        assert!(!secondary.is_primary());
        let message = InstanceMessage::new(vec!["--open".to_owned()], "/tmp");
        secondary.forward(&message).expect("forward");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
    }

    #[test]
    fn primary_cannot_forward_or_secondary_cannot_listen() {
        let name = unique_name();
        let primary = AppInstance::acquire(&name).expect("primary");
        assert!(
            primary
                .forward(&InstanceMessage::new(Vec::new(), "/tmp"))
                .is_err()
        );

        let secondary = AppInstance::acquire(&name).expect("secondary");
        assert!(secondary.listen(|_| {}).is_err());
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(matches!(
            AppInstance::acquire("  "),
            Err(AuroError::Instance { name, .. }) if name.is_empty()
        ));
    }

    #[test]
    fn path_is_sanitised() {
        let path = lock_path("../com.example/app").expect("lock dir");
        let file = path.file_name().and_then(|name| name.to_str()).unwrap();
        assert_eq!(file, "auro-..-com.example-app.lock");
    }

    #[test]
    fn long_socket_labels_stay_distinct() {
        let shared = "com.example.".to_owned() + &"a".repeat(80);
        let other = "com.example.".to_owned() + &"b".repeat(80);
        assert_ne!(socket_label(&shared), socket_label(&other));
        assert!(socket_label(&shared).len() <= 64);
    }

    #[cfg(unix)]
    #[test]
    fn private_runtime_directory_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("runtime");
        ensure_private_dir(&dir).expect("create private dir");
        assert_eq!(
            fs::metadata(&dir).expect("metadata").permissions().mode() & 0o077,
            0
        );

        // A second call reuses the directory and is still valid.
        ensure_private_dir(&dir).expect("reuse private dir");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_group_or_world_writable_runtime_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("shared");
        fs::create_dir(&dir).expect("create");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("mode");
        assert!(!is_private_existing_dir(&dir));
        assert!(ensure_private_dir(&dir).is_err());

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).expect("mode");
        assert!(!is_private_existing_dir(&dir));
        assert!(ensure_private_dir(&dir).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_runtime_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        fs::create_dir(&real).expect("create");
        let link = temp.path().join("link");
        symlink(&real, &link).expect("symlink");
        assert!(!is_private_existing_dir(&link));
        assert!(ensure_private_dir(&link).is_err());
    }
}
