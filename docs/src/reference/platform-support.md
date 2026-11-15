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

| Platform | Builds | Tests | Manually used | Notes |
|---|---|---|---|---|
| macOS | unverified | unverified | - | GPUI's most mature platform; CI must confirm the workspace |
| Linux (X11) | builds locally | unit tests pass | - | CI installs the system libraries below |
| Linux (Wayland) | unverified | unverified | - | Requires a compositor; not testable in CI |
| Windows | unverified | unverified | - | GPUI support exists upstream; parity unproven |

## Linux system dependencies

The CI job installs exactly this set to build the workspace
(`.github/workflows/ci.yml`):

```bash
sudo apt-get install -y \
  libxkbcommon-dev libwayland-dev libx11-xcb-dev libvulkan-dev \
  libasound2-dev libfontconfig-dev libssl-dev pkg-config
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
