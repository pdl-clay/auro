# 0020 - Notarization uses a keychain profile through `notarytool`

**Status:** Accepted (2026-11-18)

## Context

macOS releases need a notarization ticket, but Apple credentials are secrets and Auro has no
business storing them. `notarytool` supports a login-keychain profile, and `stapler` attaches the
ticket to a `.pkg`, `.dmg` or zipped artifact. The alternative is to hide notarization inside a
bundler invocation, which makes network/credential failures look like packaging failures and forces
an Apple account onto every local bundle.

## Decision

`auro notarize` takes an artifact and an existing keychain profile. `MacNotarizer` runs
`xcrun notarytool submit ... --wait` followed by `xcrun stapler staple ...` through an injectable,
shell-free `NotarizeRunner`. Submission and stapling are separate and explicit; `--skip-staple`
defers the second step. Auro never handles the Apple password, API key or profile creation.

Notarization is release hardening. It does not replace the signed update manifest, artifact hash or
Auro signature, and the command does not claim Gatekeeper acceptance without a real Mac check.

## Consequences

**Easier**

- Release operators can notarize with the standard macOS credential store.
- Command construction and failure ordering are testable without Apple credentials.
- Local `auro bundle` stays offline and usable without an Apple account.

**Harder**

- The operator must create and maintain the keychain profile.
- The command needs macOS, Xcode command-line tools, network and a signed artifact.
- CI needs a secret-provisioning policy for the profile.
- Automated integration into `auro bundle` and real Gatekeeper verification remain future work.

**Forbidden**

- Storing an Apple ID password/API key in `Auro.toml`, source or argv.
- Building a shell command from user-controlled values.
- Reporting a release as notarized when submission or stapling failed.
- Treating notarization as a substitute for Auro's manifest signature.

## Revisit when

- A macOS release CI with a managed keychain profile exists.
- `auro bundle` gains an opt-in notarization phase.
- Zipped `.app` signing/stapling policy is specified.
