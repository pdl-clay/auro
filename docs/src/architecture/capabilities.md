# Capabilities

Auro does not copy Tauri's capability model, because most of what Tauri checks at runtime does not
exist here. When the frontend is your own Rust code, a call to `std::fs::write` is not a privilege
escalation - it is your code, compiled in.

What remains is a real security boundary:

| Boundary | Why it is untrusted | Enforced at |
|---|---|---|
| **Plugins** | third-party code, may be installed after the app ships | plugin host, per call |
| **Content** | documents opened, scripts evaluated, remote data | the API that consumes it |
| **Webview escape hatch** | a browser pointed at content we do not control | the hatch itself, phase 4 |

## Model

- **Deny by default.** A plugin manifest lists the scopes it needs: `fs`, `net`, `shell`,
  `clipboard`, `notification`, `keychain`. Anything unlisted is refused at the call site.
- **Scopes are data, not strings in code.** The app ships a manifest; the host compiles it and
  answers verdicts. A path or host pattern is matched against the compiled form, not re-parsed per
  call.
- **The check sits next to the effect.** `auro-perms` returns a verdict; the code that opens the
  file or spawns the process is the code that asks. A single choke point far from the effect is
  how permission models rot.
- **Compile-time first.** If the compiler can prevent it, a runtime check is not the answer. Do not
  add a capability for something first-party code does directly.
- **In-process plugins are a feature flag.** A `cdylib` shares the address space, so a bug there
  can corrupt the host. The default is out-of-process; in-process is opt-in and documented as a
  trade, not as a security boundary.

## What a denial looks like

A denial is a typed error returned to the caller, not a panic and not a silently ignored call:

```rust,ignore
// Planned shape (capability enforcement not implemented yet).
let file = perms.fs_read(&plugin_id, path).await?; // Err(CapabilityDenied { scope, subject })
```

The error names the scope, the subject and the pattern that failed to match, so a plugin author
can fix the manifest without reading framework source.

## Not in scope

- Sandboxing first-party app code. If the app author writes it, it runs.
- OS-level sandboxing (App Sandbox, seccomp, AppContainer). Bundle-level sandboxing is a separate
  discussion, tracked against Phase 2 packaging work.
- Protecting secrets from the user who owns the machine. Keychain access is about correctness and
  consent, not about defending against the local user.

## Open questions

- Manifest format: `Auro.toml` section, or a separate `capabilities.json` next to the plugin.
- Whether capability grants are app-wide or per-window for the webview escape hatch.
- How a plugin upgrade that widens scopes is presented to the user at runtime.
