use std::{fmt::Write, str::FromStr};

use proc_macro::{TokenStream, TokenTree};

mod schema;

#[proc_macro]
pub fn impl_handlers(input: TokenStream) -> TokenStream {
    match expand(input) {
        Ok(output) => output,
        Err(message) => compile_error(&message),
    }
}

#[proc_macro]
pub fn impl_routes(input: TokenStream) -> TokenStream {
    match expand_routes(input) {
        Ok(output) => output,
        Err(message) => compile_error(&message),
    }
}

#[proc_macro_derive(Schema, attributes(schema))]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    match schema::expand(input) {
        Ok(output) => output,
        Err(message) => compile_error(&message),
    }
}

fn expand(input: TokenStream) -> Result<TokenStream, String> {
    let maximum = parse_maximum(input)?;
    let mut output = String::new();

    for arity in 1..=maximum {
        output.push_str(&handler_invocation(arity)?);
    }

    TokenStream::from_str(&output).map_err(|error| error.to_string())
}

fn expand_routes(input: TokenStream) -> Result<TokenStream, String> {
    let maximum = parse_maximum(input)?;
    let mut output = String::new();

    for arity in 1..=maximum {
        output.push_str("impl_route_tuple!(");
        for index in 1..=arity {
            if index > 1 {
                output.push(',');
            }
            write!(output, "R{index}").map_err(|error| error.to_string())?;
        }
        output.push_str(");");
    }

    TokenStream::from_str(&output).map_err(|error| error.to_string())
}

fn parse_maximum(input: TokenStream) -> Result<usize, String> {
    let mut tokens = input.into_iter();

    let literal = match tokens.next() {
        Some(TokenTree::Literal(literal)) => literal,
        _ => return Err("expected one positive decimal integer".to_owned()),
    };

    if tokens.next().is_some() {
        return Err("expected one positive decimal integer".to_owned());
    }

    let maximum = literal
        .to_string()
        .parse::<usize>()
        .map_err(|_| "expected one positive decimal integer".to_owned())?;

    if maximum == 0 {
        return Err("handler arity must be greater than zero".to_owned());
    }

    Ok(maximum)
}

fn handler_invocation(arity: usize) -> Result<String, String> {
    let mut invocation = String::from("impl_handler!([");

    for index in 0..arity.saturating_sub(1) {
        if index > 0 {
            invocation.push(',');
        }

        write!(invocation, "(A{index},a{index})").map_err(|error| error.to_string())?;
    }

    let last = arity - 1;
    write!(invocation, "];(A{last},a{last}));").map_err(|error| error.to_string())?;

    Ok(invocation)
}

fn compile_error(message: &str) -> TokenStream {
    TokenStream::from_str(&format!("compile_error!({message:?});")).unwrap_or_default()
}
