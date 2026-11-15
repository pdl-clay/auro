// SPDX-License-Identifier: MIT OR Apache-2.0

use std::future::Future;
use std::sync::LazyLock;

use tokio::runtime::{Builder, Handle, Runtime};

/// The process-wide tokio runtime.
///
/// GPUI polls its own executor on the main thread and installs no tokio reactor, so every command
/// body is moved here before it runs. The runtime is created lazily: tests and first-party call
/// sites work without an explicit `App::run`.
pub fn runtime() -> &'static Runtime {
    static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
        Builder::new_multi_thread()
            .enable_all()
            .thread_name("auro-worker")
            .build()
            .expect("failed to create the Auro runtime")
    });
    &RUNTIME
}

pub fn handle() -> Handle {
    runtime().handle().clone()
}

/// Runs a future on the Auro runtime and awaits its result.
pub async fn run<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match handle().spawn(future).await {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// Runs a blocking body on the runtime's blocking pool and awaits its result.
pub async fn run_blocking<T, F>(body: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match handle().spawn_blocking(body).await {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}
