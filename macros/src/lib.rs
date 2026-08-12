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
    let mut invocation = String::from("impl_handler!(stream,state;");

    for index in 0..arity {
        let state = state_type(index);
        let input = if index == 0 { "stream" } else { "state" };

        write!(invocation, "(A{index},M{index},a{index},{state},{input}),")
            .map_err(|error| error.to_string())?;
    }

    invocation.push_str(");");

    Ok(invocation)
}

fn state_type(index: usize) -> String {
    let mut state = String::from("Box<dyn RequestStream>");

    for previous in 0..index {
        state = format!("<A{previous} as ResolveRequest<M{previous},{state}>>::Next");
    }

    state
}

fn compile_error(message: &str) -> TokenStream {
    TokenStream::from_str(&format!("compile_error!({message:?});")).unwrap_or_default()
}
