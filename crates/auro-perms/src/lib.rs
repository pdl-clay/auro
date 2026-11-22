// SPDX-License-Identifier: MIT OR Apache-2.0

//! Capability manifests and runtime enforcement.
//!
//! Auro does not sandbox first-party app code; capabilities guard the real boundaries: plugins,
//! untrusted content and the future webview escape hatch. A [`Policy`] is deny-by-default. Each
//! [`Grant`] names a subject, a [`Capability`] and, when the capability is scoped, a glob pattern
//! that the value must match. The check returns a typed [`Denied`] error, not a panic.

use std::fmt;
use std::str::FromStr;

use globset::{Glob, GlobMatcher};
use thiserror::Error;

/// Why a capability was refused.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PolicyError {
    #[error("invalid glob pattern `{0}`: {1}")]
    InvalidPattern(String, #[source] globset::Error),

    #[error("{capability} is scoped and requires a pattern")]
    ScopeRequired { capability: Capability },

    #[error("{capability} is not scoped and cannot carry a pattern")]
    ScopeNotSupported { capability: Capability },
}

/// A boundary that a grant can cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// File-system access.
    Fs,
    /// Network access.
    Net,
    /// External process execution.
    Shell,
    /// Clipboard access.
    Clipboard,
    /// Desktop notifications.
    Notification,
    /// Secrets in the platform keychain.
    Keychain,
}

impl Capability {
    /// The stable manifest spelling of this capability.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Fs => "fs",
            Capability::Net => "net",
            Capability::Shell => "shell",
            Capability::Clipboard => "clipboard",
            Capability::Notification => "notification",
            Capability::Keychain => "keychain",
        }
    }

    /// Whether a grant for this capability must carry a scope pattern.
    pub fn is_scoped(&self) -> bool {
        matches!(self, Capability::Fs | Capability::Net | Capability::Shell)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = UnknownCapability;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "fs" => Ok(Capability::Fs),
            "net" => Ok(Capability::Net),
            "shell" => Ok(Capability::Shell),
            "clipboard" => Ok(Capability::Clipboard),
            "notification" => Ok(Capability::Notification),
            "keychain" => Ok(Capability::Keychain),
            _ => Err(UnknownCapability(text.to_owned())),
        }
    }
}

/// A capability string the policy does not recognise.
#[derive(Debug, Error)]
#[error("unknown capability `{0}`")]
#[non_exhaustive]
pub struct UnknownCapability(pub String);

/// A compiled glob scope.
#[derive(Clone, Debug)]
struct Pattern {
    text: String,
    matcher: GlobMatcher,
}

impl Pattern {
    fn new(text: impl Into<String>) -> Result<Self, PolicyError> {
        let text = text.into();
        let matcher = Glob::new(&text)
            .map_err(|error| PolicyError::InvalidPattern(text.clone(), error))?
            .compile_matcher();
        Ok(Pattern { text, matcher })
    }

    fn matches(&self, value: &str) -> bool {
        self.matcher.is_match(value)
    }
}

/// One deny-by-default allowance.
#[derive(Clone, Debug)]
pub struct Grant {
    subject: String,
    capability: Capability,
    pattern: Option<Pattern>,
}

impl Grant {
    /// Grants `capability` to `subject` without a scope. Only useful for unscoped capabilities.
    pub fn new(subject: impl Into<String>, capability: Capability) -> Result<Self, PolicyError> {
        if capability.is_scoped() {
            return Err(PolicyError::ScopeRequired { capability });
        }
        Ok(Grant {
            subject: subject.into(),
            capability,
            pattern: None,
        })
    }

    /// Grants `capability` to `subject` for values matching the glob pattern.
    pub fn scoped(
        subject: impl Into<String>,
        capability: Capability,
        pattern: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        if !capability.is_scoped() {
            return Err(PolicyError::ScopeNotSupported { capability });
        }
        Ok(Grant {
            subject: subject.into(),
            capability,
            pattern: Some(Pattern::new(pattern)?),
        })
    }

    /// The subject this grant applies to.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The capability this grant covers.
    pub fn capability(&self) -> Capability {
        self.capability
    }

    /// The scope pattern, when the grant is scoped.
    pub fn pattern(&self) -> Option<&str> {
        self.pattern.as_ref().map(|pattern| pattern.text.as_str())
    }

    fn matches(&self, subject: &str, capability: Capability, value: Option<&str>) -> bool {
        if self.subject != subject || self.capability != capability {
            return false;
        }
        match (self.pattern.as_ref(), value) {
            (Some(pattern), Some(value)) => pattern.matches(value),
            (None, None) => true,
            _ => false,
        }
    }
}

/// A typed denial.
#[derive(Debug, Error)]
#[error(
    "capability {capability} denied for {subject}{}",
    .value.as_ref().map(|value| format!(" (`{value}`)")).unwrap_or_default()
)]
#[non_exhaustive]
pub struct Denied {
    /// The subject that asked.
    pub subject: String,
    /// The capability that was refused.
    pub capability: Capability,
    /// The value that failed to match a scope, when the check was scoped.
    pub value: Option<String>,
}

/// The compiled capability manifest.
///
/// Auro ships the manifest and answers verdicts from it. First-party app code is not sandboxed;
/// this policy only guards subjects the host deems untrusted.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    grants: Vec<Grant>,
}

impl Policy {
    /// A policy with no grants: every check is denied.
    pub fn deny_all() -> Self {
        Policy::default()
    }

    /// Adds one allowance.
    pub fn allow(mut self, grant: Grant) -> Self {
        self.grants.push(grant);
        self
    }

    /// Adds a scoped allowance for values matching the glob pattern.
    pub fn allow_scope(
        self,
        subject: impl Into<String>,
        capability: Capability,
        pattern: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        Ok(self.allow(Grant::scoped(subject, capability, pattern)?))
    }

    /// The grants currently compiled into this policy.
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// Answers a capability check for `subject`.
    pub fn check(&self, subject: &str, capability: Capability) -> Result<(), Denied> {
        if self
            .grants
            .iter()
            .any(|grant| grant.matches(subject, capability, None))
        {
            return Ok(());
        }
        Err(Denied {
            subject: subject.to_owned(),
            capability,
            value: None,
        })
    }

    /// Answers a scoped capability check for `subject`.
    ///
    /// Unscoped capabilities fall back to [`Self::check`]; the value is ignored there. A
    /// filesystem scope is lexical: the value must be an absolute path without `.` or `..`
    /// components, otherwise a plugin could request `/allowed/../secret` and match a prefix
    /// pattern. Symlinks are not resolved here; the call site that performs the effect must
    /// canonicalize and re-check when symlink traversal matters.
    pub fn check_scope(
        &self,
        subject: &str,
        capability: Capability,
        value: &str,
    ) -> Result<(), Denied> {
        if !capability.is_scoped() {
            return self.check(subject, capability);
        }
        if capability == Capability::Fs && !is_absolute_path_without_dots(value) {
            return Err(Denied {
                subject: subject.to_owned(),
                capability,
                value: Some(value.to_owned()),
            });
        }
        if capability == Capability::Shell && !has_no_dot_components(value) {
            return Err(Denied {
                subject: subject.to_owned(),
                capability,
                value: Some(value.to_owned()),
            });
        }
        if self
            .grants
            .iter()
            .any(|grant| grant.matches(subject, capability, Some(value)))
        {
            return Ok(());
        }
        Err(Denied {
            subject: subject.to_owned(),
            capability,
            value: Some(value.to_owned()),
        })
    }
}

/// Whether `value` is a rooted path whose components are only normal names.
///
/// `Path::is_absolute` is false on Windows for a Unix-style `/var/...` value, which capability
/// tests and cross-platform plugin manifests use. A leading separator is rooted (the current
/// volume on Windows) and is accepted; a drive-relative `C:foo` is not.
fn is_absolute_path_without_dots(value: &str) -> bool {
    use std::path::Path;

    (Path::new(value).is_absolute() || value.starts_with(['/', '\\']))
        && has_no_dot_components(value)
}

/// Whether a value has no `.` or `..` path components.
///
/// This is lexical: `Path::components` normalizes a `.` away, so the raw separators are checked.
/// A shell command string is not parsed, so a caller that passes a full command rather than an
/// executable path must still validate its own syntax.
fn has_no_dot_components(value: &str) -> bool {
    !value
        .split(['/', '\\'])
        .any(|component| component == "." || component == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy::deny_all()
            .allow(Grant::new("app", Capability::Clipboard).expect("unscoped clipboard"))
            .allow(Grant::new("app", Capability::Notification).expect("unscoped notification"))
            .allow_scope("plugin:notes", Capability::Fs, "/var/lib/notes/**")
            .expect("scope")
            .allow_scope("plugin:web", Capability::Net, "*.example.com")
            .expect("scope")
    }

    #[test]
    fn denies_by_default() {
        let policy = Policy::deny_all();
        assert!(policy.check("app", Capability::Clipboard).is_err());
        assert!(policy.check_scope("app", Capability::Fs, "/tmp").is_err());
    }

    #[test]
    fn allows_an_unscoped_capability() {
        let policy = policy();
        policy
            .check("app", Capability::Clipboard)
            .expect("clipboard");
        policy
            .check("app", Capability::Notification)
            .expect("notification");
        assert!(policy.check("app", Capability::Keychain).is_err());
    }

    #[test]
    fn matches_a_scoped_grant() {
        let policy = policy();
        policy
            .check_scope("plugin:notes", Capability::Fs, "/var/lib/notes/a.txt")
            .expect("allowed path");
        assert!(
            policy
                .check_scope("plugin:notes", Capability::Fs, "/var/lib")
                .is_err()
        );
        assert!(
            policy
                .check_scope("plugin:notes", Capability::Fs, "/etc/passwd")
                .is_err()
        );
    }

    #[test]
    fn rejects_an_out_of_scope_value() {
        let policy = policy();
        let error = policy
            .check_scope("plugin:web", Capability::Net, "attacker.test")
            .expect_err("denied");
        assert_eq!(error.subject, "plugin:web");
        assert_eq!(error.capability, Capability::Net);
        assert_eq!(error.value.as_deref(), Some("attacker.test"));
    }

    #[test]
    fn scopes_are_subject_scoped() {
        let policy = policy();
        assert!(
            policy
                .check_scope("plugin:other", Capability::Fs, "/var/lib/notes/a.txt")
                .is_err()
        );
        assert!(policy.check("plugin:notes", Capability::Clipboard).is_err());
    }

    #[test]
    fn composes_grants() {
        let policy = policy();
        assert_eq!(policy.grants().len(), 4);
        assert!(
            policy
                .check_scope("plugin:notes", Capability::Fs, "/var/lib/notes/deep/file")
                .is_ok()
        );
    }

    #[test]
    fn capability_strings_round_trip() {
        for text in [
            "fs",
            "net",
            "shell",
            "clipboard",
            "notification",
            "keychain",
        ] {
            let capability: Capability = text.parse().expect("capability");
            assert_eq!(capability.as_str(), text);
        }
        assert!("nope".parse::<Capability>().is_err());
        assert_eq!(Capability::Fs.to_string(), "fs");
        assert!(Capability::Fs.is_scoped());
        assert!(!Capability::Clipboard.is_scoped());
    }

    #[test]
    fn rejects_a_traversal_or_relative_filesystem_value() {
        let policy = policy();
        for value in [
            "/var/lib/notes/../secret",
            "/var/lib/notes/./a.txt",
            "relative/a.txt",
            "/var/lib/notes/../../etc/passwd",
        ] {
            assert!(
                policy
                    .check_scope("plugin:notes", Capability::Fs, value)
                    .is_err(),
                "`{value}` must not pass a lexical scope"
            );
        }
        policy
            .check_scope("plugin:notes", Capability::Fs, "/var/lib/notes/a.txt")
            .expect("a normal absolute path");

        let shell = Policy::deny_all()
            .allow_scope("plugin", Capability::Shell, "/usr/bin/**")
            .expect("shell scope");
        assert!(
            shell
                .check_scope("plugin", Capability::Shell, "/usr/bin/../tmp/evil")
                .is_err()
        );
        shell
            .check_scope("plugin", Capability::Shell, "/usr/bin/git")
            .expect("a normal executable");
    }

    #[test]
    fn rejects_an_invalid_pattern() {
        assert!(matches!(
            Grant::scoped("app", Capability::Fs, "[invalid"),
            Err(PolicyError::InvalidPattern(_, _))
        ));
    }

    #[test]
    fn rejects_a_scope_mismatch() {
        assert!(matches!(
            Grant::new("app", Capability::Fs),
            Err(PolicyError::ScopeRequired {
                capability: Capability::Fs
            })
        ));
        assert!(matches!(
            Grant::scoped("app", Capability::Clipboard, "*"),
            Err(PolicyError::ScopeNotSupported {
                capability: Capability::Clipboard
            })
        ));
    }

    #[test]
    fn unscoped_check_ignores_the_value() {
        let policy = policy();
        policy
            .check_scope("app", Capability::Clipboard, "ignored")
            .expect("clipboard");
    }

    #[test]
    fn raw_plugin_subject_string_has_no_implicit_grant() {
        let policy = Policy::deny_all();
        let subject = "plugin:notes@1.0.0.deadbeef.inst_1";
        assert!(policy.check(subject, Capability::Clipboard).is_err());
        assert!(
            policy
                .check_scope(subject, Capability::Fs, "/var/lib/notes/a.txt")
                .is_err()
        );
    }
}
