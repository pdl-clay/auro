// SPDX-License-Identifier: MIT OR Apache-2.0

//! Panic containment at the command boundary.

use std::any::Any;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::task::Poll;

use crate::error::AuroError;
use crate::response::Response;

/// Awaits a command response, turning a panic into an error response.
///
/// The boundary is used when the caller is not Rust code, so a panic in an app or plugin command
/// must not unwind into the dispatcher. Panics raised while polling the wrapped future (an `async`
/// body or [`crate::runtime::run_blocking`] resuming a blocking-pool panic) become
/// [`AuroError::Command`] with kind `command`; the app's event loop and other commands stay usable.
pub async fn catch_command_panic<F>(name: &str, future: F) -> Response
where
    F: Future<Output = Response>,
{
    let mut future = pin!(future);
    poll_fn(
        move |cx| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Ready(response)) => Poll::Ready(response),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Response::from_error(&AuroError::Command {
                name: name.to_owned(),
                message: panic_message(payload),
            })),
        },
    )
    .await
}

fn panic_message(payload: Box<dyn Any + Send + 'static>) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match detail {
        Some(detail) if !detail.is_empty() => format!("the command body panicked: {detail}"),
        _ => "the command body panicked".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_a_command_error() {
        let response = crate::runtime::runtime().block_on(catch_command_panic("boom", async {
            panic!("kaboom");
            #[allow(unreachable_code)]
            Response::ok(())
        }));
        assert!(matches!(
            response,
            Response::Err { ref kind, ref message }
                if kind == "command" && message.contains("panicked") && message.contains("kaboom")
        ));
    }

    #[test]
    fn a_successful_response_passes_through() {
        let response = crate::runtime::runtime()
            .block_on(catch_command_panic("ok", async { Response::ok(7) }));
        assert!(response.is_ok());
    }
}
