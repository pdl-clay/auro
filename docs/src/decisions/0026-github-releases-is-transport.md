# 0026 - GitHub Releases is transport, not a trust anchor

**Status:** Accepted (2026-11-24)

## Context

An installed Auro app needs to discover a new release before it can use the signed updater. GitHub
Releases is the operator's existing publication point and needs no new service. The release API is
unauthenticated for public repositories and is controlled by the repository owner, not by the app's
release key; a compromised repository or a hijacked asset can serve arbitrary JSON. Specs 0012 and
0013 already make the Ed25519 manifest the authenticity boundary, and RFC 0001 records that key
rotation, revocation and metadata expiry are unresolved.

## Decision

A first-party GitHub Releases channel may discover a release and fetch one named signed manifest,
but it never treats the API response as authenticity. The app pins one public key in `Auro.toml`;
`Updater::check` verifies the manifest before the artifact URL is used. Fetch targets from the API
are limited to HTTPS GitHub hosts, and the app sends no credentials. Scheduling and the install
target stay with the app; the channel only records the last check time and hands staged bytes to
the existing journal and restart helper.

## Consequences

- Apps no longer hand-write release discovery, manifest fetch and checker wiring; the AppImage path
  is a complete opt-in slice.
- A second trust model does not enter the updater: a wrong-key, tampered or replayed manifest still
  fails in `Updater`, and a downgrade still needs an explicit rollback path.
- Private releases, key rotation, revocation and freshness need later decisions. They are not
  simulated by reading release metadata.
- Future providers reuse the same `Updater` boundary; they do not inherit GitHub-specific trust.
