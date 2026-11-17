// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed application events.
//!
//! The channel is renderer-agnostic: commands and background tasks hold an [`EventSender`], which
//! boxes the event and queues it for the main thread. The facade owns the pump and the typed
//! subscribers, because delivery needs the GPUI app handle.

use std::any::{Any, TypeId};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A cloneable, thread-safe emitter for typed application events.
///
/// `emit` may be called from a command, a background task or the main thread. Delivery happens on
/// the main thread; `false` means the app is gone and the event was not queued.
#[derive(Clone, Debug)]
pub struct EventSender {
    tx: UnboundedSender<QueuedEvent>,
}

impl EventSender {
    pub fn emit<E: Send + 'static>(&self, event: E) -> bool {
        self.tx
            .send(QueuedEvent {
                type_id: TypeId::of::<E>(),
                payload: Box::new(event),
            })
            .is_ok()
    }
}

/// One queued event, consumed by the facade's main-thread pump.
///
/// Public so the facade can move it across the crate boundary; the fields are intentionally
/// opaque to applications.
#[doc(hidden)]
pub struct QueuedEvent {
    type_id: TypeId,
    payload: Box<dyn Any + Send>,
}

impl QueuedEvent {
    #[doc(hidden)]
    pub fn into_parts(self) -> (TypeId, Box<dyn Any + Send>) {
        (self.type_id, self.payload)
    }
}

/// Creates the sender kept in app state and the receiver moved into the main-thread pump.
#[doc(hidden)]
pub fn channel() -> (EventSender, UnboundedReceiver<QueuedEvent>) {
    let (tx, rx) = unbounded_channel();
    (EventSender { tx }, rx)
}
