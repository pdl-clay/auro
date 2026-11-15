// SPDX-License-Identifier: MIT OR Apache-2.0

use proc_macro::TokenStream;

mod command;
mod handler;

/// Marks a function as an Auro command.
///
/// The body always runs on the Auro runtime, never on the caller's executor: GPUI polls its own
/// executor on the main thread and installs no tokio reactor.
///
/// Options: `name = "..."`, `rename_all = "snake_case" | "camelCase"` (default `snake_case`).
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    command::expand(attr.into(), item.into())
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}

/// Builds a [`CommandHandler`] from a list of command paths.
///
/// Paths are required when the command lives in another module (`services::save_document`); a bare
/// name works in the module where the command is defined.
#[proc_macro]
pub fn generate_handler(input: TokenStream) -> TokenStream {
    handler::expand(input.into())
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}
