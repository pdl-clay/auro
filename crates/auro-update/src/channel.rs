// SPDX-License-Identifier: MIT OR Apache-2.0

//! GitHub Releases update channel (spec 0037).
//!
//! Discovery and manifest transport live here. Trust does not: the pinned Ed25519 key and the
//! [`Updater`] verify every offer before an artifact is requested.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::runtime::http_agent;
use crate::{
    ApplyJournal, ApplyKind, ApplyPlan, Manifest, PublicKey, Relaunch, StagedUpdate,
    UpdateDecision, UpdateError, Updater,
};

/// The public API endpoint used to resolve the latest release.
const API_BASE: &str = "https://api.github.com";
/// The release asset name the channel looks for unless configured otherwise.
pub const DEFAULT_MANIFEST_ASSET: &str = "auro-update.json";
/// Maximum bytes read from a release response or a signed manifest asset.
const MANIFEST_LIMIT: usize = 1024 * 1024;
const LAST_CHECK_FILE: &str = "last-check";
const JOURNAL_FILE: &str = "update.json";
const GITHUB_HOSTS: [&str; 3] = [
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
];

/// Settings for one GitHub Releases channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelConfig {
    repository: String,
    public_key: String,
    manifest_asset: String,
    interval: Duration,
    check_on_start: bool,
}

impl ChannelConfig {
    /// Creates settings for a public `owner/repo` with a base64 Ed25519 public key.
    pub fn new(repository: &str, public_key_base64: &str) -> Self {
        ChannelConfig {
            repository: repository.to_owned(),
            public_key: public_key_base64.to_owned(),
            manifest_asset: DEFAULT_MANIFEST_ASSET.to_owned(),
            interval: Duration::from_secs(3600),
            check_on_start: true,
        }
    }

    /// Selects the signed manifest asset in the release.
    pub fn with_manifest_asset(mut self, name: &str) -> Self {
        self.manifest_asset = name.to_owned();
        self
    }

    /// Sets the minimum interval between automatic checks.
    pub fn with_interval_secs(mut self, seconds: u64) -> Self {
        self.interval = Duration::from_secs(seconds);
        self
    }

    /// Allows or suppresses a check before the first timestamp has been recorded.
    pub fn with_check_on_start(mut self, enabled: bool) -> Self {
        self.check_on_start = enabled;
        self
    }

    /// The configured `owner/repo`.
    pub fn repository(&self) -> &str {
        &self.repository
    }

    /// The configured manifest asset name.
    pub fn manifest_asset(&self) -> &str {
        &self.manifest_asset
    }

    /// The minimum interval between automatic checks.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Whether a check is allowed before the first timestamp has been recorded.
    pub fn check_on_start(&self) -> bool {
        self.check_on_start
    }
}

/// A GitHub Releases adapter over the existing signed updater.
pub struct GithubChannel {
    settings: ChannelConfig,
    state_dir: PathBuf,
    updater: Updater,
    agent: ureq::Agent,
    api_base: String,
}

impl GithubChannel {
    /// Creates a channel using `state_dir` for staging, the last-check timestamp and the journal.
    ///
    /// The directory must be private to the user: it is the parent of the apply journal. The public
    /// key is parsed here, so a malformed pin fails before any request.
    pub fn new(
        config: ChannelConfig,
        state_dir: impl Into<PathBuf>,
        current_version: &str,
    ) -> Result<Self, UpdateError> {
        if !valid_repository(&config.repository) {
            return Err(UpdateError::InvalidField {
                field: "updater.repository",
                reason: "expected owner/repo with GitHub-safe names",
            });
        }
        if !valid_asset_name(&config.manifest_asset) {
            return Err(UpdateError::InvalidField {
                field: "updater.manifest-asset",
                reason: "expected a plain asset name",
            });
        }
        if config.interval.is_zero() {
            return Err(UpdateError::InvalidField {
                field: "updater.interval-secs",
                reason: "must be positive",
            });
        }
        let public_key = PublicKey::from_base64(&config.public_key)?;
        let state_dir = state_dir.into();
        let updater = Updater::new(state_dir.join("staging"), public_key, current_version)?;
        fs::create_dir_all(&state_dir)?;
        Ok(GithubChannel {
            settings: config,
            state_dir,
            updater,
            agent: http_agent(),
            api_base: API_BASE.to_owned(),
        })
    }

    /// The validated settings this channel was built with.
    pub fn settings(&self) -> &ChannelConfig {
        &self.settings
    }

    /// Fetches the signed manifest from the latest release and verifies its signature and platform.
    ///
    /// This does not download the artifact. A signed older or up-to-date manifest is still returned
    /// so the caller can classify it with [`Self::check`].
    pub fn manifest(&self) -> Result<Manifest, UpdateError> {
        let release = self.latest_release()?;
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == self.settings.manifest_asset)
            .ok_or_else(|| UpdateError::ReleaseAssetMissing {
                release: release.tag_name.clone(),
                asset: self.settings.manifest_asset.clone(),
            })?;
        validate_asset_url(&asset.browser_download_url, self.api_host().as_deref())?;
        let body = self.fetch_limited(&asset.browser_download_url)?;
        let text = std::str::from_utf8(&body)
            .map_err(|_| UpdateError::ReleaseResponse("signed manifest is not UTF-8".to_owned()))?;
        let manifest = Manifest::from_json(text)?;
        self.updater.check(&manifest)?;
        Ok(manifest)
    }

    /// Classifies the signed manifest in the latest release.
    pub fn check(&self) -> Result<UpdateDecision, UpdateError> {
        let manifest = self.manifest()?;
        self.updater.check(&manifest)
    }

    /// Whether enough time has passed for an automatic check.
    pub fn due(&self) -> Result<bool, UpdateError> {
        self.due_at(SystemTime::now())
    }

    /// Records a successful check attempt in the state directory.
    pub fn record_check(&self) -> Result<(), UpdateError> {
        self.record_check_at(SystemTime::now())
    }

    /// Checks when due, recording the attempt even when it fails.
    pub fn check_if_due(&self) -> Result<Option<UpdateDecision>, UpdateError> {
        if !self.due()? {
            return Ok(None);
        }
        let result = self.check();
        self.record_check()?;
        result.map(Some)
    }

    /// Verifies the latest release and stages its artifact, returning `None` when not newer.
    ///
    /// Supplying `base` uses the signed delta when the manifest has one; otherwise the full
    /// artifact is downloaded. The base must be the currently installed artifact.
    pub fn stage(&self, base: Option<&Path>) -> Result<Option<StagedUpdate>, UpdateError> {
        let manifest = self.manifest()?;
        if self.updater.check(&manifest)? != UpdateDecision::Available {
            return Ok(None);
        }
        let staged = match (base, manifest.delta.as_ref()) {
            (Some(base), Some(_)) => self.updater.download_delta(&manifest, base)?,
            _ => self.updater.download(&manifest)?,
        };
        Ok(Some(staged))
    }

    /// Stages the latest release and writes an AppImage apply journal without starting the helper.
    pub fn prepare(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<Option<ApplyJournal>, UpdateError> {
        let Some(staged) = self.stage(base)? else {
            return Ok(None);
        };
        let kind = if cfg!(target_os = "linux") {
            ApplyKind::AppImage
        } else {
            return Err(UpdateError::UnsupportedApply {
                kind: ApplyKind::AppImage,
            });
        };
        let plan = ApplyPlan::new(
            kind,
            staged.path().to_path_buf(),
            Some(target.into()),
            staged.manifest().clone(),
            self.updater.public_key(),
        );
        let journal = ApplyJournal::create_for_apply(
            self.state_dir.join(JOURNAL_FILE),
            staged.manifest().version.clone(),
            plan,
            Relaunch::current()?,
        )?;
        Ok(Some(journal))
    }

    /// Prepares and starts the restart helper. Returns `false` when no update is available.
    ///
    /// The app must exit after `Ok(true)` so the helper can take the journal lock.
    pub fn apply(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<bool, UpdateError> {
        let Some(journal) = self.prepare(target, base)? else {
            return Ok(false);
        };
        journal.spawn_helper()?;
        Ok(true)
    }

    /// Prepares an apply when the check is due, recording the attempt either way.
    pub fn prepare_if_due(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<Option<ApplyJournal>, UpdateError> {
        if !self.due()? {
            return Ok(None);
        }
        let result = self.prepare(target, base);
        self.record_check()?;
        result
    }

    /// Starts an update when the check is due, recording the attempt either way.
    pub fn apply_if_due(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<bool, UpdateError> {
        if !self.due()? {
            return Ok(false);
        }
        let result = self.apply(target, base);
        self.record_check()?;
        result
    }

    fn latest_release(&self) -> Result<Release, UpdateError> {
        let url = format!(
            "{}/repos/{}/releases/latest",
            self.api_base.trim_end_matches('/'),
            self.settings.repository
        );
        let body = self.fetch_limited(&url)?;
        serde_json::from_slice(&body)
            .map_err(|error| UpdateError::ReleaseResponse(error.to_string()))
    }

    fn fetch_limited(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "auro-updater")
            .call()?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MANIFEST_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MANIFEST_LIMIT {
            return Err(UpdateError::ManifestTooLarge {
                limit: MANIFEST_LIMIT,
            });
        }
        Ok(bytes)
    }

    fn due_at(&self, now: SystemTime) -> Result<bool, UpdateError> {
        let last = self.last_check()?;
        if last.is_none() && !self.settings.check_on_start {
            return Ok(false);
        }
        match last {
            Some(last) => {
                Ok(now.duration_since(last).unwrap_or(Duration::ZERO) >= self.settings.interval)
            }
            None => Ok(true),
        }
    }

    fn last_check(&self) -> Result<Option<SystemTime>, UpdateError> {
        let path = self.state_dir.join(LAST_CHECK_FILE);
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // A corrupt timestamp is local state, not a trust decision: check now instead of failing.
        let Ok(seconds) = text.trim().parse::<u64>() else {
            return Ok(None);
        };
        Ok(UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
    }

    fn record_check_at(&self, now: SystemTime) -> Result<(), UpdateError> {
        let seconds = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let mut temporary = tempfile::NamedTempFile::new_in(&self.state_dir)?;
        write!(temporary, "{seconds}")?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(self.state_dir.join(LAST_CHECK_FILE))
            .map_err(|error| error.error)?;
        Ok(())
    }

    fn api_host(&self) -> Option<String> {
        let rest = self.api_base.split_once("://")?.1;
        let authority = rest.split(['/', '?', '#']).next()?;
        Some(
            authority
                .split(':')
                .next()
                .unwrap_or(authority)
                .to_ascii_lowercase(),
        )
    }

    #[cfg(test)]
    fn with_api_base(mut self, base: String) -> Self {
        self.api_base = base;
        self
    }
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn valid_repository(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, repository)| github_name(owner) && github_name(repository))
}

fn github_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn valid_asset_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn validate_asset_url(url: &str, test_host: Option<&str>) -> Result<(), UpdateError> {
    let reject = || UpdateError::UnsafeAssetUrl {
        url: url.to_owned(),
    };
    let (scheme, rest) = url.split_once("://").ok_or_else(reject)?;
    if scheme != "https" && !(cfg!(test) && scheme == "http") {
        return Err(reject());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(reject());
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    let github = GITHUB_HOSTS.contains(&host.as_str()) && port.is_none();
    let local_test_host = cfg!(test) && test_host == Some(host.as_str());
    if !github && !local_test_host {
        return Err(reject());
    }
    let path = &rest[authority.len()..];
    if path
        .split(['/', '?', '#'])
        .any(|segment| segment == ".." || segment.contains('\\'))
    {
        return Err(reject());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    use auro_sign::SigningKey;
    use serde_json::json;
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::{Delta, Rollout};

    fn sha256_bytes(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex(hasher.finalize())
    }

    fn hex(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    struct FakeGitHub {
        base: String,
        routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        _server: JoinHandle<()>,
    }

    impl FakeGitHub {
        fn new(requests: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let base = format!("http://{}", listener.local_addr().expect("addr"));
            let routes = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
            let server_routes = Arc::clone(&routes);
            let server = thread::spawn(move || {
                for _ in 0..requests {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut buffer = [0u8; 8192];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (status, body) = match server_routes.lock().expect("routes").get(&path) {
                        Some(body) => ("200 OK", body.clone()),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            FakeGitHub {
                base,
                routes,
                _server: server,
            }
        }

        fn set(&self, path: &'static str, body: Vec<u8>) {
            self.routes
                .lock()
                .expect("routes")
                .insert(path.to_owned(), body);
        }

        fn standard(&self, manifest: Vec<u8>, artifact: Option<Vec<u8>>) {
            self.set(
                RELEASE_PATH,
                self.release(
                    DEFAULT_MANIFEST_ASSET,
                    &format!("{}{MANIFEST_PATH}", self.base),
                ),
            );
            self.set(MANIFEST_PATH, manifest);
            if let Some(artifact) = artifact {
                self.set(ARTIFACT_PATH, artifact);
            }
        }

        fn release(&self, asset: &str, url: &str) -> Vec<u8> {
            serde_json::to_vec(&json!({
                "tag_name": "v1.2.3",
                "assets": [{ "name": asset, "browser_download_url": url }]
            }))
            .expect("release JSON")
        }

        fn channel(&self, key: &SigningKey, version: &str, state_dir: &Path) -> GithubChannel {
            let config = ChannelConfig::new("pdl-clay/auri", &key.public_key().to_base64());
            GithubChannel::new(config, state_dir, version)
                .expect("channel")
                .with_api_base(format!("{}/api.github.com", self.base))
        }
    }

    fn signed_manifest(
        key: &SigningKey,
        bytes: &[u8],
        version: &str,
        url: &str,
        platform: &str,
        delta: Option<Delta>,
        rollout: Option<Rollout>,
    ) -> Vec<u8> {
        let mut manifest = Manifest::new(
            version,
            platform,
            url,
            bytes.len() as u64,
            sha256_bytes(bytes),
            "notes",
            "2026-11-24T00:00:00Z",
        );
        if let Some(delta) = delta {
            manifest = manifest.with_delta(delta);
        }
        if let Some(rollout) = rollout {
            manifest = manifest.with_rollout(rollout);
        }
        manifest
            .sign(key)
            .expect("sign")
            .to_json()
            .expect("json")
            .into_bytes()
    }

    fn state_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    const RELEASE_PATH: &str = "/api.github.com/repos/pdl-clay/auri/releases/latest";
    const MANIFEST_PATH: &str = "/auro-update.json";
    const ARTIFACT_PATH: &str = "/App.AppImage";

    #[test]
    fn resolves_and_stages_a_signed_release() {
        let key = SigningKey::generate();
        let artifact = b"new AppImage bytes".to_vec();
        let state = state_dir();
        let github = FakeGitHub::new(5);
        let manifest = signed_manifest(
            &key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            None,
            None,
        );
        github.standard(manifest, Some(artifact.clone()));
        let channel = github.channel(&key, "1.0.0", state.path());

        assert_eq!(channel.check().expect("check"), UpdateDecision::Available);
        let staged = channel
            .stage(None)
            .expect("stage")
            .expect("newer release is staged");
        assert_eq!(staged.manifest().version, "1.2.3");
        assert_eq!(fs::read(staged.path()).expect("staged bytes"), artifact);
    }

    #[test]
    fn rejects_a_foreign_key_before_an_artifact_request() {
        let release_key = SigningKey::generate();
        let pin = SigningKey::generate();
        let artifact = b"should not be fetched".to_vec();
        let state = state_dir();
        let github = FakeGitHub::new(2);
        let manifest = signed_manifest(
            &release_key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            None,
            None,
        );
        github.standard(manifest, None);
        let channel = github.channel(&pin, "1.0.0", state.path());
        assert!(channel.check().is_err());
        assert!(!state.path().join("staging").join("artifact").exists());
    }

    #[test]
    fn rejects_bad_release_metadata_and_oversized_manifest() {
        let key = SigningKey::generate();
        let state = state_dir();
        let github = FakeGitHub::new(5);
        let channel = github.channel(&key, "1.0.0", state.path());

        github.set(
            RELEASE_PATH,
            github.release("other.json", &format!("{}{MANIFEST_PATH}", github.base)),
        );
        assert!(matches!(
            channel.manifest(),
            Err(UpdateError::ReleaseAssetMissing { .. })
        ));

        github.set(
            RELEASE_PATH,
            github.release(DEFAULT_MANIFEST_ASSET, "http://evil.test/auro-update.json"),
        );
        assert!(matches!(
            channel.manifest(),
            Err(UpdateError::UnsafeAssetUrl { .. })
        ));

        let safe_url = format!("{}{MANIFEST_PATH}", github.base);
        github.set(
            RELEASE_PATH,
            github.release(DEFAULT_MANIFEST_ASSET, &safe_url),
        );
        github.set(MANIFEST_PATH, vec![b'x'; MANIFEST_LIMIT + 1]);
        assert!(matches!(
            channel.manifest(),
            Err(UpdateError::ManifestTooLarge { .. })
        ));

        github.set(RELEASE_PATH, b"not json".to_vec());
        assert!(matches!(
            channel.manifest(),
            Err(UpdateError::ReleaseResponse(_))
        ));
    }

    #[test]
    fn honours_the_check_interval_and_records_the_last_check() {
        let key = SigningKey::generate();
        let state = state_dir();
        let github = FakeGitHub::new(0);
        let config = ChannelConfig::new("pdl-clay/auri", &key.public_key().to_base64())
            .with_interval_secs(90);
        let channel = GithubChannel::new(config, state.path(), "1.0.0")
            .expect("channel")
            .with_api_base(github.base.clone());
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(channel.due_at(now).expect("due on first check"));
        channel.record_check_at(now).expect("record");
        assert!(
            !channel
                .due_at(now + Duration::from_secs(89))
                .expect("not due before interval")
        );
        assert!(
            channel
                .due_at(now + Duration::from_secs(90))
                .expect("due at interval")
        );
        assert_eq!(channel.last_check().expect("last check"), Some(now));

        let no_start = ChannelConfig::new("pdl-clay/auri", &key.public_key().to_base64())
            .with_check_on_start(false);
        let fresh = tempfile::tempdir().expect("fresh state");
        let channel = GithubChannel::new(no_start, fresh.path(), "1.0.0")
            .expect("channel")
            .with_api_base(github.base.clone());
        assert!(!channel.due_at(now).expect("no cold-start check"));
    }

    #[test]
    fn prepares_an_appimage_apply() {
        let key = SigningKey::generate();
        let artifact = b"new AppImage bytes".to_vec();
        let state = state_dir();
        let github = FakeGitHub::new(3);
        let manifest = signed_manifest(
            &key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            None,
            None,
        );
        github.standard(manifest, Some(artifact));
        let channel = github.channel(&key, "1.0.0", state.path());
        let target = state.path().join("App.AppImage");
        fs::write(&target, b"old AppImage").expect("target");

        let journal = channel
            .prepare(&target, None)
            .expect("prepare")
            .expect("journal");
        assert_eq!(journal.kind(), ApplyKind::AppImage);
        assert_eq!(journal.target(), Some(target.as_path()));
        assert_eq!(journal.version(), "1.2.3");
        assert!(journal.manifest().is_some());
        assert!(journal.verify_key().is_some());
        assert_eq!(journal.state(), crate::ApplyState::Prepared);
    }

    #[test]
    fn stages_a_delta_or_the_full_artifact() {
        let key = SigningKey::generate();
        let base = b"old AppImage bytes".to_vec();
        let artifact = b"new AppImage bytes, longer than before".to_vec();
        let state = state_dir();
        let github = FakeGitHub::new(6);
        let base_path = state.path().join("base.AppImage");
        let new_path = state.path().join("new.AppImage");
        fs::write(&base_path, &base).expect("base");
        fs::write(&new_path, &artifact).expect("new artifact");
        let patch = crate::create_delta(&base_path, &new_path).expect("delta");
        let delta = Delta {
            from_version: "1.0.0".to_owned(),
            from_sha256: sha256_bytes(&base),
            url: format!("{}/patch.bin", github.base),
            size: patch.len() as u64,
            sha256: sha256_bytes(&patch),
        };
        let manifest = signed_manifest(
            &key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            Some(delta),
            None,
        );
        github.standard(manifest, None);
        github.set("/patch.bin", patch);
        let channel = github.channel(&key, "1.0.0", state.path());
        let staged = channel
            .stage(Some(&base_path))
            .expect("delta stage")
            .expect("staged");
        assert_eq!(fs::read(staged.path()).expect("bytes"), artifact);

        let second = state_dir();
        let manifest = signed_manifest(
            &key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            None,
            None,
        );
        github.standard(manifest, Some(artifact.clone()));
        let channel = github.channel(&key, "1.0.0", second.path());
        let staged = channel
            .stage(Some(&base_path))
            .expect("full stage")
            .expect("staged");
        assert_eq!(fs::read(staged.path()).expect("bytes"), artifact);
    }

    #[test]
    fn held_back_manifest_stages_nothing() {
        let key = SigningKey::generate();
        let artifact = b"new AppImage bytes".to_vec();
        let state = state_dir();
        let github = FakeGitHub::new(4);
        let manifest = signed_manifest(
            &key,
            &artifact,
            "1.2.3",
            &format!("{}{ARTIFACT_PATH}", github.base),
            &crate::current_platform(),
            None,
            Some(Rollout { percentage: 0 }),
        );
        github.standard(manifest, None);
        let channel = github.channel(&key, "1.0.0", state.path());
        assert_eq!(channel.check().expect("check"), UpdateDecision::HeldBack);
        assert!(channel.stage(None).expect("stage").is_none());
    }
}
