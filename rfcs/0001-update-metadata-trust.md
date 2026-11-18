# 0001 - Update metadata trust: freshness, rotation and revocation

- **Status:** Draft
- **Author(s):** @pdl-clay
- **Created:** 2026-11-18
- **Supersedes:** none

## Summary

Auro currently verifies one Ed25519 signature over a manifest and compares semantic versions. That
detects tampering, but it cannot detect a stale-but-valid manifest (freeze), a replay after a
rollback, a compromised release key, or a mirror that withholds a security update. This RFC
proposes a TUF-inspired metadata subset for Auro: an offline root that delegates release keys,
short-lived timestamp metadata, a signed snapshot for consistency, an expiry on every role, a
monotonic generation persisted in private state, and a signed revocation list. It is a proposal,
not an implemented format change. Manifest v3 must not ship without an accepted RFC and a
migration plan.

## Motivation

The current contract is in [spec 0012](../docs/src/specs/0012-update-manifests.md) and
[spec 0013](../docs/src/specs/0013-updater-runtime.md): one pinned key per app, a signed JSON
manifest with version, platform, URL, size, SHA-256, notes, publication time and optional
delta/rollout fields, and an updater that rejects an offer not newer than the running version.
That is a reasonable first release, but it leaves these failures:

- **Freeze.** A mirror can keep serving a valid old manifest. The client has no timestamp or expiry
  to distinguish "no update exists" from "the mirror is withholding one".
- **Replay and downgrade.** The normal updater rejects older versions, but an explicit rollback or
  a reinstall can make an old signed release acceptable again. The journal records a version but no
  monotonic generation.
- **Key compromise.** The release key can sign any version until users install a build with a new
  pinned key. There is no key ID, delegation, threshold or revocation path, so recovery requires an
  out-of-band app update.
- **Mix-and-match.** A manifest is verified in isolation. There is no snapshot binding the
  manifest, delta and rollback package to the same repository state.
- **Malicious mirror.** TLS and the signature prevent forgery, but not withholding, stale data or
  traffic analysis. Size bounds already stop endless-data responses; wrong-platform and
  delta-base-mismatch offers are already rejected.
- **Local TOCTOU.** Verify-then-open still leaves a window if the staging directory is writable by
  another local user. The journal path is now validated ([spec 0033](../docs/src/specs/0033-update-journal-trust.md)),
  but staged artifacts and installer handoff are not.

The audience is applications that enable the updater and ship signed releases. The cost today is a
security property the project cannot honestly claim: "the update you get is the latest one the
publisher intended for you."

## Guide-level explanation

The app pins one or more root public keys at build time. A release operator holds the offline root
key and an online release key. The update endpoint serves four small signed documents plus the
artifact:

```
root.json          offline root key(s), release key IDs, thresholds, expiry
timestamp.json     short-lived online signature over the current snapshot hash
snapshot.json      hashes and versions of targets.json (and optional delegated files)
targets.json       artifacts: version, platform, URL, size, SHA-256, generation
manifest.json      the existing signed artifact manifest (v3) referenced by targets
```

The client verifies root → timestamp → snapshot → targets → manifest/artifact, in that order. Each
document carries an expiry and a generation. The client stores the highest trusted root, timestamp,
snapshot and generation in private state and refuses a lower value. A revoked key is refused even
if its signature is otherwise valid.

A release-key rotation is a new `root.json` signed by the old root key (or by a threshold of root
keys). The root names the new release key with a `not_before`/`not_after` window; old clients keep
using their pinned root until they install a build carrying the new root. A root-key rotation or a
root compromise still needs an app update or a pre-pinned secondary root; metadata alone cannot
recover a client whose only root key is gone. A revocation list is a root-signed document with key
IDs, an effective time and a minimum generation; it blocks future updates from a revoked key but
cannot recall an already installed binary.

This is deliberately smaller than full TUF. It borrows TUF's roles, expiry, thresholds and
rollback/freeze reasoning, but it does not implement delegations, consistent snapshots, hashed
binaries or a general-purpose repository client. If Auro later adopts full TUF, that is a new RFC
and a reviewed implementation, not a home-grown extension of this subset.

## Reference-level explanation

### Metadata v3

The manifest keeps its current signed payload and adds:

```rust
pub struct MetadataV3 {
    pub format: u32,           // 3
    pub key_id: String,        // "ed25519:<sha256-prefix>"
    pub generation: u64,       // monotonically increasing repository generation
    pub published_at: String,  // RFC 3339
    pub expires_at: String,    // RFC 3339; required, not optional
    pub version: String,       // SemVer
    pub platform: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub min_version: Option<String>, // oldest running version allowed to apply it
    pub snapshot_sha256: String,     // binds the manifest to snapshot.json
    pub signature: String,           // Ed25519 over the canonical payload
}
```

`root.json` carries:

```rust
pub struct RootMetadata {
    pub format: u32,
    pub expires_at: String,
    pub version: u64,
    pub keys: Vec<RootKey>,       // key_id, public_key, not_before, not_after
    pub roles: Roles,             // release: threshold, key_ids; timestamp: threshold, key_ids
    pub signature: Vec<String>,   // threshold signatures
}
```

The exact JSON field names and canonicalization are fixed by a spec before implementation. The
existing `auro-sign` Ed25519 primitives and canonical JSON are reused; no new cryptographic
primitive is introduced. Threshold verification is a loop over distinct key IDs, not a new
signature scheme.

### Verification order and failure behavior

1. Load the persisted root and generation floor from private state. The state file uses the same
   private-directory validation as the journal ([spec 0033](../docs/src/specs/0033-update-journal-trust.md)).
2. Verify the fetched root: threshold, expiry, `version >= persisted root version`, and that the
   new root is signed by a threshold of keys trusted by the persisted root. A root fetched over
   the network is never trusted because it is newer.
3. Verify `timestamp.json`: expiry, release-key threshold, `generation >= persisted generation`,
   and `snapshot_sha256`.
4. Verify `snapshot.json`: expiry, generation, `targets_sha256`, and that its version is not lower
   than the persisted snapshot version.
5. Verify `targets.json`: expiry, generation, artifact entry (version, platform, URL, size,
   SHA-256, min_version), and the manifest's `snapshot_sha256`.
6. Verify the artifact size/SHA-256 and its signature with the key named by targets. The key's
   `not_before`/`not_after` and the revocation list are checked here as well.
7. Persist the new root/timestamp/snapshot/generation only after the whole chain and the artifact
   verification succeed. A partially verified repository update is never persisted.
8. Refuse automatic updates when a timestamp is expired beyond the configured clock-skew window.
   A manual "check anyway" may be offered by the app, but it must keep signature, expiry and
   generation checks; it cannot turn off revocation.

The clock is an input, not a proof. Tests must cover a clock before `published_at`, a clock after
`expires_at`, a clock rollback, and a mirror that returns an old timestamp with a valid signature.
The skew window is policy (a value such as 24 hours is a starting point, not a measured fact).

### Rotation and revocation

- **Release key rotation.** Root signs a new `root.json` that adds the new release key and keeps
  the old key valid until `not_after`. Old clients continue to trust their pinned root; they do
  not learn the new key until they install a build that carries the new root. This is a real
  limitation, not a metadata bug.
- **Root key rotation.** The new root must be signed by a threshold of the old root keys. If the
  old root is lost, a build with a new root must be distributed out of band. A pre-pinned
  secondary root key, used only for rotation, reduces this risk; whether to require it is an open
  question.
- **Revocation.** A root-signed revocation document lists `key_id`, `effective_at` and
  `min_generation`. A revoked key is refused for target/manifest signatures and for new root
  signatures after the effective time. Revocation cannot uninstall a malicious release; it blocks
  future updates signed by that key and can force a minimum generation.
- **Compromise drill.** If the release key leaks, publish a new root (signed by the offline root)
  that removes it, plus a revocation document. Clients that fetch the new root before their
  timestamp expires recover without an app update. Clients offline past expiry fail closed until
  they can fetch a fresh timestamp or install a new build.

### Downgrade and replay

The normal path requires `offered > current_version` (already implemented) and
`generation > persisted_generation`. The explicit package rollback API remains an opt-in exception:
it is caller-supplied, verified against a strictly older manifest, and records the new floor and
reason. A reinstall cannot silently accept an old generation because the floor is persisted in the
private state directory; if that state is lost, the reinstall is a new trust epoch and the app must
say so in its UI/log.

### Staging and TOCTOU

The `Updater` staging directory is supplied by the app. It should be validated with the same
private-directory rule as the journal; the current `Updater::new` does not do that. A follow-up
should:

- validate/create the staging directory private (`0700`/user ACL), and
- verify the artifact from the same opened handle that is later applied, or copy it into a private
  content-addressed file and re-verify immediately before handing it to the installer.

For `.deb`/`.rpm`, the helper already passes `/proc/<pid>/fd/<fd>` to `pkexec`; that is the model to
extend. For MSI/NSIS and macOS `.app`/`.pkg`, the external installer opens a path, so a residual
verify-then-open window remains unless the platform offers a handle-based install API. The contract
must state that residual risk instead of claiming a fix.

### Compatibility and migration

- Manifest v2 clients keep working. A v3 client may accept v2 only while no v3 metadata has been
  seen, and must never fall back to v2 after a v3 generation; otherwise a mirror can downgrade the
  metadata format.
- The release switch is a separate PR: publish root/timestamp/snapshot/targets first, keep the v2
  manifest for one release, then require v3. The exact dual-read window is an open question.
- No crate, tag or release is published as part of this RFC.

## Drawbacks

- More moving parts: five documents instead of one, a root-key custody process, and a clock
  dependency.
- Offline clients can fall out of the timestamp window and be unable to update automatically.
- Root-key loss is still a distribution problem; a threshold helps only if the keys are actually
  kept separate.
- The Windows ACL path is compile-verified only; shipping a claim before runtime tests would be
  dishonest.
- A full TUF implementation is larger than this subset. The subset has to be reviewed for the
  attacks it does not cover (delegations, consistent snapshots, extraneous dependencies).
- Signing and key-management ceremony grows for small projects. The updater stays opt-in.

## Alternatives

- **Full TUF now.** The strongest reference design, but it is a large dependency and operational
  change. It needs a separate RFC, a reviewed implementation and a migration plan. Rejected for the
  first alpha; retained as a future option.
- **Single key plus expiry only.** Cheapest, but a compromised key still signs fresh metadata until
  the app is replaced. It does not solve rotation or revocation.
- **Code signing / OS trust only.** Sparkle and Squirrel lean on code signing and TLS. That helps
  installation, but it does not give the client a signed freshness statement or a generation floor.
  It is not a substitute for metadata verification.
- **Do nothing.** Keep the current single-key, no-expiry model. Simple and already implemented, but
  the freeze/replay/rotation gaps remain and the updater cannot be described as production-ready.
- **Adopt Sparkle/Tauri-style single-key appcast.** Lower ceremony than TUF, but neither documents
  a full rotation/revocation/freshness model; the RFC's threat list would remain partially open.

## Comparison

| System | Signed metadata | Freshness/expiry | Rollback/generation | Key rotation/revocation | Notes |
|---|---|---|---|---|---|
| **TUF** | Root, targets, snapshot, timestamp; thresholds | Every role expires; timestamp is short-lived | Snapshot versions and hashes; rollback and freeze are named attacks | Root can replace/revoke keys; threshold trust | The reference design; full adoption is larger than this RFC |
| **Sparkle 2** | EdDSA artifact signatures; optional signed feed | `pubDate`; `SUSignedFeedFailureExpirationInterval` (default 20 days) is a failure recovery, not a freshness proof | Version comparison in the appcast; no persisted generation floor | Single `SUPublicEDKey` pinned in the app; key rollover needs a new app | Excellent macOS installer UX; not a TUF-style repository |
| **Tauri updater** | Per-artifact signature with a pinned `pubkey`; HTTPS | `pub_date` is informational; no expiry check documented | Internal version check, overridable for rollback; no persisted generation | Single pinned key; docs warn that losing it blocks updates | Close to Auro's current model |
| **Squirrel.Windows** | `RELEASES` file with SHA-1; optional Authenticode | No signed expiry | Version/package selection; server-driven | Code-signing certificate; no metadata key rotation | SHA-1 is not a modern manifest signature |
| **Squirrel.Mac** | Server-driven JSON; code signing | No signed expiry | Server can drive rollback | Code signing; no metadata key rotation | Trust is mostly TLS + code signing |
| **Auro today** | One Ed25519 manifest signature | `published_at` only | Semantic version check; no persisted generation | One pinned key; no rotation/revocation | Specs 0012/0013; journal path now validated |

Sources: [TUF security](https://theupdateframework.io/docs/security/) and
[TUF specification](https://theupdateframework.github.io/specification/latest/);
[Sparkle publishing](https://sparkle-project.org/documentation/publishing/) and
[Sparkle customization](https://sparkle-project.org/documentation/customization/);
[Tauri updater](https://tauri.app/plugin/updater/);
[Squirrel.Windows update process](https://github.com/Squirrel/Squirrel.Windows/blob/master/docs/using/update-process.md)
and [application signing](https://github.com/Squirrel/Squirrel.Windows/blob/master/docs/using/application-signing.md);
[Squirrel.Mac](https://github.com/Squirrel/Squirrel.Mac).

## Unresolved questions

- Threshold and custody for the root key: 1-of-1? 2-of-3? Who holds the offline shares?
- Is a short-lived timestamp role worth an online signing service for a pre-alpha project, or should
  the first step be expiry + generation on the existing manifest?
- Which clock-skew window and maximum expiry are acceptable? These need a policy decision and tests,
  not an invented number.
- Can the Windows journal/staging validation be runtime-tested in CI? The current change is
  compile-verified only.
- Can the installer handoff consume a held descriptor on MSI/NSIS and macOS, or is the residual
  TOCTOU unavoidable?
- How does staged rollout interact with generation and `min_version`? A withheld rollout must not
  look like a freeze.
- Do we adopt a reviewed TUF implementation later, and if so, which one? Do not hand-roll
  threshold cryptography.

## Future possibilities

- A transparency log or build attestations for released artifacts.
- Mirror diversity and per-mirror health, without weakening the signed metadata.
- Reproducible builds and a signed SBOM, once the release process exists.
- A user-visible "last successful update check" and "metadata expired" state, so a freeze is
  diagnosable rather than silent.
