# 0010 - Typed events, delivered on the main thread

**Status:** Accepted (2026-11-17)

## Context

Phase 1 needs app-wide events: a command finishes an import, a background task makes progress, a
view wants to announce something to other views. The project forbids stringly-typed dispatch
(ADR 0002) and requires UI mutation on the GPUI main thread (ADR 0004). Commands run on Auro's
tokio runtime, so a background producer cannot touch `App`, entities or windows directly.

Options considered:

1. A string topic bus (Tauri's `emit`/`listen` shape). Rejected: a renamed event becomes a silent
   runtime no-op, which is the failure mode Auro exists to remove.
2. GPUI's per-entity `EventEmitter` only. Useful for parent/child wiring, but a command has no
   entity and app-wide delivery would still need a fan-out mechanism.
3. A typed event queue keyed by `TypeId`, with a main-thread pump. Chosen.
4. Synchronous callbacks from the worker thread. Rejected: they would let background code mutate
   UI state and violate the threading contract.

## Decision

`auro-core` owns a renderer-agnostic channel. Any `E: Send + 'static` can be emitted.

- `EventSender` is cloneable, `Send + Sync`, and auto-managed in the state store, so a command can
  take `State<EventSender>` and call `sender.emit(event)`. It boxes the event with its `TypeId` and
  queues it on an unbounded `tokio::sync::mpsc` channel. `emit` returns `false` when the app is
  gone.
- The facade installs an `EventBus` global and a foreground pump. The pump awaits the receiver and
  dispatches each event on the main thread through `AsyncApp::update`.
- `App::on_event::<E>(|event, cx| ...)` registers a handler before the app runs. Handlers are
  keyed by `TypeId`, run in registration order, and receive `&mut App` so they can update entities
  and call `cx.notify()`. A panic propagates.
- `emit(event, cx)` is the main-thread path and dispatches synchronously. Background producers must
  use `EventSender`.
- An event with no subscriber is dropped; there is no error and no fallback to a string topic.
- Handler lists are cloned before dispatch, so a handler can emit (or cause a close/lifecycle
  event) without tripping a `RefCell` borrow.

Ordering is FIFO per producer through the queue. A main-thread `emit` can interleave with queued
events; there is no total order across producers. The queue is unbounded and has no backpressure in
this slice.

## Consequences

**Easier**

- Commands and views share one typed event path with no string names.
- Events reach the UI on the correct thread without every app inventing its own channel.
- The event type is a Rust type: renames and payload changes are compile errors.

**Harder**

- A flooding producer can grow memory because the channel is unbounded; a bounded/backpressure
  policy is deferred.
- Subscriptions are builder-time only; dynamic subscribe/unsubscribe needs a subscription handle
  and a lifetime story.
- Cross-producer ordering is not guaranteed; apps that need it must sequence their own work.
- Events are not persisted; a late subscriber misses earlier events.

**Forbidden**

- String event names, untyped payloads or worker-thread UI mutation.
- Treating "no subscriber" as an error; a dropped event is the documented behaviour.

## Revisit when

- A real app needs backpressure, event coalescing or ordering guarantees.
- Dynamic subscriptions become common enough to justify a handle API.
- Events cross the process/plugin boundary (Phase 3), where serialization and schema matter.
