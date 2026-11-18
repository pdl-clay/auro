# 0032 - macOS notarization submission

**Status:** Implemented in tree (2026-11-18; `auro notarize`, keychain-profile submission and
stapling). Apple credentials, a notarized artifact and macOS runtime verification are not available
on this host.

Apple distributes signed macOS software through notarization. `tauri-bundler` handles packaging and
codesigning inputs, but it does not replace the release operator's notarytool submission. This spec
adds the smallest explicit release command: submit an artifact with an existing keychain profile,
then staple the ticket.

## Scope

- `auro notarize --artifact <path> --profile <keychain-profile>`.
- `MacNotarizer`/`NotarizeRunner` in `auro-bundle`.
- `xcrun notarytool submit <artifact> --keychain-profile <profile> --wait`.
- `xcrun stapler staple <artifact>` unless `--skip-staple` is set.

## Out of scope

- Codesigning; the artifact must already be signed with a Developer ID identity.
- Creating or storing the keychain profile, Apple ID/API-key handling or secrets in config.
- Gatekeeper assessment on a target Mac, stapling a remote copy, or notarization of an unsigned
  `.app`.
- Automatic notarization inside `auro bundle`.
- Windows Authenticode and Linux package signatures.
- A notarization service account, CI secret policy or release-channel policy.

## Decisions

### D1 - `notarytool` is invoked shell-free with an absolute `xcrun`

`MacNotarizer` builds an argument vector and runs `/usr/bin/xcrun` through `NotarizeRunner`:

```text
/usr/bin/xcrun notarytool submit <artifact> --keychain-profile <profile> --wait
/usr/bin/xcrun stapler staple <artifact>
```

No shell, command string, PATH lookup or credential in argv. The profile name identifies an entry
created with `xcrun notarytool store-credentials`; Auro never reads or prints the password.

**Rejected:** embedding Apple ID/password/API key in `Auro.toml` or environment variables. A
keychain profile is the supported macOS secret boundary. **Rejected:** calling `notarytool` through
a shell string; it adds quoting risk without a capability.

### D2 - Submission and stapling are separate, explicit steps

Submission must succeed before stapling; a failed submission does not start stapling. `--skip-staple`
is for release pipelines that staple a downloaded copy elsewhere or deliberately defer it. A
non-zero command is an `AuroError::Bundle` with the action and exit code. The command does not
delete, modify or re-sign the artifact.

The command is intentionally not hidden behind `auro bundle`: notarization needs Apple
credentials, network access and a signed artifact, and a local bundle run should remain usable
offline.

### D3 - Notarization is release hardening, not update trust

The updater already verifies the signed manifest and artifact hash. Notarization adds an Apple
Gatekeeper claim; it does not replace the Auro signature, and a missing ticket must not be silently
treated as a successful release. Runtime Gatekeeper acceptance is a separate macOS verification.

## Interfaces

```rust
pub trait NotarizeRunner {
    fn run(&self, program: &Path, args: &[OsString])
        -> Result<Option<i32>, AuroError>;
}

pub struct SystemNotarizeRunner;

pub struct MacNotarizer { /* runner */ }

impl MacNotarizer {
    pub fn new() -> Self;
    pub fn with_runner(runner: impl NotarizeRunner + 'static) -> Self;
    pub fn notarize(
        &self,
        artifact: &Path,
        profile: &str,
        skip_staple: bool,
    ) -> Result<(), AuroError>;
}
```

CLI:

```text
auro notarize --artifact target/release/bundle/dmg/Demo.dmg --profile auro-release
auro notarize --artifact Demo.pkg --profile auro-release --skip-staple
```

A missing artifact or an empty profile is rejected before any command runs.

## Acceptance

1. Submission and stapling use the exact `xcrun` argument vectors
   (`submits_and_staples_with_xcrun`).
2. `--skip-staple`/`skip_staple=true` runs only submission (`can_skip_stapling`).
3. A failed submission does not run stapling (`reports_submission_failure_and_does_not_staple`).
4. A failed staple is reported (`reports_stapling_failure`).
5. Missing artifacts and empty profiles fail before running a command
   (`rejects_a_missing_artifact_or_empty_profile`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

|Question|Where it settles|
|---|---|
|CI keychain profile and Apple credential policy|Release CI follow-up|
|Automatic notarization after `auro bundle`|Release UX/spec|
|Gatekeeper assessment on a real Mac|macOS release verification|
|Notarizing a zipped `.app` and stapling the unpacked bundle|macOS packaging follow-up|
