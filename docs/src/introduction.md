<section class="hero">
    <div>
        <p class="hero__eyebrow">Auro 0.1.0-alpha.2 &middot; native desktop</p>
        <h1>Desktop apps with a Rust frontend.</h1>
        <p class="hero__lede">
            Auro keeps Tauri's app model - configuration, windows, capabilities, bundling,
            signed updates - and replaces the webview with GPUI, Zed's GPU-accelerated Rust
            UI toolkit. One Rust codebase, from window to command.
        </p>
        <div class="hero__actions">
            <a class="button button--primary" href="start.html">Start here</a>
            <a class="button" href="architecture/overview.html">See the architecture</a>
        </div>
        <p class="hero__note">
            Pre-alpha. The CLI alpha is public; the framework is not production-ready. Hot
            reload is measured only for the documented data boundary.
            <a href="reference/platform-support.html">Platform status</a>.
        </p>
    </div>
    <aside class="terminal-card" aria-label="Quickstart commands">
        <div class="terminal-card__bar">
            <span class="terminal-card__dot"></span>
            <span class="terminal-card__dot"></span>
            <span class="terminal-card__dot"></span>
            <span class="terminal-card__title">zsh - auro</span>
        </div>
        <div class="terminal-card__body">
<pre><span class="comment"># install the alpha CLI</span>
<span class="prompt">$</span> cargo install auro-cli --version 0.1.0-alpha.2
<span class="prompt">$</span> auro new hello
<span class="prompt">$</span> cd hello
<span class="prompt">$</span> auro dev</pre>
        </div>
        <div class="terminal-card__footer">
            <span>default dev loop: rebuild and relaunch</span>
            <button class="terminal-copy" type="button" data-copy="cargo install auro-cli --version 0.1.0-alpha.2
auro new hello
cd hello
auro dev">Copy</button>
        </div>
    </aside>
</section>

<section class="landing-section">
    <span class="section-kicker">The shape of the work</span>
    <h2>One workspace from install to window.</h2>
    <ol class="steps">
        <li>
            <h3>Scaffold a real app</h3>
            <p>
                <code>auro new</code> writes a four-crate workspace: the bin, the view, the
                hot boundary and the shared data types.
            </p>
        </li>
        <li>
            <h3>Run the native loop</h3>
            <p>
                <code>auro dev</code> builds, launches and watches. The default loop rebuilds
                and restarts; the hot path is opt-in and has a narrower contract.
            </p>
        </li>
        <li>
            <h3>Build and bundle</h3>
            <p>
                <code>auro build</code> makes a release binary. <code>auro bundle</code> uses
                the platform packaging paths; support is documented, not assumed.
            </p>
        </li>
    </ol>
</section>

<section class="landing-section">
    <span class="section-kicker">The boundary</span>
    <h2>A command is a Rust function.</h2>
    <div class="split">
        <div>
            <p class="section-lede">
                The schema is derived from the Rust signature, and the call site is a Rust
                call. A renamed command or a wrong argument is a compile error, not a runtime
                string lookup.
            </p>
            <p>
                Cheap mutation stays on the UI thread. File I/O and network calls run as
                commands on <code>tokio</code>; the result returns to the main thread, updates
                an entity, and the next frame renders it.
            </p>
            <p>
                There is no <code>invoke("name", {...})</code> in first-party code because
                there is no JavaScript bridge to invoke through.
                <a href="architecture/threading-and-ipc.html">Read the threading contract</a>.
            </p>
        </div>
        <div class="code-card">
<pre><code>#[auro::command]
async fn save_document(
    path: PathBuf,
    body: String,
) -&gt; Result&lt;Saved, AuroError&gt; {
    tokio::fs::write(&amp;path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}
// The call site is typed too.
let saved = api::save_document(doc.path, doc.body).await?;</code></pre>
        </div>
    </div>
</section>

<section class="landing-section">
    <span class="section-kicker">What you get</span>
    <h2>The app model, without the browser.</h2>
    <div class="feature-grid">
        <article class="feature">
            <span class="feature__tag">typed IPC</span>
            <h3>No stringly bridge</h3>
            <p>
                Commands, events and arguments are Rust types. The schema is generated from
                the same signature the compiler checks.
            </p>
        </article>
        <article class="feature">
            <span class="feature__tag">renderer</span>
            <h3>GPUI, not a webview</h3>
            <p>
                The app draws through GPUI on the GPU. The webview is a scoped Phase 4 escape
                hatch for OAuth and embedded docs, not the foundation.
            </p>
        </article>
        <article class="feature">
            <span class="feature__tag">capabilities</span>
            <h3>Fence the real boundary</h3>
            <p>
                Grants are deny-by-default and scoped to filesystem, network, shell,
                clipboard, notifications and keychain. Most UI code needs no runtime check.
            </p>
        </article>
        <article class="feature">
            <span class="feature__tag">application layer</span>
            <h3>Keep the boring parts</h3>
            <p>
                Config, window management, lifecycle, single-instance, tray, shortcuts and
                deep links stay in the framework, where Tauri already proved the shape.
            </p>
        </article>
        <article class="feature">
            <span class="feature__tag">release</span>
            <h3>Signed updates, opt-in</h3>
            <p>
                Update manifests are signed, artifacts are hashed and staged, and apply paths
                are platform-specific. The updater is a feature, not a default.
            </p>
        </article>
        <article class="feature">
            <span class="feature__tag">the loop</span>
            <h3>Hot reload, measured honestly</h3>
            <p>
                Edits in the documented data boundary measured 368 ms median on Linux. View,
                layout, type and dependency edits rebuild the host and restart the process.
            </p>
        </article>
    </div>
</section>

<section class="landing-section">
    <span class="section-kicker">Scope</span>
    <h2>What Auro is not.</h2>
    <ul class="non-goals">
        <li>
            <strong>Not a webview wrapper.</strong>
            The webview is a scoped Phase 4 escape hatch for OAuth and embedded docs, not the
            foundation.
        </li>
        <li>
            <strong>Not a GUI toolkit.</strong>
            GPUI owns the widgets and the renderer. Auro is the application layer around it.
        </li>
        <li>
            <strong>Not a GPUI fork.</strong>
            Upstream is pinned, with one thin compatibility module and no GPUI outside
            <code>auro-ui</code>.
        </li>
        <li>
            <strong>Not "CSS for Rust".</strong>
            The UI is a builder API plus design tokens. There is no styling language in 1.0.
        </li>
    </ul>
</section>

<section class="landing-section">
    <span class="section-kicker">Choose a path</span>
    <h2>Build, inspect, or challenge it.</h2>
    <div class="path-grid">
        <a class="path" href="start.html">
            <span class="path__label">01 / build</span>
            <span class="path__title">Start with the alpha</span>
            <span class="path__desc">Install the CLI, create the scaffold, and run the default dev loop.</span>
            <span class="path__cta">Read the start guide</span>
        </a>
        <a class="path" href="architecture/overview.html">
            <span class="path__label">02 / understand</span>
            <span class="path__title">Read the architecture</span>
            <span class="path__desc">Follow a command from the UI thread to tokio and back, and see where the crates stop.</span>
            <span class="path__cta">Open the overview</span>
        </a>
        <a class="path" href="decisions/index.html">
            <span class="path__label">03 / evaluate</span>
            <span class="path__title">Check the reasoning</span>
            <span class="path__desc">Decision records carry the tradeoffs, including the ones that killed a mechanism.</span>
            <span class="path__cta">Browse the decisions</span>
        </a>
    </div>
</section>

<section class="landing-section">
    <div class="alpha-band">
        <div>
            <h2>Alpha means alpha.</h2>
            <p>
                The CLI is published; the framework contract is not stable. Read the limits
                before choosing it for anything that matters.
            </p>
        </div>
        <ul>
            <li><code>0.1.0-alpha.2</code> is on crates.io; there is no stable release.</li>
            <li>Hot reload is measured on Linux for the data boundary; Windows and macOS are above the target.</li>
            <li>Platform rows are published as verified or unverified, not as promises.</li>
        </ul>
    </div>
    <div class="hero__actions">
        <a class="button button--primary" href="start.html">Start here</a>
        <a class="button" href="specs/0024-alpha-release.html">Read the release gates</a>
    </div>
</section>
