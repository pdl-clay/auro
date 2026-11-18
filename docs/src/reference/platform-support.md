# Platform support

> **Nothing has been released.** This table states what the roadmap promises at 1.0 and what is
> actually verified today. A row moves to "verified" only when CI exercises it on every pull
> request.

## Target matrix (1.0)

| Tier | Platform | Minimum | Targets |
|---|---|---|---|
| 1 | macOS | 12 (Monterey) | `aarch64-apple-darwin`, `x86_64-apple-darwin` |
| 1 | Linux | Ubuntu 22.04 | X11 and Wayland, `x86_64-unknown-linux-gnu` |
| 1 | Windows | 10 | `x86_64-pc-windows-msvc` |

Tier 1 means: builds in CI, tests run, a maintainer uses it, regressions are release blockers.

## Current status

Measured against CI run `44339911191` (push of `af7c224`, 2026-11-18): CI, Docs and Bundle smoke
all succeeded. "Not verified" still means no maintainer has exercised a real desktop session.

| Platform | Builds | Tests | Manually used | Notes |
|---|---|---|---|---|
| macOS | CI clippy green | CI tests green | - | Format step runs only on the Linux leg; no desktop-session verification; notarization and `.pkg` execution unverified |
| Linux (X11) | `ubuntu-latest` and `ubuntu-22.04` CI green | `ubuntu-latest` fmt, clippy and tests green | - | No desktop-session verification |
| Linux (Wayland) | unverified in CI | unverified in CI | local only (niri) | Phase 0/1 demos, hot-reload measurements and `auro bench` were run locally on niri/Wayland; no CI compositor job |
| Windows | CI clippy green | CI tests green | - | No desktop-session verification; MSI/NSIS install was exercised manually on Windows 11 earlier in Phase 2, the gate run does not install |

Text input, IME and accessibility have their own audit in
[Text input, IME and accessibility](../architecture/accessibility.md): GPUI 0.2.2 has IME plumbing
on all three platforms but no AccessKit, and Auro has no text-input primitive.

## Security-relevant verification gaps

- The single-instance private-directory, ownership and mode checks are unit-tested on Unix; no
  multi-user Linux or macOS desktop session has exercised them. Windows uses a machine-global named
  pipe and relies on its ACL; that isolation is unverified.
- Updater signature, size and SHA-256 paths are unit-tested on every OS; TUF-style key rotation,
  revocation and a real signed release have not been exercised. Update artifacts are hashed in
  bounded chunks, but platform installers are only integration-tested where the host tools exist.
- The macOS `.pkg` and notarization paths are command-tested but not run against Apple services;
  Windows MSI/NSIS is not installed by the CI gate. Both remain manual verification lines.

## Linux system dependencies

The CI job installs exactly this set to build the workspace
(`.github/workflows/ci.yml`):

```bash
sudo apt-get install -y --no-install-recommends \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libvulkan-dev libasound2-dev libfontconfig-dev libssl-dev pkg-config \
  libgtk-3-dev libayatana-appindicator3-dev
```

Contributors on other distributions need the equivalent packages; there is no per-distribution
script yet.

## Honesty rule

The roadmap names immature Windows/Linux support as the biggest schedule risk and commits to
publishing the support matrix from Phase 0. Therefore:

- an unverified platform is listed as unverified, not as supported;
- a platform that fails CI is removed from Tier 1 in the same change that breaks it;
- "it compiles" is not "it works" - text input, IME, clipboard and window management are separate
  verification lines.
