// SPDX-License-Identifier: MIT OR Apache-2.0

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Path, Token};

pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let paths = Punctuated::<Path, Token![,]>::parse_terminated.parse2(input)?;

    if paths.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`generate_handler!` needs at least one command",
        ));
    }

    let mut entries = Vec::new();

    for path in paths {
        let mut wrapper = path.clone();
        let mut schema_path = path.clone();
        let mut name_path = path;

        let ident = {
            let segment = wrapper
                .segments
                .last_mut()
                .expect("a parsed path has at least one segment");
            let ident = segment.ident.clone();
            segment.ident = format_ident!("__cmd_{}", ident);
            ident
        };

        name_path
            .segments
            .last_mut()
            .expect("a parsed path has at least one segment")
            .ident = format_ident!("__AURO_CMD_NAME_{}", ident);

        schema_path
            .segments
            .last_mut()
            .expect("a parsed path has at least one segment")
            .ident = format_ident!("__schema_{}", ident);

        entries.push(quote! {
            {
                let (__input, __output) = #schema_path();
                __handler = __handler.command_with_schema(
                    #name_path,
                    #wrapper,
                    __input,
                    __output,
                );
            }
        });
    }

    Ok(quote! {{
        let mut __handler = ::auro::ipc::CommandHandler::new();
        #(#entries)*
        __handler
    }})
}
