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
use std::time::{Duration, Instant};

#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{ListenerOptions, Name, prelude::*};
#[cfg(target_os = "linux")]
use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
use serde::{Deserialize, Serialize};

use crate::AuroError;

/// Maximum JSON payload one forwarding connection may carry.
///
/// Launch arguments are bounded rather than silently truncated: a message over the limit is
/// rejected by [`AppInstance::forward`] before it is written, so the secondary can report the
/// failure instead of exiting as if the primary had received it.
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

/// Big-endian byte length of the frame that precedes each JSON payload.
const LENGTH_PREFIX_BYTES: usize = 8;

/// Acknowledges a framed message so the secondary does not retry with the legacy framing.
const FORWARD_ACK: u8 = 0x06;

/// How long a secondary waits for the primary's acknowledgement before falling back.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a parser waits for the ordered dispatcher to accept or reject its message.
const DECISION_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on reading one forwarding connection. A sender that dies mid-frame must not block
/// the listener forever; platforms without receive timeouts fall back to a connection thread.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the dispatcher waits for an earlier complete message before skipping it.
///
/// A sequence is assigned only after a complete message has been parsed, so this window only
/// covers thread scheduling between assignment and delivery. A stalled sender never holds one.
const ORDER_PATIENCE: Duration = Duration::from_secs(2);

/// Maximum number of connections parsed concurrently.
///
/// A local process can open connections faster than they time out; a fixed worker pool bounds
/// threads and memory without letting one client starve the accept loop.
const CONNECTION_WORKERS: usize = 8;

/// Connections waiting for a parser worker. Excess connections are accepted and dropped.
const CONNECTION_QUEUE: usize = 16;

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

        let payload = serde_json::to_vec(message)?;
        let length = payload.len() as u64;
        if length > MAX_MESSAGE_BYTES {
            return Err(AuroError::instance(
                &self.name,
                format!(
                    "launch message is {length} bytes; the primary accepts at most {MAX_MESSAGE_BYTES}"
                ),
            ));
        }

        // alpha.3 introduced framed messages without a version handshake. A framed write is
        // acknowledged by an alpha.4 listener; when no acknowledgement arrives, retry with the
        // newline-delimited format understood by alpha.2. An alpha.3 primary accepts the frame
        // and ignores the legacy retry, so the message is delivered once.
        if self.forward_framed(&payload).is_ok() {
            return Ok(());
        }
        self.forward_legacy(&payload)
    }

    fn connect_primary(&self) -> Result<LocalSocketStream, AuroError> {
        let name = socket_name(&self.name)?;
        LocalSocketStream::connect(name).map_err(|error| {
            AuroError::instance(
                &self.name,
                format!("cannot connect to the primary instance: {error}"),
            )
        })
    }

    fn forward_framed(&self, payload: &[u8]) -> Result<(), AuroError> {
        let mut stream = self.connect_primary()?;
        // Without a receive timeout the read below could wait forever on an older primary that
        // does not acknowledge. If the platform cannot set one, use the legacy framing directly.
        stream.set_recv_timeout(Some(ACK_TIMEOUT))?;
        stream.write_all(&(payload.len() as u64).to_be_bytes())?;
        stream.write_all(payload)?;
        let mut acknowledgement = [0u8; 1];
        stream.read_exact(&mut acknowledgement)?;
        if acknowledgement[0] != FORWARD_ACK {
            return Err(AuroError::instance(
                &self.name,
                "the primary acknowledged with an unknown protocol byte",
            ));
        }
        Ok(())
    }

    fn forward_legacy(&self, payload: &[u8]) -> Result<(), AuroError> {
        let mut stream = self.connect_primary()?;
        stream.write_all(payload)?;
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
        #[cfg(windows)]
        {
            use interprocess::os::windows::local_socket::ListenerOptionsExt as _;
            use interprocess::os::windows::security_descriptor::SecurityDescriptor;
            use widestring::U16CString;

            // A named pipe is machine-global. Do not inherit the token's default DACL: allow
            // only the creating user (OW), SYSTEM and Administrators to connect (#89).
            let sddl = U16CString::from_str("D:P(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;BA)").map_err(
                |error| {
                    AuroError::instance(
                        &self.name,
                        format!("cannot build the instance pipe ACL: {error}"),
                    )
                },
            )?;
            let descriptor = SecurityDescriptor::deserialize(sddl.as_ucstr()).map_err(|error| {
                AuroError::instance(
                    &self.name,
                    format!("cannot build the instance pipe ACL: {error}"),
                )
            })?;
            options = options.security_descriptor(descriptor);
        }
        let listener = options.create_sync().map_err(|error| {
            AuroError::instance(
                &self.name,
                format!("cannot listen for a second instance: {error}"),
            )
        })?;
        let thread_name = self.name.clone();
        let (outcomes, received) = std::sync::mpsc::channel::<(u64, ConnectionOutcome)>();
        // Parsers run on a fixed worker pool: a local process cannot turn every accepted
        // connection into an OS thread. A slow handler stays isolated from this pool.
        let sequence = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (connections, queued) =
            std::sync::mpsc::sync_channel::<LocalSocketStream>(CONNECTION_QUEUE);
        let queued = std::sync::Arc::new(std::sync::Mutex::new(queued));
        let mut connection_workers = Vec::with_capacity(CONNECTION_WORKERS);
        for _ in 0..CONNECTION_WORKERS {
            let outcomes = outcomes.clone();
            let sequence = std::sync::Arc::clone(&sequence);
            let queued = std::sync::Arc::clone(&queued);
            let worker = thread::Builder::new()
                .name("auro-instance-message".to_owned())
                .spawn(move || {
                    loop {
                        let stream = {
                            let receiver = queued.lock().expect("instance connection queue");
                            match receiver.recv() {
                                Ok(stream) => stream,
                                Err(_) => return,
                            }
                        };
                        handle_connection(
                            std::sync::Arc::clone(&sequence),
                            stream,
                            outcomes.clone(),
                        );
                    }
                })
                .map_err(|error| {
                    AuroError::instance(
                        &self.name,
                        format!("cannot start an instance connection worker: {error}"),
                    )
                })?;
            connection_workers.push(worker);
        }
        // A slow app handler must not stall the listener, and a frame that misses the ordering
        // window must not be acknowledged and then dropped. The ordered dispatcher owns the
        // accept/reject decision; only an accepted frame is acknowledged.
        let (messages, received_messages) = std::sync::mpsc::channel::<InstanceMessage>();
        let worker = thread::Builder::new()
            .name(format!("auro-instance-handler-{thread_name}"))
            .spawn(move || {
                while let Ok(message) = received_messages.recv() {
                    let dispatched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handler(message);
                    }));
                    if dispatched.is_err() {
                        tracing::error!("instance listener handler panicked");
                    }
                }
            })
            .map_err(|error| {
                AuroError::instance(
                    &self.name,
                    format!("cannot start the instance handler: {error}"),
                )
            })?;
        let dispatcher = thread::Builder::new()
            .name(format!("auro-instance-order-{thread_name}"))
            .spawn(move || {
                dispatch_in_order(received, move |message| {
                    if messages.send(message).is_err() {
                        tracing::warn!("instance listener handler is gone");
                    }
                });
            })
            .map_err(|error| {
                AuroError::instance(
                    &self.name,
                    format!("cannot start the instance dispatcher: {error}"),
                )
            })?;
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
                    // The queue is bounded. When every parser is busy, close the excess
                    // connection instead of creating an unbounded thread or stalling accepts.
                    match connections.try_send(stream) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            tracing::warn!(
                                "instance listener at connection capacity; dropping a client"
                            );
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
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
            _dispatcher: dispatcher,
            _worker: worker,
            _connection_workers: connection_workers,
        })
    }
}

struct ConnectionOutcome {
    message: Option<InstanceMessage>,
    framed: bool,
    decision: std::sync::mpsc::Sender<bool>,
}

fn handle_connection(
    sequence_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stream: LocalSocketStream,
    outcomes: std::sync::mpsc::Sender<(u64, ConnectionOutcome)>,
) {
    // Windows named pipes reject receive timeouts; isolation in this thread still keeps the
    // accept loop responsive if a sender stalls.
    let _ = stream.set_recv_timeout(Some(CONNECTION_TIMEOUT));
    let mut reader = BufReader::new(stream);
    let framed = match reader.fill_buf() {
        Ok([]) => return,
        Ok(bytes) => bytes
            .first()
            .is_some_and(|byte| *byte != b'{' && !byte.is_ascii_whitespace()),
        Err(error) => {
            tracing::warn!(%error, "instance listener read failed");
            return;
        }
    };
    // An incomplete frame has no launch to order. A client can send a length header and stop;
    // assigning a sequence before the payload arrives would let it reserve a slot for
    // CONNECTION_TIMEOUT and delay a later launch by ORDER_PATIENCE (#81).
    let message = if framed {
        read_framed(&mut reader)
    } else {
        read_legacy(&mut reader)
    };
    let Some(message) = message else {
        return;
    };
    let sequence = sequence_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (decision, decided) = std::sync::mpsc::channel();
    let acknowledge = framed;
    let outcome = ConnectionOutcome {
        message: Some(message),
        framed,
        decision,
    };
    if outcomes.send((sequence, outcome)).is_err() {
        return;
    }
    if acknowledge && matches!(decided.recv_timeout(DECISION_TIMEOUT), Ok(true)) {
        let _ = reader.get_mut().write_all(&[FORWARD_ACK]);
    }
}

fn read_framed(reader: &mut BufReader<LocalSocketStream>) -> Option<InstanceMessage> {
    let mut header = [0u8; LENGTH_PREFIX_BYTES];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(error) => {
            tracing::warn!(%error, "instance listener read failed");
            return None;
        }
    }
    let length = u64::from_be_bytes(header);
    if length > MAX_MESSAGE_BYTES {
        tracing::warn!(
            length,
            limit = MAX_MESSAGE_BYTES,
            "instance listener rejected an oversized message"
        );
        return None;
    }
    let mut payload = vec![0u8; length as usize];
    if let Err(error) = reader.read_exact(&mut payload) {
        tracing::warn!(%error, "instance listener read failed");
        return None;
    }
    match serde_json::from_slice::<InstanceMessage>(&payload) {
        Ok(message) => Some(message),
        Err(error) => {
            tracing::warn!(%error, "instance listener received invalid JSON");
            None
        }
    }
}

fn read_legacy(reader: &mut BufReader<LocalSocketStream>) -> Option<InstanceMessage> {
    // alpha.2 and earlier wrote one JSON line, with no length prefix.
    let mut line = String::new();
    match reader
        .by_ref()
        .take(MAX_MESSAGE_BYTES + 1)
        .read_line(&mut line)
    {
        Ok(0) => return None,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(error) => {
            tracing::warn!(%error, "instance listener read failed");
            return None;
        }
    }
    if line.len() as u64 > MAX_MESSAGE_BYTES {
        tracing::warn!(
            length = line.len(),
            limit = MAX_MESSAGE_BYTES,
            "instance listener rejected an oversized message"
        );
        return None;
    }
    match serde_json::from_str::<InstanceMessage>(line.trim()) {
        Ok(message) => Some(message),
        Err(error) => {
            tracing::warn!(%error, "instance listener received invalid JSON");
            None
        }
    }
}

fn dispatch_in_order(
    received: std::sync::mpsc::Receiver<(u64, ConnectionOutcome)>,
    dispatch: impl Fn(InstanceMessage),
) {
    let mut pending: std::collections::BTreeMap<u64, ConnectionOutcome> =
        std::collections::BTreeMap::new();
    let mut next = 0u64;
    let mut disconnected = false;
    let mut skip_at: Option<Instant> = None;
    loop {
        while let Some(outcome) = pending.remove(&next) {
            match outcome.message {
                Some(message) if outcome.framed => {
                    // Acknowledge only after the message is accepted in order. If the parser gave
                    // up waiting, dropping the message lets the legacy retry be the single
                    // delivery.
                    if outcome.decision.send(true).is_ok() {
                        dispatch(message);
                    }
                }
                Some(message) => dispatch(message),
                None if outcome.framed => {
                    let _ = outcome.decision.send(false);
                }
                None => {}
            }
            next += 1;
            skip_at = None;
        }
        if disconnected {
            if pending.is_empty() {
                break;
            }
            next = *pending.keys().next().expect("pending is not empty");
            skip_at = None;
            continue;
        }

        if pending.is_empty() {
            skip_at = None;
        } else if skip_at.is_none() {
            skip_at = Some(Instant::now() + ORDER_PATIENCE);
        }
        if let Some(deadline) = skip_at
            && Instant::now() >= deadline
            && let Some(smallest) = pending.keys().next().copied()
        {
            tracing::warn!(
                expected = next,
                found = smallest,
                "instance listener skipped a missing connection"
            );
            next = smallest;
            skip_at = None;
            continue;
        }
        let wait = skip_at.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let outcome = match wait {
            Some(wait) => match received.recv_timeout(wait) {
                Ok(outcome) => outcome,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(smallest) = pending.keys().next().copied() {
                        tracing::warn!(
                            expected = next,
                            found = smallest,
                            "instance listener skipped a missing connection"
                        );
                        next = smallest;
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    continue;
                }
            },
            None => match received.recv() {
                Ok(outcome) => outcome,
                Err(_) => {
                    disconnected = true;
                    continue;
                }
            },
        };
        if outcome.0 >= next {
            pending.insert(outcome.0, outcome.1);
        } else {
            // The sequence window already passed: do not acknowledge, so the secondary can use
            // the legacy fallback. The message itself must not be delivered twice.
            if outcome.1.framed {
                let _ = outcome.1.decision.send(false);
            }
            tracing::warn!(
                sequence = outcome.0,
                next,
                "instance listener dropped a late connection"
            );
        }
    }
}

/// Keeps the primary lock and forwarding threads alive for the app's lifetime.
pub struct InstanceListener {
    _guard: AppInstance,
    _thread: JoinHandle<()>,
    _dispatcher: JoinHandle<()>,
    _worker: JoinHandle<()>,
    _connection_workers: Vec<JoinHandle<()>>,
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
    let path = socket_path(name)?;
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

#[cfg(unix)]
fn socket_path(name: &str) -> Result<PathBuf, AuroError> {
    Ok(lock_dir(name)?.join(socket_label(name)))
}

/// A short, filesystem-safe socket name that keeps long identifiers distinct.
///
/// The identifier is truncated to fit `sun_path`, so a stable hash of the full identifier is
/// appended; two apps whose sanitised prefixes match cannot share a socket and steal each
/// other's forwarded launches.
fn socket_label(name: &str) -> String {
    format!(
        "auro-{}-{:016x}",
        sanitized_name(name, 8),
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
    // Truncation alone makes two long identifiers share a lock; append the same stable hash the
    // socket name uses so the lock is unique for the full identifier.
    Ok(lock_dir(name)?.join(format!(
        "auro-{}-{:016x}.lock",
        sanitized_name(name, 128),
        stable_hash(name)
    )))
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
        assert_eq!(
            file,
            format!(
                "auro-..-com.example-app-{:016x}.lock",
                stable_hash("../com.example/app")
            )
        );
    }

    #[test]
    fn long_identifiers_with_the_same_prefix_do_not_share_a_lock() {
        let common = format!("com.example.{}", "a".repeat(140));
        let first = AppInstance::acquire(&format!("{common}.one")).expect("first");
        assert!(first.is_primary());
        let second = AppInstance::acquire(&format!("{common}.two")).expect("second");
        assert!(
            second.is_primary(),
            "a different identifier must not share the lock file"
        );
        assert_ne!(first.path(), second.path());
    }

    #[test]
    fn a_large_but_bounded_message_is_delivered() {
        let name = unique_name();
        let (tx, rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["a".repeat(70 * 1024)], "/tmp");
        secondary.forward(&message).expect("forward");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
    }

    #[test]
    fn an_oversized_message_is_rejected_before_it_is_written() {
        let name = unique_name();
        let (tx, _rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["a".repeat(1024 * 1024)], "/tmp");
        assert!(matches!(
            secondary.forward(&message),
            Err(AuroError::Instance { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_fits_sun_path() {
        use std::os::unix::ffi::OsStrExt as _;

        let name = "com.example.".to_owned() + &"a".repeat(80);
        let path = socket_path(&name).expect("socket path");
        let limit =
            std::mem::size_of::<libc::sockaddr_un>() - std::mem::size_of::<libc::sa_family_t>();
        assert!(
            path.as_os_str().as_bytes().len() < limit,
            "{} exceeds the {limit}-byte sun_path",
            path.display()
        );
    }

    #[test]
    fn long_socket_labels_stay_distinct() {
        let shared = "com.example.".to_owned() + &"a".repeat(80);
        let other = "com.example.".to_owned() + &"b".repeat(80);
        assert_ne!(socket_label(&shared), socket_label(&other));
        assert!(socket_label(&shared).len() <= 40);
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

    #[test]
    fn a_slow_handler_does_not_make_the_next_launch_duplicate() {
        use std::sync::{Arc, Mutex};

        let name = unique_name();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler_seen = Arc::clone(&seen);
        let (started_tx, started_rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = started_tx.send(());
                std::thread::sleep(Duration::from_millis(700));
                handler_seen.lock().expect("seen").push(message);
            })
            .expect("listener");

        let first = InstanceMessage::new(vec!["first".to_owned()], "/tmp");
        let one = AppInstance::acquire(&name).expect("first secondary");
        one.forward(&first).expect("first forward");
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the handler started");

        let second = InstanceMessage::new(vec!["second".to_owned()], "/tmp");
        let two = AppInstance::acquire(&name).expect("second secondary");
        two.forward(&second).expect("second forward");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while seen.lock().expect("seen").len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        // Give a would-be legacy duplicate time to arrive before counting.
        std::thread::sleep(Duration::from_millis(600));
        let seen = seen.lock().expect("seen");
        assert_eq!(
            seen.len(),
            2,
            "a slow handler must not deliver a launch twice: {seen:?}"
        );
        assert!(seen.contains(&first));
        assert!(seen.contains(&second));
    }

    #[test]
    fn dispatches_messages_in_connection_order() {
        let (outcomes, received) = mpsc::channel();
        let (seen_tx, seen_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            dispatch_in_order(received, move |message| {
                let _ = seen_tx.send(message);
            });
        });

        let first = InstanceMessage::new(vec!["first".to_owned()], "/tmp");
        let second = InstanceMessage::new(vec!["second".to_owned()], "/tmp");
        let (decision1, acknowledged1) = mpsc::channel();
        let (decision0, acknowledged0) = mpsc::channel();
        outcomes
            .send((
                1,
                ConnectionOutcome {
                    message: Some(second.clone()),
                    framed: true,
                    decision: decision1,
                },
            ))
            .expect("send second");
        outcomes
            .send((
                0,
                ConnectionOutcome {
                    message: Some(first.clone()),
                    framed: true,
                    decision: decision0,
                },
            ))
            .expect("send first");
        drop(outcomes);

        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(5)).expect("first"),
            first
        );
        assert_eq!(
            seen_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("second"),
            second
        );
        assert_eq!(acknowledged0.recv_timeout(Duration::from_secs(1)), Ok(true));
        assert_eq!(acknowledged1.recv_timeout(Duration::from_secs(1)), Ok(true));
        worker.join().expect("dispatcher");
    }

    #[test]
    fn a_frame_that_misses_the_order_window_is_not_acknowledged() {
        let (outcomes, received) = mpsc::channel();
        let (seen_tx, seen_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            dispatch_in_order(received, move |message| {
                let _ = seen_tx.send(message);
            });
        });

        let first = InstanceMessage::new(vec!["first".to_owned()], "/tmp");
        let second = InstanceMessage::new(vec!["second".to_owned()], "/tmp");
        let (decision1, acknowledged1) = mpsc::channel();
        outcomes
            .send((
                1,
                ConnectionOutcome {
                    message: Some(second.clone()),
                    framed: true,
                    decision: decision1,
                },
            ))
            .expect("send second");

        // Sequence 0 is missing; the dispatcher skips it after ORDER_PATIENCE and accepts 1.
        assert_eq!(
            seen_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("second"),
            second
        );
        assert_eq!(acknowledged1.recv_timeout(Duration::from_secs(1)), Ok(true));

        let (decision0, acknowledged0) = mpsc::channel();
        outcomes
            .send((
                0,
                ConnectionOutcome {
                    message: Some(first),
                    framed: true,
                    decision: decision0,
                },
            ))
            .expect("send late first");
        assert_eq!(
            acknowledged0.recv_timeout(Duration::from_secs(1)),
            Ok(false),
            "a frame outside the ordering window must not be acknowledged"
        );
        assert!(
            seen_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the late frame must not be delivered"
        );
        drop(outcomes);
        worker.join().expect("dispatcher");
    }

    #[test]
    fn an_idle_connection_does_not_delay_a_later_launch() {
        let name = unique_name();
        let (tx, rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        // Connect and send nothing: there is no launch to order, so it gets no sequence.
        let _idle = LocalSocketStream::connect(socket_name(&name).expect("name"))
            .expect("connect idle client");
        std::thread::sleep(Duration::from_millis(100));

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["later".to_owned()], "/tmp");
        let started = std::time::Instant::now();
        secondary.forward(&message).expect("forward");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "an idle connection delayed the launch by {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_incomplete_frame_does_not_delay_or_duplicate_a_later_launch() {
        let name = unique_name();
        let (tx, rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        // Promise a payload and never send it. The connection parser owns this read.
        let mut stalled = LocalSocketStream::connect(socket_name(&name).expect("name"))
            .expect("connect stalled client");
        stalled
            .write_all(&(64 * 1024u64).to_be_bytes())
            .expect("write header");
        std::thread::sleep(Duration::from_millis(100));

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["later".to_owned()], "/tmp");
        let started = std::time::Instant::now();
        secondary.forward(&message).expect("forward");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "an incomplete frame delayed the next launch by {:?}",
            started.elapsed()
        );
        drop(stalled);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the failed stalled frame must not duplicate the later launch"
        );
    }

    #[test]
    fn accepts_a_newline_framed_message_from_an_older_instance() {
        let name = unique_name();
        let (tx, rx) = mpsc::channel();
        let primary = AppInstance::acquire(&name).expect("primary");
        let _listener = primary
            .listen(move |message| {
                let _ = tx.send(message);
            })
            .expect("listener");

        let message = InstanceMessage::new(vec!["legacy".to_owned()], "/tmp");
        let mut client = LocalSocketStream::connect(socket_name(&name).expect("name"))
            .expect("connect to the alpha.4 listener");
        serde_json::to_writer(&mut client, &message).expect("write JSON");
        client.write_all(b"\n").expect("write newline");
        drop(client);

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
    }

    #[test]
    fn a_new_secondary_forwards_to_an_old_newline_primary() {
        let name = unique_name();
        let _primary = AppInstance::acquire(&name).expect("primary");
        let listener = ListenerOptions::new()
            .name(socket_name(&name).expect("name"))
            .try_overwrite(true)
            .create_sync()
            .expect("old listener");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(stream) = connection else {
                    continue;
                };
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                if let Ok(message) = serde_json::from_str::<InstanceMessage>(line.trim()) {
                    let _ = tx.send(message);
                    break;
                }
            }
        });

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["fallback".to_owned()], "/tmp");
        secondary.forward(&message).expect("forward");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
        handle.join().expect("listener thread");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_secondary_forwards_once_to_an_old_framed_primary() {
        let name = unique_name();
        let _primary = AppInstance::acquire(&name).expect("primary");
        let listener = ListenerOptions::new()
            .name(socket_name(&name).expect("name"))
            .try_overwrite(true)
            .create_sync()
            .expect("old listener");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            for (index, connection) in listener.incoming().enumerate() {
                let Ok(stream) = connection else {
                    continue;
                };
                let mut reader = BufReader::new(stream);
                if index == 0 {
                    let mut header = [0u8; LENGTH_PREFIX_BYTES];
                    if reader.read_exact(&mut header).is_ok() {
                        let length = u64::from_be_bytes(header);
                        if length <= MAX_MESSAGE_BYTES {
                            let mut payload = vec![0u8; length as usize];
                            if reader.read_exact(&mut payload).is_ok()
                                && let Ok(message) =
                                    serde_json::from_slice::<InstanceMessage>(&payload)
                            {
                                let _ = tx.send(message);
                            }
                        }
                    }
                } else {
                    // The legacy retry is not a frame; consume its header and stop.
                    let mut header = [0u8; LENGTH_PREFIX_BYTES];
                    let _ = reader.read_exact(&mut header);
                    break;
                }
            }
        });

        let secondary = AppInstance::acquire(&name).expect("secondary");
        let message = InstanceMessage::new(vec!["compat".to_owned()], "/tmp");
        secondary.forward(&message).expect("forward");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("message"),
            message
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the legacy retry must not duplicate the launch"
        );
        handle.join().expect("listener thread");
    }
}
