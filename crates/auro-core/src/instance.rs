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
    /// otherwise in a private per-user directory (`%LOCALAPPDATA%\auro\instance` on Windows).
    /// A secondary instance gets `is_primary() == false` rather than an error.
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
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // A pre-created symlink or reparse point must not redirect the lock elsewhere.
            options.custom_flags(windows_private::OPEN_REPARSE_POINT);
        }
        let file = options.open(&path)?;
        // Windows has no mode bits to inherit: the lock is only usable when its DACL keeps
        // other users out. The directory was validated by `lock_path`, and this check also
        // covers a lock file left behind by an older build.
        #[cfg(windows)]
        windows_private::validate_file(&file, &path)
            .map_err(|message| AuroError::instance(name, message))?;

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

        #[cfg(windows)]
        let pipe = pipe_label(&self.name)?;
        #[cfg(windows)]
        let name = namespaced_name(&pipe, &self.name)?;
        #[cfg(not(windows))]
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
            if is_permission_denied(&error) {
                #[cfg(windows)]
                {
                    return AuroError::instance(
                        &self.name,
                        format!(
                            "cannot listen for a second instance: another process or user already owns the instance pipe `{pipe}`: {error}"
                        ),
                    );
                }
                #[cfg(not(windows))]
                {
                    return AuroError::instance(
                        &self.name,
                        format!(
                            "cannot listen for a second instance: another process already owns the instance socket: {error}"
                        ),
                    );
                }
            }
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
    let socket = pipe_label(name)?;
    namespaced_name(&socket, name)
}

/// Wraps a named-pipe name for the local-socket API.
#[cfg(windows)]
fn namespaced_name(socket: &str, name: &str) -> Result<Name<'static>, AuroError> {
    socket
        .to_ns_name::<GenericNamespaced>()
        .map(Name::into_owned)
        .map_err(|error| {
            AuroError::instance(
                name,
                format!("cannot create the instance socket name: {error}"),
            )
        })
}

/// The Windows named-pipe name for an app.
///
/// A pipe name is machine-global, so the app identifier alone is predictable from the public
/// `Auro.toml` and another local user could pre-create it. The per-user token kept in the
/// private instance directory makes the name unguessable without reading that file (#94).
#[cfg(windows)]
fn pipe_label(name: &str) -> Result<String, AuroError> {
    let token = windows_private::instance_token(name)?;
    Ok(format!("{}-{token}", socket_label(name)))
}

/// Whether a socket creation error means another principal already owns the name.
fn is_permission_denied(error: &std::io::Error) -> bool {
    #[cfg(not(windows))]
    {
        error.kind() == std::io::ErrorKind::PermissionDenied
    }
    #[cfg(windows)]
    {
        error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(windows_private::ERROR_ACCESS_DENIED_CODE)
    }
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
///
/// On Windows `XDG_RUNTIME_DIR` is ignored because it is not a Windows convention. The
/// directory is `%LOCALAPPDATA%\auro\instance` (or `%TEMP%\auro-<sid>` when `LOCALAPPDATA` is
/// unset) and trust comes from its owner, DACL and the absence of reparse points (#96).
fn lock_dir(name: &str) -> Result<PathBuf, AuroError> {
    // `XDG_RUNTIME_DIR` is a Unix convention. On Windows it is not owned by the desktop
    // session, so honoring it would accept an arbitrary directory with an unchecked DACL.
    #[cfg(unix)]
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
    let dir = private_temp_dir(name)?;
    // Windows trusts a directory through its owner and DACL rather than a mode bit. Checking
    // the chosen directory here guarantees `acquire` fails before a secondary can forward
    // launch arguments into a lock an attacker controls (#96).
    #[cfg(windows)]
    if !is_private_existing_dir(&dir) {
        return Err(AuroError::instance(
            name,
            format!(
                "`{}` is not a private directory owned by this user; refusing to place the instance lock there",
                dir.display()
            ),
        ));
    }
    Ok(dir)
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

#[cfg(windows)]
fn private_temp_dir(name: &str) -> Result<PathBuf, AuroError> {
    windows_private::private_dir(name)
}

#[cfg(not(any(unix, windows)))]
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

#[cfg(windows)]
fn is_private_existing_dir(path: &Path) -> bool {
    windows_private::validate_private_dir(path).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn is_private_existing_dir(path: &Path) -> bool {
    path.is_dir()
}

/// Windows owner, DACL and reparse-point checks for the instance directory and its files.
///
/// Unix encodes trust in directory modes; Windows has none. The lock directory, the lock file
/// and the per-user token are accepted only when their owner is this user (or a system
/// principal) and no other principal may write, delete or re-ACL them (#96).
#[cfg(windows)]
mod windows_private {
    use std::ffi::c_void;
    use std::fs::{self, File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;

    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetLengthSid, GetTokenInformation, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
        WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinCreatorOwnerSid,
        WinLocalSystemSid,
    };
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DELETE_CHILD,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_ATTRIBUTES,
        FILE_WRITE_DATA, FILE_WRITE_EA, FileAttributeTagInfo, GetFileInformationByHandleEx,
        WRITE_DAC, WRITE_OWNER,
    };
    use windows::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_OBJECT_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use crate::AuroError;

    /// Name of the file that holds the per-user instance token.
    const TOKEN_FILE: &str = "instance.token";

    /// `FILE_FLAG_OPEN_REPARSE_POINT`, so an opened path is never followed through a link.
    pub(super) const OPEN_REPARSE_POINT: u32 = FILE_FLAG_OPEN_REPARSE_POINT.0;

    /// Raw `ERROR_ACCESS_DENIED`, used alongside [`std::io::ErrorKind::PermissionDenied`].
    pub(super) const ERROR_ACCESS_DENIED_CODE: i32 = ERROR_ACCESS_DENIED.0 as i32;

    /// Rights that let another principal modify, replace or re-ACL an entry.
    const WRITE_ACCESS: u32 = FILE_WRITE_DATA.0
        | FILE_APPEND_DATA.0
        | FILE_WRITE_EA.0
        | FILE_WRITE_ATTRIBUTES.0
        | FILE_ADD_FILE.0
        | FILE_ADD_SUBDIRECTORY.0
        | FILE_DELETE_CHILD.0
        | DELETE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | 0x4000_0000 // GENERIC_WRITE
        | 0x1000_0000; // GENERIC_ALL

    struct Principals {
        user: Vec<u8>,
        system: Vec<u8>,
        administrators: Vec<u8>,
        creator_owner: Vec<u8>,
        creator_owner_rights: Vec<u8>,
    }

    /// The per-user directory that holds the lock, socket name token and lock files.
    ///
    /// `%LOCALAPPDATA%\auro\instance` is the normal location. When `LOCALAPPDATA` is absent (a
    /// service or a stripped environment) the fallback is `%TEMP%\auro-<sid>`, which is still
    /// owner-checked and never shared between users because the SID is part of the name.
    pub(super) fn private_dir(name: &str) -> Result<PathBuf, AuroError> {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
            let root = PathBuf::from(local).join("auro");
            create_and_validate(&root, name)?;
            let dir = root.join("instance");
            create_and_validate(&dir, name)?;
            return Ok(dir);
        }

        let sid = user_sid_string().map_err(|message| AuroError::instance(name, message))?;
        let dir = std::env::temp_dir().join(format!("auro-{sid}"));
        create_and_validate(&dir, name)?;
        Ok(dir)
    }

    fn create_and_validate(dir: &Path, name: &str) -> Result<(), AuroError> {
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(AuroError::instance(
                    name,
                    format!(
                        "cannot create the private instance directory `{}`: {error}",
                        dir.display()
                    ),
                ));
            }
        }
        validate_private_dir(dir).map_err(|message| AuroError::instance(name, message))
    }

    /// Checks that `path` is a directory this user controls.
    pub(super) fn validate_private_dir(path: &Path) -> Result<(), String> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | OPEN_REPARSE_POINT);
        let directory = options.open(path).map_err(|error| {
            format!(
                "cannot open the instance directory `{}`: {error}",
                path.display()
            )
        })?;
        let metadata = directory.metadata().map_err(|error| {
            format!(
                "cannot inspect the instance directory `{}`: {error}",
                path.display()
            )
        })?;
        if !metadata.is_dir() {
            return Err(format!(
                "`{}` is not a directory; refusing to trust it",
                path.display()
            ));
        }
        validate_handle(HANDLE(directory.as_raw_handle()), path)
    }

    /// Checks an already opened lock or token file.
    ///
    /// The caller opens with [`OPEN_REPARSE_POINT`], so the handle always refers to the entry
    /// that will be used rather than to a link target.
    pub(super) fn validate_file(file: &File, path: &Path) -> Result<(), String> {
        validate_handle(HANDLE(file.as_raw_handle()), path)
    }

    fn validate_handle(handle: HANDLE, path: &Path) -> Result<(), String> {
        reject_reparse(handle, path)?;
        validate_owner_and_dacl(handle, path)
    }

    fn reject_reparse(handle: HANDLE, path: &Path) -> Result<(), String> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: the handle is live and the buffer matches `FileAttributeTagInfo`.
        unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                &mut info as *mut _ as *mut c_void,
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        }
        .map_err(|_| format!("cannot read the attributes of `{}`", path.display()))?;
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(format!(
                "`{}` is a reparse point; refusing to trust the instance directory",
                path.display()
            ));
        }
        Ok(())
    }

    fn validate_owner_and_dacl(handle: HANDLE, path: &Path) -> Result<(), String> {
        let principals = Principals {
            user: current_user_sid()?,
            system: well_known_sid(WinLocalSystemSid)?,
            administrators: well_known_sid(WinBuiltinAdministratorsSid)?,
            creator_owner: well_known_sid(WinCreatorOwnerSid)?,
            creator_owner_rights: well_known_sid(WinCreatorOwnerRightsSid)?,
        };

        let mut owner = PSID(null_mut());
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR(null_mut());
        // SAFETY: the out-pointers are valid and the handle is live.
        let result = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        if result != ERROR_SUCCESS {
            return Err(format!(
                "cannot read the security information of `{}`",
                path.display()
            ));
        }
        // SAFETY: the descriptor owns the memory until `LocalFree`; owner and dacl point into it.
        let result = (|| {
            if owner.0.is_null() {
                return Err(format!("`{}` has no owner", path.display()));
            }
            if !sid_is_allowed(owner, &principals) {
                return Err(format!(
                    "`{}` is not owned by this user; refusing to trust the instance directory",
                    path.display()
                ));
            }
            if dacl.is_null() {
                return Err(format!(
                    "`{}` has no DACL; refusing to trust the instance directory",
                    path.display()
                ));
            }
            validate_dacl(dacl, path, &principals)
        })();
        // SAFETY: `GetSecurityInfo` allocated the descriptor; the owner and DACL are borrowed.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }

    fn validate_dacl(dacl: *mut ACL, path: &Path, principals: &Principals) -> Result<(), String> {
        let mut info = ACL_SIZE_INFORMATION::default();
        // SAFETY: dacl is a valid ACL from `GetSecurityInfo` and info matches the class.
        unsafe {
            GetAclInformation(
                dacl,
                &mut info as *mut _ as *mut c_void,
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        }
        .map_err(|_| format!("cannot enumerate the DACL of `{}`", path.display()))?;

        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: index is within AceCount and ace receives a pointer into the ACL.
            unsafe { GetAce(dacl, index, &mut ace) }
                .map_err(|_| format!("cannot read a DACL entry of `{}`", path.display()))?;
            if ace.is_null() {
                return Err(format!("`{}` has a null DACL entry", path.display()));
            }
            // SAFETY: `GetAce` returned an ACE whose first field is `ACE_HEADER`.
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            // An inherit-only ACE applies to children, never to this object.
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE.0 != 0 {
                continue;
            }
            match u32::from(header.AceType) {
                ACCESS_ALLOWED_ACE_TYPE => {
                    // SAFETY: an allowed ACE has the `ACCESS_ALLOWED_ACE` layout.
                    let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                    if allowed.Mask & WRITE_ACCESS == 0 {
                        continue;
                    }
                    // SAFETY: `SidStart` is the first bytes of the variable-length SID.
                    let sid = PSID(&allowed.SidStart as *const u32 as *mut c_void);
                    if !sid_is_allowed(sid, principals) {
                        return Err(format!(
                            "the DACL of `{}` grants write access to another principal; refusing to trust the instance directory",
                            path.display()
                        ));
                    }
                }
                ACCESS_DENIED_ACE_TYPE => {}
                ACCESS_ALLOWED_OBJECT_ACE_TYPE => {
                    return Err(format!(
                        "the DACL of `{}` contains an unsupported object ACE",
                        path.display()
                    ));
                }
                _ => {
                    return Err(format!(
                        "the DACL of `{}` contains an unsupported ACE type",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    /// The per-user token that makes the named pipe unguessable (#94).
    ///
    /// The file is created once with `create_new` and inherited from the validated private
    /// directory. A malformed file is an error rather than a silent regeneration so a
    /// half-written file cannot silently change the pipe name under a running app.
    pub(super) fn instance_token(name: &str) -> Result<String, AuroError> {
        let dir = private_dir(name)?;
        let path = dir.join(TOKEN_FILE);
        // Three attempts cover the window in which a concurrent first start has created the
        // file but not yet written it.
        for attempt in 0..3 {
            let existing =
                read_token(&path).map_err(|message| AuroError::instance(name, message))?;
            if let Some(text) = existing {
                let token = text.trim();
                if is_valid_token(token) {
                    return Ok(token.to_owned());
                }
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                return Err(malformed_token(name, &path));
            }
            let created =
                create_token(&path).map_err(|message| AuroError::instance(name, message))?;
            if let Some(token) = created {
                return Ok(token);
            }
            // Another process created the file between the read and `create_new`; read it.
        }
        Err(AuroError::instance(
            name,
            format!(
                "cannot create or read the instance token `{}`",
                path.display()
            ),
        ))
    }

    fn malformed_token(name: &str, path: &Path) -> AuroError {
        AuroError::instance(
            name,
            format!(
                "the instance token `{}` is malformed; it must contain exactly 32 lowercase hexadecimal characters (delete the file to make Auro generate a new one)",
                path.display()
            ),
        )
    }

    fn read_token(path: &Path) -> Result<Option<String>, String> {
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(OPEN_REPARSE_POINT);
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "cannot open the instance token `{}`: {error}",
                    path.display()
                ));
            }
        };
        validate_file(&file, path)?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(|error| {
            format!(
                "cannot read the instance token `{}`: {error}",
                path.display()
            )
        })?;
        Ok(Some(text))
    }

    fn create_token(path: &Path) -> Result<Option<String>, String> {
        let token = generate_token();
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(OPEN_REPARSE_POINT);
        let mut file = match options.open(path) {
            Ok(file) => file,
            // `create_new` refuses an existing file, including one created concurrently.
            Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "cannot create the instance token `{}`: {error}",
                    path.display()
                ));
            }
        };
        validate_file(&file, path)?;
        file.write_all(token.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .map_err(|error| {
                format!(
                    "cannot write the instance token `{}`: {error}",
                    path.display()
                )
            })?;
        Ok(Some(token))
    }

    /// 128 bits of randomness without a crypto dependency.
    ///
    /// `RandomState` is seeded by the standard library from OS randomness and each instance is
    /// independent; the clock input keeps two instances called in the same nanosecond distinct.
    /// The token is a private name component, not a key.
    fn generate_token() -> String {
        use std::fmt::Write as _;
        use std::hash::{BuildHasher, Hasher};

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let mut token = String::with_capacity(32);
        for index in 0u8..2 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u8(index);
            hasher.write_u128(nanos);
            let _ = write!(token, "{:016x}", hasher.finish());
        }
        token
    }

    fn is_valid_token(token: &str) -> bool {
        token.len() == 32
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }

    fn current_user_sid() -> Result<Vec<u8>, String> {
        let mut token = HANDLE(null_mut());
        // SAFETY: GetCurrentProcess returns a pseudo-handle; token receives a real handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|_| "cannot open the process token".to_owned())?;
        let mut length = 0u32;
        // SAFETY: a null buffer with length zero asks for the required size.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
        if length == 0 {
            // SAFETY: token was opened by this function.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(token);
            }
            return Err("cannot size the process token user".to_owned());
        }
        let mut buffer = vec![0u8; length as usize];
        // SAFETY: buffer is at least `length` bytes and token is live.
        let result = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr() as *mut c_void),
                length,
                &mut length,
            )
        };
        // SAFETY: token was opened by this function.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(token);
        }
        result.map_err(|_| "cannot read the process token user".to_owned())?;
        // SAFETY: the buffer contains a TOKEN_USER followed by its SID.
        let token_user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let sid = token_user.User.Sid;
        // SAFETY: sid points into the live buffer; GetLengthSid returns its byte length.
        let length = unsafe { GetLengthSid(sid) } as usize;
        if length == 0 {
            return Err("the process token user has no SID".to_owned());
        }
        // SAFETY: sid is valid for `length` bytes.
        Ok(unsafe { std::slice::from_raw_parts(sid.0 as *const u8, length) }.to_vec())
    }

    /// The current user's SID as the canonical `S-1-5-...` string, for the temp fallback name.
    fn user_sid_string() -> Result<String, String> {
        let sid = current_user_sid()?;
        let mut text = windows::core::PWSTR::null();
        // SAFETY: sid is a valid SID; text receives a LocalAlloc string on success.
        unsafe { ConvertSidToStringSidW(PSID(sid.as_ptr() as *mut c_void), &mut text) }
            .map_err(|_| "cannot convert this user's SID to a string".to_owned())?;
        if text.is_null() {
            return Err("this user's SID string is empty".to_owned());
        }
        // SAFETY: ConvertSidToStringSidW produced a NUL-terminated UTF-16 string.
        let value = unsafe { text.to_string() }
            .map_err(|_| "this user's SID string is not valid UTF-16".to_owned());
        // SAFETY: ConvertSidToStringSidW allocated the string with LocalAlloc.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
        }
        value
    }

    fn well_known_sid(
        kind: windows::Win32::Security::WELL_KNOWN_SID_TYPE,
    ) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0u8; windows::Win32::Security::SECURITY_MAX_SID_SIZE as usize];
        let mut length = buffer.len() as u32;
        // SAFETY: buffer is SECURITY_MAX_SID_SIZE bytes and length matches.
        unsafe {
            CreateWellKnownSid(
                kind,
                None,
                Some(PSID(buffer.as_mut_ptr() as *mut c_void)),
                &mut length,
            )
        }
        .map_err(|_| "cannot create a well-known SID".to_owned())?;
        buffer.truncate(length as usize);
        Ok(buffer)
    }

    fn sid_is_allowed(sid: PSID, principals: &Principals) -> bool {
        sid_is_principal(sid, &principals.user)
            || sid_is_principal(sid, &principals.system)
            || sid_is_principal(sid, &principals.administrators)
            || sid_is_principal(sid, &principals.creator_owner)
            || sid_is_principal(sid, &principals.creator_owner_rights)
    }

    fn sid_is_principal(sid: PSID, expected: &[u8]) -> bool {
        if sid.0.is_null() || expected.is_empty() {
            return false;
        }
        // SAFETY: both SIDs are valid and expected is a byte slice holding a SID.
        unsafe { EqualSid(sid, PSID(expected.as_ptr() as *mut c_void)).is_ok() }
    }
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

    /// Serializes tests that mutate process-wide environment variables.
    #[cfg(any(unix, windows))]
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores an environment variable when dropped, including on panic.
    #[cfg(any(unix, windows))]
    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(any(unix, windows))]
    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: the caller holds `ENV_LOCK`, so no test reads or writes the environment
            // concurrently.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    #[cfg(any(unix, windows))]
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same lock invariant as `set`.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn lock_dir_refuses_a_group_writable_runtime_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let _serialized = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = temp.path().join("runtime");
        fs::create_dir(&runtime).expect("create");
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o775)).expect("mode");
        let _guard = EnvGuard::set("XDG_RUNTIME_DIR", runtime.as_os_str());

        let dir = lock_dir("com.example.auro-instance-env").expect("lock dir");
        assert_ne!(
            dir, runtime,
            "a group-writable XDG_RUNTIME_DIR must not hold the instance lock"
        );
        assert!(is_private_existing_dir(&dir));
    }

    #[cfg(windows)]
    #[test]
    fn windows_token_is_stable_across_calls_and_socket_names_match() {
        let name = unique_name();
        let token = windows_private::instance_token(&name).expect("token");
        assert_eq!(token.len(), 32);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "the token must be lowercase hexadecimal: {token}"
        );

        let label = pipe_label(&name).expect("pipe label");
        assert_eq!(label, format!("{}-{token}", socket_label(&name)));
        assert_eq!(label, pipe_label(&name).expect("pipe label"));
        assert_eq!(
            socket_name(&name).expect("first socket name"),
            socket_name(&name).expect("second socket name")
        );

        let dir = private_temp_dir(&name).expect("private directory");
        let text = std::fs::read_to_string(dir.join("instance.token")).expect("token file");
        assert_eq!(text.trim(), token);
        assert!(
            is_private_existing_dir(&dir),
            "the validated private directory must pass the trust check"
        );

        // A DACL that grants another user write cannot be built with the standard library
        // alone (it needs a foreign SID and an explicit ACL edit), so the write-ACE branch of
        // `validate_dacl` is only exercised by a Windows host with such a directory configured.
        assert!(!is_private_existing_dir(&dir.join("missing")));
        let file = dir.join("not-a-directory");
        std::fs::write(&file, b"x").expect("write file");
        assert!(!is_private_existing_dir(&file));
        std::fs::remove_file(&file).expect("remove file");
    }

    #[cfg(windows)]
    #[test]
    fn windows_lock_dir_ignores_xdg_runtime_dir() {
        let _serialized = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = temp.path().join("runtime");
        std::fs::create_dir(&runtime).expect("create");
        let _guard = EnvGuard::set("XDG_RUNTIME_DIR", runtime.as_os_str());

        let dir = lock_dir("com.example.auro-instance-xdg").expect("lock dir");
        assert_ne!(
            dir, runtime,
            "XDG_RUNTIME_DIR is a Unix convention and must not hold the lock on Windows"
        );
        assert!(is_private_existing_dir(&dir));
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
