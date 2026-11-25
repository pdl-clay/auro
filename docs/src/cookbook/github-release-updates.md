# GitHub release updates

This recipe wires an installed AppImage to a public GitHub Release using Auro's signed updater. It
uses the release-side CLI already in `auro` and the opt-in `GithubChannel` in the runtime. The
complete app lives in `examples/update-channel/`.

## Trust boundary

GitHub Releases discovers the artifact; it does not authenticate it. The app pins the Ed25519
public key in `Auro.toml`, and `Updater` verifies the signed manifest and the artifact's SHA-256
before anything is staged. A wrong-key or tampered manifest is rejected before the artifact URL is
used. See [ADR 0026](../decisions/0026-github-releases-is-transport.md).

## App configuration

```toml
[app]
identifier = "dev.pdl.auri"
product-name = "Auri"
version = "1.2.3"

[[app.windows]]
label = "main"

[updater]
provider = "github"
repository = "pdl-clay/auri"
public-key = "<base64 key printed by auro generate-key>"
# manifest-asset = "auro-update.json"   # default
# interval-secs = 3600                  # default
# check-on-start = true                 # default
```

The install target is not a config key: an installed AppImage has a host-specific path, normally
available as `$APPIMAGE`. The app supplies it to `apply_if_due` at runtime.

Enable the facade feature:

```toml
[dependencies]
auro = { version = "=0.1.0-alpha.14", features = ["updater"] }
```

App flow (see the runnable example for the full code):

```rust,ignore
let config = Config::load(&std::env::current_dir()?)?;
let Some(settings) = config.updater.as_ref() else { return Ok(false) };
let channel = GithubChannel::new(
    ChannelConfig::new(&settings.repository, &settings.public_key)
        .with_manifest_asset(&settings.manifest_asset)
        .with_interval_secs(settings.interval_secs)
        .with_check_on_start(settings.check_on_start),
    std::env::current_dir()?.join(".auro-updates"),
    &config.app.version,
)?;
let target = std::env::var_os("APPIMAGE")
    .map(PathBuf::from)
    .unwrap_or(std::env::current_exe()?);
Ok(channel.apply_if_due(target, None)?)
```

After `apply_if_due` returns `true`, the app must exit. The restart helper waits for that exit,
verifies the staged artifact again, replaces the AppImage, marks the journal, and relaunches from
`$APPIMAGE`. On the next start, call `ApplyJournal::load(...).reconcile(&version)` and `finalize`
once `Recovery::Complete` is returned; the example shows that path too.

To use a delta, keep the currently installed artifact as `base` and pass it instead of `None`.
The channel applies the signed delta when the manifest has one and falls back to a full download
otherwise.

## Release recipe

1. Generate and keep the signing key outside the repository:

   ```text
   auro generate-key --out release.key
   ```

   Pin the printed public key in `Auro.toml`. Never ship `release.key`.

2. Build the AppImage and publish it on a **public** GitHub Release. Copy the browser download URL
   for the asset (for example
   `https://github.com/pdl-clay/auri/releases/download/v1.2.3/Auri-x86_64.AppImage`).

3. Create the signed manifest with the platform key of the target and a stable publication time:

   ```text
   auro manifest \
     --artifact target/release/bundle/appimage/Auri-x86_64.AppImage \
     --version 1.2.3 \
     --url https://github.com/pdl-clay/auri/releases/download/v1.2.3/Auri-x86_64.AppImage \
     --key release.key \
     --out auro-update.json \
     --published-at 2026-11-24T00:00:00Z \
     --platform linux-x86_64
   ```

4. Optional delta. Upload `patch.bin` to the same release, then add the delta fields to the
   manifest command:

   ```text
   auro delta --from old.AppImage --to new.AppImage --out patch.bin
   auro manifest ... \
     --delta-from old.AppImage \
     --delta-from-version 1.2.2 \
     --delta-url https://github.com/pdl-clay/auri/releases/download/v1.2.3/patch.bin \
     --delta-out patch.bin
   ```

   `auro manifest` hashes the patch and signs the delta metadata. The delta is base-bound: only an
   install running `1.2.2` with that exact base hash can use it.

5. Upload the manifest as a named release asset and publish:

   ```text
   gh release upload v1.2.3 auro-update.json --clobber
   gh release edit v1.2.3 --draft=false
   ```

   The file name must match `[updater].manifest-asset`. `gh` reads its token from the operator's
   environment; the app never receives a token.

## Limits

- Public releases only. Private releases need an authenticated API, which is deliberately absent.
- One manifest asset per release. Channels, beta tracks and metadata expiry are later work.
- AppImage apply is first. Windows and macOS manifests can be published, but `GithubChannel::apply`
  only prepares the AppImage journal.
- The signed manifest's artifact URL is not restricted to GitHub; the asset URL from the API is.
  A release operator can point the artifact at a CDN, and the SHA-256 from the signed manifest
  still decides whether the bytes are accepted.
- The state directory holds the journal and must be private to the user. Do not put it under a
  world-writable path.

## Verification

- `cargo test -p auro-update` covers release discovery, signature rejection, asset URL restrictions,
  a 1 MiB manifest bound, check intervals, delta staging and AppImage journal preparation against a
  local fake API.
- `cargo test -p auro-core` covers parsing, defaults and validation of `[updater]`.
- `cargo check --manifest-path examples/update-channel/Cargo.toml` compiles the documented app flow.
