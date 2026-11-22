// SPDX-License-Identifier: MIT OR Apache-2.0

//! Out-of-process plugin host with fail-closed negotiation, effect-site checks and supervision.
//!
//! The host spawns one child process per loaded plugin, passes a private endpoint through an
//! environment variable and dispatches only typed [`PluginCommand`] values. No subject, grant or
//! command name from the wire selects a handler: the loaded instance supplies its own
//! [`VerifiedSubject`] to [`EffectGateway`].
//!
//! Each instance owns one I/O thread. The thread accepts the single connection, performs the exact
//! handshake and serializes calls on the stream. Calls are submitted through a bounded queue:
//! [`PluginHost::submit`] applies backpressure with [`HostError::QueueFull`], and
//! [`PluginHost::wait`] bounds the reply wait with a per-call timeout. A silent child therefore
//! produces [`HostError::Timeout`] instead of hanging a caller. On EOF, child exit, abort or
//! timeout the instance is marked failed, its child is reaped, its grants are removed from the
//! host's effect policy and a typed [`PluginAvailability`] event is queued.
//!
//! The host is crash isolation, not a sandbox: plugin native code runs as the same OS user and can
//! bypass host-mediated effects. No same-user hostile plugin is in scope.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use auro_perms::Policy;
use interprocess::local_socket::Stream;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::channel::{self, PluginChannel};
use crate::effects::{EffectError, EffectGateway};
use crate::manifest::{ManifestError, ManifestGrant, PluginManifest};
use crate::protocol::{
    self, Hello, HelloAck, NegotiationError, PROTOCOL_VERSION, PluginCommand, PluginReply, Request,
};
use crate::registry::{PluginLoadError, PluginRegistry, VerifiedSubject};

/// Environment variable through which the host passes the private endpoint to the child.
pub const ENDPOINT_ENV: &str = "AURO_PLUGIN_ENDPOINT";

/// Default maximum time a single handshake or call waits for a plugin reply.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of outstanding calls queued for one plugin instance.
pub const REQUEST_QUEUE_CAPACITY: usize = 64;

/// How long a failed instance's I/O thread is given to exit before it is detached.
///
/// An I/O thread blocked in `accept` cannot be cancelled; the child is killed before this wait, so
/// a connected child's thread always finishes. A child that never connects leaves one detached
/// thread which exits when its endpoint is released.
const WORKER_JOIN_GRACE: Duration = Duration::from_millis(200);

/// Typed allowlist of commands an app exposes to plugins.
///
/// Deny by default: every command must be enabled explicitly. Dispatch matches Rust variants, so
/// a command name or payload supplied by a plugin cannot select a handler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandPolicy {
    read_file: bool,
    write_file: bool,
    ping: bool,
}

impl CommandPolicy {
    /// A policy that exposes no commands to plugins.
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Exposes [`PluginCommand::ReadFile`].
    #[must_use]
    pub fn allow_read_file(mut self) -> Self {
        self.read_file = true;
        self
    }

    /// Exposes [`PluginCommand::WriteFile`].
    #[must_use]
    pub fn allow_write_file(mut self) -> Self {
        self.write_file = true;
        self
    }

    /// Exposes [`PluginCommand::Ping`].
    #[must_use]
    pub fn allow_ping(mut self) -> Self {
        self.ping = true;
        self
    }

    /// Whether `command` is exposed by this policy.
    pub fn allows(&self, command: &PluginCommand) -> bool {
        match command {
            PluginCommand::ReadFile { .. } => self.read_file,
            PluginCommand::WriteFile { .. } => self.write_file,
            PluginCommand::Ping => self.ping,
        }
    }
}

/// Lifecycle of a loaded plugin instance's negotiation.
///
/// A failed negotiation is terminal for that instance: no command may be dispatched after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandshakeState {
    NotConnected,
    Handshaking,
    Ready,
    Failed,
}

/// Public lifecycle reported through [`PluginAvailability`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginState {
    /// The child was spawned but no handshake has completed.
    Starting,
    /// The exact handshake completed; typed calls may be dispatched.
    Ready,
    /// The instance is terminal. Its child was reaped and its grants were released.
    Failed,
}

/// Why an instance became unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginFailure {
    /// The plugin did not answer within the configured call timeout.
    Timeout,
    /// More calls were submitted than the bounded request queue can hold.
    QueueFull,
    /// The child died without an exit status (abort, signal, EOF or forced termination).
    ChildDied,
    /// The child exited with `code`; `None` means it was killed by a signal.
    Exited { code: Option<i32> },
    /// A protocol or handshake error closed the channel.
    Protocol,
}

/// A typed change in one plugin instance's availability.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PluginAvailability {
    /// Id of the instance whose state changed.
    pub id: String,
    /// The new state.
    pub state: PluginState,
    /// The terminal cause, present only when `state` is [`PluginState::Failed`].
    pub cause: Option<PluginFailure>,
}

/// One loaded plugin instance and the host-side resources it owns.
#[derive(Debug)]
pub struct LoadedPlugin {
    subject: VerifiedSubject,
    manifest: PluginManifest,
    channel: Arc<PluginChannel>,
    child: Child,
    commands: Option<SyncSender<WorkerCommand>>,
    worker: Option<JoinHandle<()>>,
    state: HandshakeState,
}

impl LoadedPlugin {
    /// The verified subject injected into every effect check for this instance.
    pub fn subject(&self) -> &VerifiedSubject {
        &self.subject
    }

    /// The verified manifest whose grants drive negotiation.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// The private channel bound before the child was spawned.
    pub fn channel(&self) -> &PluginChannel {
        &self.channel
    }

    /// The child process handle.
    ///
    /// The host reaps it on failure and on drop; [`PluginHost::child_status`] polls it without
    /// taking a mutable borrow of the handle.
    pub fn child(&self) -> &Child {
        &self.child
    }

    /// The instance's current lifecycle state.
    pub fn state(&self) -> PluginState {
        match self.state {
            HandshakeState::NotConnected | HandshakeState::Handshaking => PluginState::Starting,
            HandshakeState::Ready => PluginState::Ready,
            HandshakeState::Failed => PluginState::Failed,
        }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.try_send(WorkerCommand::Stop);
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        join_worker(&mut self.worker);
    }
}

/// Every host-side failure before, during or after a plugin call.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HostError {
    /// The plugin id was not a validated reverse-DNS-ish identifier.
    #[error("invalid plugin id `{id}`: {reason}")]
    InvalidPluginId {
        /// Rejected id.
        id: String,
        /// Validation detail.
        reason: String,
    },

    /// The endpoint name was short or did not embed a fresh secret.
    #[error("plugin endpoint name is predictable")]
    PredictableName,

    /// A channel name was occupied or the bind was denied.
    #[error("plugin channel is in use or access was denied: {0}")]
    ChannelConflict(#[source] io::Error),

    /// A channel operation failed for another reason.
    #[error("plugin channel I/O failed: {0}")]
    ChannelIo(#[source] io::Error),

    /// The private directory could not be created, opened or secured.
    #[error("private plugin directory operation failed: {0}")]
    PrivateDirectory(#[source] io::Error),

    /// The private directory is a symlink, is not owned by the user or is group/other-writable.
    #[error("insecure private plugin directory `{0}`")]
    InsecureDirectory(String),

    /// The secret token is a symlink, has loose permissions or does not match the endpoint.
    #[error("insecure plugin token file `{0}`")]
    InsecureToken(String),

    /// The endpoint path could not be represented as Unicode.
    #[error("plugin endpoint name is not valid Unicode")]
    InvalidChannelName,

    /// The plugin manifest could not be read.
    #[error("plugin manifest could not be read: {0}")]
    ManifestIo(#[source] io::Error),

    /// The plugin manifest failed validation.
    #[error("invalid plugin manifest: {0}")]
    Manifest(#[from] ManifestError),

    /// The manifest entrypoint was not a safe relative path.
    #[error("invalid plugin entrypoint `{0}`")]
    InvalidEntrypoint(String),

    /// The pinned artifact could not be read.
    #[error("plugin artifact could not be read: {0}")]
    ArtifactIo(#[source] io::Error),

    /// The same plugin id is already loaded in this host.
    #[error("plugin `{0}` is already loaded")]
    AlreadyLoaded(String),

    /// No instance with this id has been loaded.
    #[error("plugin `{0}` is not loaded")]
    NotLoaded(String),

    /// Identity verification failed before any process was spawned.
    #[error("plugin load failed: {0}")]
    Load(#[from] PluginLoadError),

    /// The child process or its I/O thread could not be spawned.
    #[error("plugin process could not be spawned: {0}")]
    Spawn(#[source] io::Error),

    /// The handshake failed; no command was dispatched.
    #[error("plugin handshake failed: {0}")]
    Handshake(#[from] NegotiationError),

    /// A framed message could not be written or read after a valid handshake.
    #[error("plugin protocol I/O failed: {0}")]
    ProtocolIo(#[source] io::Error),

    /// A typed command was outside the app's allowlist.
    #[error("command `{0}` is not exposed to plugins")]
    CommandNotAllowed(&'static str),

    /// A command was sent before the handshake completed.
    #[error("plugin handshake is required before commands")]
    HandshakeRequired,

    /// A file effect failed before any frame was sent.
    #[error("plugin file effect failed: {0}")]
    Effect(#[from] EffectError),

    /// The plugin answered a different request.
    #[error("plugin replied to request {expected} with {found}")]
    ReplyMismatch {
        /// Request that was sent.
        expected: u64,
        /// Request id carried by the reply.
        found: u64,
    },

    /// The plugin did not reply within the configured timeout.
    #[error("plugin `{id}` timed out after {duration:?}")]
    Timeout {
        /// Id of the timed-out instance.
        id: String,
        /// The timeout that elapsed.
        duration: Duration,
    },

    /// The bounded request queue for this instance was full.
    #[error("plugin `{id}` request queue is full")]
    QueueFull {
        /// Id of the instance whose queue was full.
        id: String,
    },

    /// The plugin process exited on its own.
    #[error("plugin `{id}` exited with status {status}")]
    PluginExited {
        /// Id of the exited instance.
        id: String,
        /// Reaped exit status.
        status: ExitStatus,
    },

    /// The plugin process died without a normal exit status, or its channel reached EOF.
    #[error("plugin `{id}` process died before replying")]
    ChildDied {
        /// Id of the dead instance.
        id: String,
    },
}

/// A submitted call whose typed reply has not been collected yet.
///
/// Submitting applies the effect checks and enqueues the request; the reply is collected with
/// [`PluginHost::wait`], which applies the per-call timeout and supervises the instance on failure.
#[derive(Debug)]
#[must_use = "a submitted call must be collected with PluginHost::wait"]
pub struct PendingCall {
    id: String,
    _request_id: u64,
    timeout: Duration,
    receiver: Receiver<Result<PluginReply, HostError>>,
}

impl PendingCall {
    /// The request id carried by this call's wire envelope.
    pub fn request_id(&self) -> u64 {
        self._request_id
    }
}

/// Command sent from the host thread to one instance's I/O thread.
#[derive(Debug)]
enum WorkerCommand {
    Handshake {
        grants: Vec<ManifestGrant>,
        reply: SyncSender<Result<(), HostError>>,
    },
    Call {
        request: Request,
        reply: SyncSender<Result<PluginReply, HostError>>,
    },
    Stop,
}

/// Spawns verified plugins and dispatches typed commands to them.
pub struct PluginHost {
    registry: PluginRegistry,
    effects: EffectGateway,
    root: PathBuf,
    commands: CommandPolicy,
    private_dir: PathBuf,
    child_args: Vec<OsString>,
    child_env: HashMap<OsString, OsString>,
    plugins: HashMap<String, LoadedPlugin>,
    call_timeout: Duration,
    events: Arc<Mutex<VecDeque<PluginAvailability>>>,
}

impl PluginHost {
    /// Creates a host over `registry`, with file effects rooted at `root` and a typed command
    /// allowlist.
    ///
    /// `registry` carries the signed index and app allowlist; it is the only producer of subjects.
    /// The plugin package for id `X` is read from `<root>/X/manifest.json` and its entrypoint.
    /// Calls use [`DEFAULT_CALL_TIMEOUT`] until [`Self::set_call_timeout`] changes it.
    pub fn new(
        registry: PluginRegistry,
        policy: Policy,
        root: impl Into<PathBuf>,
        commands: CommandPolicy,
    ) -> Self {
        let root = root.into();
        let private_dir = channel::default_private_dir();
        Self {
            registry,
            effects: EffectGateway::new(policy, root.clone()),
            root,
            commands,
            private_dir,
            child_args: Vec::new(),
            child_env: HashMap::new(),
            plugins: HashMap::new(),
            call_timeout: DEFAULT_CALL_TIMEOUT,
            events: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Overrides the reply timeout for handshakes and calls.
    ///
    /// A zero duration still performs one poll of the reply channel before timing out.
    pub fn set_call_timeout(&mut self, timeout: Duration) {
        self.call_timeout = timeout;
    }

    /// Builder form of [`Self::set_call_timeout`].
    #[must_use]
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.set_call_timeout(timeout);
        self
    }

    /// The current per-call timeout.
    pub fn call_timeout(&self) -> Duration {
        self.call_timeout
    }

    /// Overrides the private channel directory, for tests that need an isolated directory.
    pub fn set_private_dir(&mut self, dir: impl Into<PathBuf>) {
        self.private_dir = dir.into();
    }

    /// Adds a host-controlled argument for spawned children.
    ///
    /// This is a fixture hook for driving the SDK's current-test-binary child mode; plugin
    /// manifests never choose process arguments. A production launcher replaces this hook.
    pub fn add_child_arg(&mut self, arg: impl Into<OsString>) {
        self.child_args.push(arg.into());
    }

    /// Adds a host-controlled environment variable for spawned children.
    ///
    /// It cannot override [`ENDPOINT_ENV`], which is set last. Plugin manifests never choose
    /// process environment variables.
    pub fn set_child_env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.child_env.insert(key.into(), value.into());
    }

    /// The effect gateway used for every file command.
    pub fn effects(&self) -> &EffectGateway {
        &self.effects
    }

    /// The root under which plugin packages are read.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The loaded instance with this id, if any.
    pub fn loaded(&self, id: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(id)
    }

    /// Reaped status of a child, if it has exited.
    ///
    /// This is a non-blocking `try_wait`; the host already reaps failed children, so a failed
    /// instance still reports its cached status here.
    pub fn child_status(&mut self, id: &str) -> io::Result<Option<ExitStatus>> {
        match self.plugins.get_mut(id) {
            Some(plugin) => plugin.child.try_wait(),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "plugin is not loaded",
            )),
        }
    }

    /// Drains queued availability events after polling children for unobserved exits.
    pub fn take_events(&mut self) -> Vec<PluginAvailability> {
        self.poll_children();
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.drain(..).collect()
    }

    /// Loads `<root>/<id>/manifest.json`, verifies the pinned artifact and spawns the child.
    ///
    /// The manifest is parsed with its strict limits and the entrypoint is hashed before
    /// [`PluginRegistry::resolve`] admits the instance. Nothing is fetched and no shell is used.
    pub fn load(&mut self, id: &str, version: &str, instance_id: &str) -> Result<(), HostError> {
        let package = self.root.join(id);
        let manifest_path = package.join("manifest.json");
        let bytes = std::fs::read(&manifest_path).map_err(HostError::ManifestIo)?;
        let manifest = PluginManifest::parse(&bytes)?;
        self.load_with_manifest(id, version, instance_id, &manifest)
    }

    /// Verifies and spawns a caller-supplied, already validated manifest.
    ///
    /// This is the explicit variant of [`Self::load`] for hosts that compile their manifest in or
    /// parse it themselves. The same identity and artifact checks apply.
    pub fn load_with_manifest(
        &mut self,
        id: &str,
        version: &str,
        instance_id: &str,
        manifest: &PluginManifest,
    ) -> Result<(), HostError> {
        if self.plugins.contains_key(id) {
            return Err(HostError::AlreadyLoaded(id.to_owned()));
        }
        crate::validate::plugin_id(id).map_err(|reason| HostError::InvalidPluginId {
            id: id.to_owned(),
            reason: reason.to_owned(),
        })?;
        crate::validate::entrypoint(&manifest.entrypoint)
            .map_err(|_| HostError::InvalidEntrypoint(manifest.entrypoint.clone()))?;
        let entrypoint = self.root.join(id).join(&manifest.entrypoint);
        let artifact = std::fs::read(&entrypoint).map_err(HostError::ArtifactIo)?;
        let artifact_hash = crate::hex::encode(Sha256::digest(artifact));
        // resolve compares this hash with the signed index pin and the manifest, then derives
        // the only subject that can reach an effect site.
        let subject = self
            .registry
            .resolve(id, version, &artifact_hash, manifest, instance_id)?;

        let secret = channel::generate_secret();
        let channel = Arc::new(PluginChannel::new(&self.private_dir, id, &secret)?);
        let mut process = Command::new(&entrypoint);
        process
            .current_dir(self.root.join(id))
            .args(&self.child_args)
            .envs(&self.child_env)
            .env(ENDPOINT_ENV, channel.endpoint_name());
        let mut child = process.spawn().map_err(HostError::Spawn)?;
        let (commands, receiver) = mpsc::sync_channel(REQUEST_QUEUE_CAPACITY);
        let worker_id = id.to_owned();
        let worker_channel = Arc::clone(&channel);
        let worker = thread::Builder::new()
            .name(format!("auro-plugin-{worker_id}"))
            .spawn(move || run_worker(worker_id, worker_channel, receiver));
        let worker = match worker {
            Ok(handle) => handle,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostError::Spawn(error));
            }
        };
        self.plugins.insert(
            id.to_owned(),
            LoadedPlugin {
                subject,
                manifest: manifest.clone(),
                channel,
                child,
                commands: Some(commands),
                worker: Some(worker),
                state: HandshakeState::NotConnected,
            },
        );
        self.push_event(PluginAvailability {
            id: id.to_owned(),
            state: PluginState::Starting,
            cause: None,
        });
        Ok(())
    }

    /// Reads and validates the plugin's `Hello`, then sends the exact `HelloAck`.
    ///
    /// Every mismatch is fatal and no command can be dispatched before a successful handshake.
    /// The wait is bounded by the configured timeout.
    pub fn handshake(&mut self, id: &str) -> Result<(), HostError> {
        let (commands, grants) = {
            let plugin = self
                .plugins
                .get_mut(id)
                .ok_or_else(|| HostError::NotLoaded(id.to_owned()))?;
            match plugin.state {
                HandshakeState::Ready => return Ok(()),
                HandshakeState::Failed => return Err(HostError::HandshakeRequired),
                HandshakeState::NotConnected | HandshakeState::Handshaking => {}
            }
            plugin.state = HandshakeState::Handshaking;
            let commands = plugin
                .commands
                .as_ref()
                .ok_or_else(|| HostError::ChildDied { id: id.to_owned() })?
                .clone();
            (commands, plugin.manifest.capabilities.clone())
        };
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        if let Err(error) = commands.try_send(WorkerCommand::Handshake {
            grants,
            reply: reply_sender,
        }) {
            let cause = match error {
                TrySendError::Full(_) => HostError::QueueFull { id: id.to_owned() },
                TrySendError::Disconnected(_) => HostError::ChildDied { id: id.to_owned() },
            };
            return Err(self.fail_instance(id, cause));
        }
        match reply_receiver.recv_timeout(self.call_timeout) {
            Ok(Ok(())) => {
                if let Some(plugin) = self.plugins.get_mut(id) {
                    plugin.state = HandshakeState::Ready;
                }
                self.push_event(PluginAvailability {
                    id: id.to_owned(),
                    state: PluginState::Ready,
                    cause: None,
                });
                Ok(())
            }
            Ok(Err(error)) => Err(self.fail_instance(id, error)),
            Err(RecvTimeoutError::Timeout) => Err(self.fail_instance(
                id,
                HostError::Timeout {
                    id: id.to_owned(),
                    duration: self.call_timeout,
                },
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(self.fail_instance(id, HostError::ChildDied { id: id.to_owned() }))
            }
        }
    }

    /// Applies the typed effect, then queues `command` without waiting for its reply.
    ///
    /// The allowlist and effect checks run before the request is queued, so a denied effect never
    /// reaches the child. A full bounded queue is reported immediately as
    /// [`HostError::QueueFull`]; the caller is never blocked on a busy queue.
    pub fn submit(
        &mut self,
        id: &str,
        request_id: u64,
        command: PluginCommand,
    ) -> Result<PendingCall, HostError> {
        if !self.commands.allows(&command) {
            return Err(HostError::CommandNotAllowed(command_name(&command)));
        }
        let (commands, subject) = {
            let plugin = self
                .plugins
                .get(id)
                .ok_or_else(|| HostError::NotLoaded(id.to_owned()))?;
            if plugin.state != HandshakeState::Ready {
                return Err(HostError::HandshakeRequired);
            }
            let commands = plugin
                .commands
                .as_ref()
                .ok_or_else(|| HostError::ChildDied { id: id.to_owned() })?
                .clone();
            (commands, plugin.subject.clone())
        };
        match &command {
            PluginCommand::ReadFile { path } => {
                let _ = self.effects.read_file(&subject, path)?;
            }
            PluginCommand::WriteFile { path, bytes } => {
                self.effects.write_file(&subject, path, bytes)?;
            }
            PluginCommand::Ping => {}
        }
        let (reply_sender, receiver) = mpsc::sync_channel(1);
        let request = Request {
            request_id,
            command,
        };
        match commands.try_send(WorkerCommand::Call {
            request,
            reply: reply_sender,
        }) {
            Ok(()) => Ok(PendingCall {
                id: id.to_owned(),
                _request_id: request_id,
                timeout: self.call_timeout,
                receiver,
            }),
            Err(TrySendError::Full(_)) => Err(HostError::QueueFull { id: id.to_owned() }),
            Err(TrySendError::Disconnected(_)) => {
                Err(self.fail_instance(id, HostError::ChildDied { id: id.to_owned() }))
            }
        }
    }

    /// Collects a submitted call's typed reply within the per-call timeout.
    ///
    /// A timeout, EOF, child death or protocol error fails the instance: the child is reaped, the
    /// subject's grants are released and a [`PluginState::Failed`] event is queued.
    pub fn wait(&mut self, pending: PendingCall) -> Result<PluginReply, HostError> {
        let PendingCall {
            id,
            _request_id,
            timeout,
            receiver,
        } = pending;
        match receiver.recv_timeout(timeout) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error)) => Err(self.fail_instance(&id, error)),
            Err(RecvTimeoutError::Timeout) => Err(self.fail_instance(
                &id,
                HostError::Timeout {
                    id: id.clone(),
                    duration: timeout,
                },
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(self.fail_instance(&id, HostError::ChildDied { id: id.clone() }))
            }
        }
    }

    /// Applies the typed effect, queues the call and waits for its reply.
    pub fn call(
        &mut self,
        id: &str,
        request_id: u64,
        command: PluginCommand,
    ) -> Result<PluginReply, HostError> {
        let pending = self.submit(id, request_id, command)?;
        self.wait(pending)
    }

    /// Polls live children for exits that no call observed and fails those instances.
    fn poll_children(&mut self) {
        let exited: Vec<(String, ExitStatus)> = self
            .plugins
            .iter_mut()
            .filter_map(|(id, plugin)| {
                if plugin.state == HandshakeState::Failed {
                    return None;
                }
                match plugin.child.try_wait() {
                    Ok(Some(status)) => Some((id.clone(), status)),
                    _ => None,
                }
            })
            .collect();
        for (id, status) in exited {
            let _ = self.fail_instance(
                &id,
                HostError::PluginExited {
                    id: id.clone(),
                    status,
                },
            );
        }
    }

    /// Fails one instance terminally and normalizes the reported error after reaping.
    fn fail_instance(&mut self, id: &str, error: HostError) -> HostError {
        let (subject, status) = {
            let plugin = match self.plugins.get_mut(id) {
                Some(plugin) => plugin,
                None => return error,
            };
            plugin.state = HandshakeState::Failed;
            if let Some(commands) = plugin.commands.take() {
                let _ = commands.try_send(WorkerCommand::Stop);
            }
            let status = match plugin.child.try_wait() {
                Ok(Some(status)) => Some(status),
                Ok(None) => {
                    let _ = plugin.child.kill();
                    plugin.child.wait().ok()
                }
                Err(_) => None,
            };
            join_worker(&mut plugin.worker);
            (plugin.subject.clone(), status)
        };
        self.release_grants(&subject);
        self.push_event(PluginAvailability {
            id: id.to_owned(),
            state: PluginState::Failed,
            cause: Some(cause_of(&error, status)),
        });
        let replacement = match &error {
            HostError::ChildDied { id } => status.map(|status| HostError::PluginExited {
                id: id.clone(),
                status,
            }),
            _ => None,
        };
        replacement.unwrap_or(error)
    }

    /// Rebuilds the effect policy without any grant for `subject`.
    fn release_grants(&mut self, subject: &VerifiedSubject) {
        let mut policy = Policy::deny_all();
        let mut changed = false;
        for grant in self.effects.policy().grants() {
            if grant.subject() == subject.as_str() {
                changed = true;
            } else {
                policy = policy.allow(grant.clone());
            }
        }
        if changed {
            self.effects = EffectGateway::new(policy, self.root.clone());
        }
    }

    /// Queues an availability event for the host's accessor.
    fn push_event(&self, event: PluginAvailability) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.push_back(event);
    }
}

/// Maps a terminal host error to the cause carried by an availability event.
fn cause_of(error: &HostError, status: Option<ExitStatus>) -> PluginFailure {
    match error {
        HostError::Timeout { .. } => PluginFailure::Timeout,
        HostError::QueueFull { .. } => PluginFailure::QueueFull,
        HostError::PluginExited { status, .. } => PluginFailure::Exited {
            code: status.code(),
        },
        HostError::ChildDied { .. } => match status {
            Some(status) => PluginFailure::Exited {
                code: status.code(),
            },
            None => PluginFailure::ChildDied,
        },
        _ => PluginFailure::Protocol,
    }
}

/// Bounded join for a failed instance's I/O thread.
///
/// A connected child's thread unblocks as soon as the child is killed; an `accept`-blocked thread
/// is detached rather than risking an unbounded wait.
fn join_worker(worker: &mut Option<JoinHandle<()>>) {
    if let Some(handle) = worker.take() {
        let deadline = Instant::now() + WORKER_JOIN_GRACE;
        while Instant::now() < deadline && !handle.is_finished() {
            thread::sleep(Duration::from_millis(2));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

/// Owns one instance's stream and serializes its handshake and calls.
fn run_worker(id: String, channel: Arc<PluginChannel>, commands: Receiver<WorkerCommand>) {
    let mut stream: Option<Stream> = None;
    while let Ok(command) = commands.recv() {
        match command {
            WorkerCommand::Handshake { grants, reply } => {
                let result = (|| -> Result<(), HostError> {
                    let active = match stream.take() {
                        Some(accepted) => accepted,
                        None => channel.accept()?,
                    };
                    let active = stream.insert(active);
                    let hello: Hello =
                        protocol::read_frame(active).map_err(NegotiationError::from)?;
                    protocol::validate_hello(&hello, &grants)?;
                    let ack = HelloAck {
                        protocol_version: PROTOCOL_VERSION,
                        capabilities_hash: protocol::capabilities_hash(&grants),
                    };
                    protocol::write_frame(active, &ack).map_err(HostError::ProtocolIo)
                })();
                let failed = result.is_err();
                let _ = reply.try_send(result);
                if failed {
                    break;
                }
            }
            WorkerCommand::Call { request, reply } => {
                let request_id = request.request_id;
                let result = (|| -> Result<PluginReply, HostError> {
                    let active = stream.as_mut().ok_or(HostError::HandshakeRequired)?;
                    protocol::write_frame(active, &request)
                        .map_err(|error| classify_io(error, &id))?;
                    let answer: PluginReply =
                        protocol::read_frame(active).map_err(|error| classify_io(error, &id))?;
                    let found = reply_request_id(&answer);
                    if found != request_id {
                        return Err(HostError::ReplyMismatch {
                            expected: request_id,
                            found,
                        });
                    }
                    Ok(answer)
                })();
                let failed = result.is_err();
                let _ = reply.try_send(result);
                if failed {
                    break;
                }
            }
            WorkerCommand::Stop => break,
        }
    }
}

/// Distinguishes a dead peer from other framed I/O failures.
fn classify_io(error: io::Error, id: &str) -> HostError {
    match error.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::NotConnected => HostError::ChildDied { id: id.to_owned() },
        _ => HostError::ProtocolIo(error),
    }
}

fn command_name(command: &PluginCommand) -> &'static str {
    match command {
        PluginCommand::ReadFile { .. } => "read_file",
        PluginCommand::WriteFile { .. } => "write_file",
        PluginCommand::Ping => "ping",
    }
}

fn reply_request_id(reply: &PluginReply) -> u64 {
    match reply {
        PluginReply::Ok { request_id }
        | PluginReply::FileContent { request_id, .. }
        | PluginReply::Error { request_id, .. } => *request_id,
    }
}
