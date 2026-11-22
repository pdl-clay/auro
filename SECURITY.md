# Security policy

## Status

Auro is **pre-alpha**. There is no release, no published crate and no installable binary. Nothing
here is ready to run untrusted plugins or untrusted content, and the capability model is not
implemented yet.

Because there are no versions to support, this policy has no supported-version table. One will be
added at the first tagged release.

## Plugin trust

Auro capabilities scope the effects that go through the host. Plugins are native code: an out-of-process plugin runs as the same OS user and can use the filesystem, network and processes directly, and an in-process dylib plugin shares the app's address space. A grant is not a sandbox and does not constrain code that bypasses the host.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private reporting:

https://github.com/pdl-clay/auro/security/advisories/new

Include, as far as you can:

- what the issue is, and the impact you believe it has;
- the commit or file involved (this project has no versions yet, so a commit SHA is the reference);
- a reproduction, or the smallest code sample that shows it;
- any known mitigation.

## What happens next

- Acknowledgement target: **72 hours**, best effort.
- An initial assessment, then a fix and a coordinated disclosure date agreed with you.
- Credit in the advisory unless you ask not to be named.

No bounty is offered at this stage. The absence of a bounty does not change the disclosure process.

## In scope

- Anything under the `auro-*` crates once they exist: the CLI, the runtime, the IPC boundary, the
  capability enforcement points, the plugin host.
- The updater and signing code: signature verification, key handling, rollback.
- The website and docs build, if it can be used to serve modified content or steal credentials.
- The capability model itself: a bypass, a scope that matches more than it should, a plugin escape.

## Out of scope

- **GPUI upstream.** Report to [zed-industries/zed](https://github.com/zed-industries/zed). If Auro
  vendors a patch, tell us as well so we can carry the fix.
- **Tauri.** Different project.
- **The operating system and its webview**, when the issue is in the platform component rather than
  in how Auro uses it.
- **Documentation-only findings** with no security impact, such as a missing security header on a
  static page. Open a normal issue for those.

## If you are unsure

Report privately anyway. A false positive costs a maintainer an hour; a public disclosure of a real
issue costs users their trust.
