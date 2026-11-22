// SPDX-License-Identifier: MIT OR Apache-2.0

//! Out-of-process plugin host with fail-closed negotiation and effect-site checks.
//!
//! The host spawns one child process per loaded plugin, passes a private endpoint through an
//! environment variable and dispatches only typed [`PluginCommand`] values. No subject, grant or
//! command name from the wire selects a handler: the loaded instance supplies its own
//! [`VerifiedSubject`] to [`EffectGateway`].
//!
//! Stage 2 intentionally has no timeouts, bounded queues, backpressure, crash handling or child
//! reaping. A `Child` handle is stored so stage 3 can add supervision; this stage never waits on
//! it. A plugin that never replies can therefore stall a call. The host is crash isolation, not a
//! sandbox: plugin native code runs as the same OS user and can bypass host-mediated effects.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use auro_perms::Policy;
use interprocess::local_socket::Stream;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::channel::{self, PluginChannel};
use crate::effects::{EffectError, EffectGateway};
use crate::manifest::{ManifestError, PluginManifest};
use crate::protocol::{
    self, Hello, HelloAck, NegotiationError, PROTOCOL_VERSION, PluginCommand, PluginReply,
};
use crate::registry::{PluginLoadError, PluginRegistry, VerifiedSubject};

/// Environment variable through which the host passes the private endpoint to the child.
pub const ENDPOINT_ENV: &str = "AURO_PLUGIN_ENDPOINT";

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

/// One loaded plugin instance and the host-side resources it owns.
#[derive(Debug)]
pub struct LoadedPlugin {
    subject: VerifiedSubject,
    manifest: PluginManifest,
    channel: PluginChannel,
    child: Child,
    stream: Option<Stream>,
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
    /// Stage 2 stores it but never waits on or reaps it; supervision arrives in stage 3.
    pub fn child(&self) -> &Child {
        &self.child
    }

    fn ensure_stream(&mut self) -> Result<&mut Stream, HostError> {
        let stream = match self.stream.take() {
            Some(stream) => stream,
            None => self.channel.accept()?,
        };
        Ok(self.stream.insert(stream))
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

    /// The child process could not be spawned.
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
}

impl PluginHost {
    /// Creates a host over `registry`, with file effects rooted at `root` and a typed command
    /// allowlist.
    ///
    /// `registry` carries the signed index and app allowlist; it is the only producer of subjects.
    /// The plugin package for id `X` is read from `<root>/X/manifest.json` and its entrypoint.
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
        }
    }

    /// Overrides the private channel directory, for tests that need an isolated directory.
    pub fn set_private_dir(&mut self, dir: impl Into<PathBuf>) {
        self.private_dir = dir.into();
    }

    /// Adds a host-controlled argument for spawned children.
    ///
    /// This is a fixture hook for driving the SDK's current-test-binary child mode; plugin
    /// manifests never choose process arguments. Stage 3 replaces it with a fixed launcher.
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
        let channel = PluginChannel::new(&self.private_dir, id, &secret)?;
        let mut process = Command::new(&entrypoint);
        process
            .current_dir(self.root.join(id))
            .args(&self.child_args)
            .envs(&self.child_env)
            .env(ENDPOINT_ENV, channel.endpoint_name());
        let child = process.spawn().map_err(HostError::Spawn)?;
        self.plugins.insert(
            id.to_owned(),
            LoadedPlugin {
                subject,
                manifest: manifest.clone(),
                channel,
                child,
                stream: None,
            },
        );
        Ok(())
    }

    /// Reads and validates the plugin's `Hello`, then sends the exact `HelloAck`.
    ///
    /// Every mismatch is fatal and no command can be dispatched before a successful handshake.
    pub fn handshake(&mut self, id: &str) -> Result<(), HostError> {
        let plugin = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| HostError::NotLoaded(id.to_owned()))?;
        let grants = plugin.manifest.capabilities.clone();
        let stream = plugin.ensure_stream()?;
        let hello: Hello = protocol::read_frame(stream).map_err(NegotiationError::from)?;
        protocol::validate_hello(&hello, &grants)?;
        let ack = HelloAck {
            protocol_version: PROTOCOL_VERSION,
            capabilities_hash: protocol::capabilities_hash(&grants),
        };
        protocol::write_frame(stream, &ack).map_err(HostError::ProtocolIo)
    }

    /// Applies the typed effect, sends `command` and returns the matching typed reply.
    ///
    /// The command allowlist is checked first and the effect runs before any frame is sent, so a
    /// denied effect never reaches the child. No string from the wire selects the effect.
    pub fn call(
        &mut self,
        id: &str,
        request_id: u64,
        command: PluginCommand,
    ) -> Result<PluginReply, HostError> {
        if !self.commands.allows(&command) {
            return Err(HostError::CommandNotAllowed(command_name(&command)));
        }
        let plugin = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| HostError::NotLoaded(id.to_owned()))?;
        if plugin.stream.is_none() {
            return Err(HostError::HandshakeRequired);
        }
        match &command {
            PluginCommand::ReadFile { path } => {
                self.effects.read_file(&plugin.subject, path)?;
            }
            PluginCommand::WriteFile { path, bytes } => {
                self.effects.write_file(&plugin.subject, path, bytes)?;
            }
            PluginCommand::Ping => {}
        }
        let stream = plugin.ensure_stream()?;
        let request = protocol::Request {
            request_id,
            command,
        };
        protocol::write_frame(stream, &request).map_err(HostError::ProtocolIo)?;
        let reply: PluginReply = protocol::read_frame(stream).map_err(HostError::ProtocolIo)?;
        let found = reply_request_id(&reply);
        if found != request_id {
            return Err(HostError::ReplyMismatch {
                expected: request_id,
                found,
            });
        }
        Ok(reply)
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
