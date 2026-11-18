# 0032 - macOS notarization submission

**Status:** Implemented in tree (2026-11-18; keychain-profile submission, stapling, local
verification, `.app` bundle archiving and opt-in `auro bundle` integration). Apple credentials, a
notarized artifact and macOS runtime verification are not available on this host.

Apple distributes signed macOS software through notarization. `tauri-bundler` handles packaging and
codesigning inputs, but it does not replace the release operator's notarytool submission. This spec
adds the smallest explicit release command: submit an artifact with an existing keychain profile,
then staple the ticket.

## Scope

- `auro notarize --artifact <path> --profile <keychain-profile>`.
- `auro notarize --artifact <path> --verify` for local ticket and Gatekeeper checks.
- `MacNotarizer`/`NotarizeRunner` in `auro-bundle`.
- `xcrun notarytool submit <artifact> --keychain-profile <profile> --wait`.
- `xcrun stapler staple <artifact>` unless `--skip-staple` is set, then
  `xcrun stapler validate <artifact>`.
- `.app` bundles are zipped with `ditto -c -k --keepParent` for submission and the ticket is
  stapled to the bundle.
- `[bundle.macos] notarize-profile` makes `auro bundle` notarize and verify each produced
  `.app`/`.dmg`.

## Out of scope

- Codesigning; the artifact must already be signed with a Developer ID identity.
- Creating or storing the keychain profile, Apple ID/API-key handling or secrets in config.
- Gatekeeper assessment on a target Mac as proof of end-user acceptance; `spctl --assess` is a
  local check, not a formal notarization result.
- Stapling a remote copy, or notarization of an unsigned `.app`.
- Rebuilding or reordering the bundler's DMG after the `.app` ticket is stapled.
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

### D2 - Submission, stapling and validation are separate, explicit steps

Submission must succeed before stapling; a failed submission does not start stapling and a failed
staple does not start validation. `--skip-staple` is for release pipelines that staple a downloaded
copy elsewhere or deliberately defer it. A non-zero command is an `AuroError::Bundle` with the
action and exit code. The command does not delete, modify or re-sign the artifact.

`--verify` is the offline half: `xcrun stapler validate` proves the ticket is attached and
`spctl --assess` asks Gatekeeper whether the artifact is accepted for its type (`.app` is
`execute`, `.pkg` is `install`, `.dmg` is an `open` primary-signature check). It does not
resubmit and does not prove a real user can launch the app.

A bare `.zip` is submitted only with `--skip-staple`; a ticket cannot be stapled to a zip, and
silently skipping that step would misrepresent the result.

### D3 - Notarization is release hardening, not update trust

The updater already verifies the signed manifest and artifact hash. Notarization adds an Apple
Gatekeeper claim; it does not replace the Auro signature, and a missing ticket must not be silently
treated as a successful release. Runtime Gatekeeper acceptance is a separate macOS verification.

### D4 - `auro bundle` integration is opt-in and post-hoc

Setting `[bundle.macos] notarize-profile` makes `auro bundle` call the same `MacNotarizer` after
the bundler returns. App bundles are processed before disk images. Because `tauri-bundler` has
already built the DMG from the app, stapling the app afterwards does not update the copy inside
that DMG; a first launch that must be accepted offline should either use the bundler's
environment-credential path (which notarizes the app before the DMG is built) or rebuild the DMG
after stapling. The integration is a release convenience, not a claim that ordering is solved.

If `tauri-bundler` already notarized the app from `APPLE_*` environment credentials, the profile
path submits it a second time. Use one credential path per release. On hosts that cannot produce
macOS artifacts, the key is ignored: a Linux or Windows bundle run still produces its native
formats and does not attempt an Apple command.

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
    pub fn verify(&self, artifact: &Path) -> Result<(), AuroError>;
    pub fn notarize_artifacts(
        &self,
        artifacts: &[PathBuf],
        profile: &str,
    ) -> Result<(), AuroError>;
}
```

CLI:

```text
auro notarize --artifact target/release/bundle/dmg/Demo.dmg --profile auro-release
auro notarize --artifact Demo.pkg --profile auro-release --skip-staple
auro notarize --artifact target/release/bundle/macos/Demo.app --verify
```

Config:

```toml
[bundle.macos]
notarize-profile = "auro-release"
```

A missing artifact, a missing `.app` directory, an empty profile, or an unsupported extension is
rejected before any command runs.

## Acceptance

1. Submission, stapling and validation use the exact `xcrun` argument vectors
   (`submits_staples_and_validates_with_xcrun`).
2. `--skip-staple`/`skip_staple=true` runs only submission (`can_skip_stapling`).
3. A failed submission does not run stapling (`reports_submission_failure_and_does_not_staple`).
4. A failed staple or validation is reported (`reports_stapling_failure_and_does_not_validate`,
   `reports_stapler_validation_failure`).
5. A `.app` is archived with `ditto --keepParent`, submitted as a zip, and the ticket is stapled
   and validated on the bundle (`zips_and_notarizes_an_app_bundle`).
6. A bare zip requires `--skip-staple` (`a_zip_needs_skip_staple_and_then_submits_only`).
7. `verify` runs `stapler validate` and the correct `spctl --assess` arguments for `.app`, `.pkg`
   and `.dmg` (`verifies_app_pkg_and_dmg_with_stapler_and_gatekeeper`) and reports rejection
   (`reports_gatekeeper_rejection`).
8. `notarize_artifacts` processes `.app` before `.dmg` and ignores non-macOS formats
   (`notarizes_and_verifies_macos_artifacts_and_skips_other_formats`).
9. Missing artifacts, empty profiles and unsupported extensions fail before running a command
   (`rejects_a_missing_artifact_empty_profile_or_unknown_extension`).
10. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

|Question|Where it settles|
|---|---|
|CI keychain profile and Apple credential policy|Release CI follow-up|
|Real `notarytool` submission and Gatekeeper acceptance on a Mac|macOS release verification/credentials|
|DMG ordering with a stapled `.app`|macOS packaging follow-up|
|Automatic notarization for a `.pkg` built outside `tauri-bundler`|macOS packaging follow-up|
