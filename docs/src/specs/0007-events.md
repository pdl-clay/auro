# 0007 - Typed application events

**Status:** Implemented (2026-11-17)

Phase 1 needs app-wide notifications that cross the command boundary without strings. This spec
fixes the event contract; [ADR 0010](../decisions/0010-typed-events-main-thread.md) records why the
channel and main-thread pump were chosen over a string bus, per-entity events or worker-thread
callbacks.

## Scope

- `EventSender`: a cloneable, `Send + Sync` producer handle in `auro-core`.
- The `EventSender` is automatically managed in the state store.
- `App::on_event::<E>` for main-thread handlers, registered before the app runs.
- `emit(event, cx)` for the main-thread path.
- The facade pump that delivers background events on the GPUI main thread.

## Out of scope

- Dynamic subscribe/unsubscribe after startup.
- Backpressure, coalescing, persistence or replay.
- String topics and untyped payloads.
- Cross-process/plugin events (Phase 3), which need serialization and a schema.
- A total ordering across producers.

## Decisions

### D1 - Events are Rust types

An event is any `E: Send + 'static`. It is keyed by `TypeId`; two different types with the same
shape are different events. No string names exist.

### D2 - Two producer paths

- **Main thread:** `emit(event, cx)` dispatches synchronously to the registered handlers. Use it
  from a view or lifecycle handler.
- **Any thread:** `EventSender::emit(event)` boxes the event, queues it, and returns `false` when
  the app is gone. The sender is available as `State<EventSender>` in commands and as a clone for
  whatever else the app manages.

The state store always contains one `EventSender`, inserted by `App::new`. A command can request
`State<EventSender>` without extra setup.

### D3 - Delivery is main-thread and ordered per producer

The facade installs a global `EventBus` and a foreground pump at `App::run`. The pump awaits the
unbounded channel and dispatches each event through `AsyncApp::update`, so handlers receive
`&mut gpui::App` and may update entities. Queued events are FIFO per producer. A main-thread
`emit` can interleave with the pump; there is no global order.

### D4 - Handlers

`App::on_event::<E>(|event, cx| ...)` registers a handler before `run`. Handlers run in
registration order, are `Fn` (called once per event), and must not block. A panic propagates. The
handler list is cloned before dispatch, so re-entrant emission (including a handler that closes a
window) is safe.

Events with no subscriber are dropped. An event for a type with no handler is not an error.

### D5 - No backpressure in this slice

The channel is unbounded. A producer that outruns the main thread can grow memory. Apps that emit
from a hot loop must rate-limit themselves. A bounded policy is a later decision.

## Interfaces

```rust,ignore
use auro::prelude::*;

#[derive(Clone, Debug)]
struct ImportFinished { rows: usize }

#[auro::command]
async fn import(path: String, events: State<EventSender>) -> Result<usize, AuroError> {
    let rows = 42;
    let _ = events.emit(ImportFinished { rows });
    Ok(rows)
}

fn main() {
    App::new()
        .on_event(|event: &ImportFinished, cx| {
            // update an entity; cx.notify() schedules the repaint
        })
        .root::<LogViewer>("main")
        .run();
}

// main-thread path, e.g. in a view or lifecycle handler
emit(ImportFinished { rows: 1 }, cx);
```

If the app needs a different queue policy, it can wrap `EventSender` in its own managed type; the
framework channel remains one option.

## Acceptance

1. A main-thread `emit` reaches the handler for its type and not the handler for another type
   (`app::tests::delivers_typed_events_and_keeps_types_isolated`).
2. A background `EventSender::emit` queues an event; the receiver yields it and dispatch calls the
   handlers in registration order
   (`app::tests::queues_background_events_for_the_main_thread_pump`).
3. `App::new` manages an `EventSender` in the state store; the test obtains it with
   `State<EventSender>`.
4. An event with no subscriber is dropped without panic (covered by type isolation emitting an
   unhandled type).
5. A real app emits from a command and updates the view from the handler; recorded in the pull
   request with the run environment. The pump path is otherwise covered by the queue/dispatch
   test.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Backpressure/coalescing for hot producers | Later event-policy spec |
| Dynamic subscriptions with a handle/token | When a real app needs runtime subscribe |
| Cross-process/plugin event serialization and schema | Phase 3 plugin spec |
| Whether lifecycle events should also flow through `on_event` | Keep separate: lifecycle is a closed enum with a different ordering contract |
| Event ordering across producers | App's responsibility; revisit if a framework primitive needs it |
