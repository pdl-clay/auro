// SPDX-License-Identifier: MIT OR Apache-2.0

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Block, FnArg, ItemFn, LitStr, Pat, Type};

pub fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut name = None;
    let mut rename_all = RenameAll::Snake;

    if !attr.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("rename_all") {
                let value = meta.value()?.parse::<LitStr>()?.value();
                rename_all = RenameAll::parse(&value)?;
                Ok(())
            } else {
                Err(meta.error("unknown command option; expected `name` or `rename_all`"))
            }
        });
        syn::parse::Parser::parse2(parser, attr)?;
    }

    let mut item: ItemFn = syn::parse2(item)?;
    let ident = item.sig.ident.clone();
    let command_name = name.unwrap_or_else(|| ident.to_string());
    let is_async = item.sig.asyncness.is_some();

    // Move the body onto the Auro runtime, whatever executes the function.
    let body: Block = (*item.block).clone();
    let wrapped: Block = if is_async {
        syn::parse_quote!({
            ::auro::ipc::runtime::run(async move #body).await
        })
    } else {
        item.sig.asyncness = Some(syn::Token![async](Span::call_site()));
        syn::parse_quote!({
            ::auro::ipc::runtime::run_blocking(move || #body).await
        })
    };
    *item.block = wrapped;

    let mut extracts = Vec::new();
    let mut call_args = Vec::new();

    for input in &item.sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "an Auro command is a free function: it takes no `self`",
            ));
        };
        let Pat::Ident(pat_ident) = &*pat_type.pat else {
            return Err(syn::Error::new_spanned(
                &pat_type.pat,
                "command arguments must be named bindings",
            ));
        };

        let arg = pat_ident.ident.clone();
        let ty = &*pat_type.ty;
        call_args.push(arg.clone());

        if let Some(inner) = state_inner(ty) {
            extracts.push(quote! {
                let #arg = match invoke.state::<#inner>() {
                    Ok(state) => state,
                    Err(error) => return ::auro::ipc::Response::from_error(&error),
                };
            });
        } else {
            let key = rename_all.apply(&arg.to_string());
            extracts.push(quote! {
                let #arg: #ty = match invoke.arg::<#ty>(#key) {
                    Ok(value) => value,
                    Err(error) => return ::auro::ipc::Response::from_error(&error),
                };
            });
        }
    }

    let wrapper = format_ident!("__cmd_{}", ident);
    let name_const = format_ident!("__AURO_CMD_NAME_{}", ident);

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        pub const #name_const: &str = #command_name;

        #[doc(hidden)]
        pub fn #wrapper(invoke: ::auro::ipc::Invoke) -> ::auro::ipc::ResponseFuture {
            Box::pin(async move {
                #(#extracts)*
                match #ident(#(#call_args),*).await {
                    Ok(value) => ::auro::ipc::Response::ok(value),
                    Err(error) => ::auro::ipc::Response::from_error(&error),
                }
            })
        }
    })
}

/// `State<T>` in an argument position is resolved from the app, not from the payload.
fn state_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "State" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

enum RenameAll {
    Snake,
    Camel,
}

impl RenameAll {
    fn parse(value: &str) -> syn::Result<Self> {
        match value {
            "snake_case" => Ok(RenameAll::Snake),
            "camelCase" => Ok(RenameAll::Camel),
            other => Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "unknown `rename_all` value `{other}`; expected `snake_case` or `camelCase`"
                ),
            )),
        }
    }

    fn apply(&self, name: &str) -> String {
        match self {
            RenameAll::Snake => name.to_owned(),
            RenameAll::Camel => {
                let mut out = String::with_capacity(name.len());
                let mut upper = false;
                for character in name.chars() {
                    if character == '_' {
                        upper = true;
                    } else if upper {
                        out.extend(character.to_uppercase());
                        upper = false;
                    } else {
                        out.push(character);
                    }
                }
                out
            }
        }
    }
}
