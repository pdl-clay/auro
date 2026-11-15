# 0004 - Main thread belongs to GPUI

**Status:** Accepted

## Context

GPUI runs a single application loop and expects UI mutation on that thread. At the same time, an
application framework must run file I/O, network requests, subprocesses and parsing - none of which
belong in a render function.

The two requirements conflict unless there is exactly one rule about who may touch what.

## Decision

**GPUI owns the main thread; all UI mutation happens on it; all blocking work happens on the
`tokio` runtime; `auro-ipc` is the boundary between the two.**

## Consequences

**Easier**

- One rule to review against: if it blocks, it is not on the main thread.
- State changes have a single ordering, which removes a class of teardown crashes in hot reload
  (a background task cannot outlive the entity it mutates).
- Async commands are ordinary `async fn`s, testable without a renderer.

**Harder**

- Every cross-thread result needs an explicit hop back (`cx.update` / `AsyncApp::update`), which is
  more ceremony than a shared `Mutex<State>`.
- Commands that are actually CPU-cheap pay a spawn for no reason; handlers must choose between
  direct mutation and dispatch.
- Borrow rules: an entity borrow cannot be held across an `await`, so data must be cloned out
  first.

**Forbidden**

- Blocking I/O, `blocking` HTTP clients, `Command::output` or `sleep` on the main thread.
- Mutating an `Entity` from a worker thread.
- A `static` cache of entity handles, which becomes a use-after-free when the UI `cdylib` is
  reloaded.
