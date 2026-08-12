#![forbid(unsafe_code)]
#![allow(async_fn_in_trait)]
#![doc = include_str!("../README.md")]

extern crate self as serverkit;

mod app;
mod body;
mod extract;
mod handler;
mod listener;
#[cfg(not(target_family = "wasm"))]
mod native;
mod request;
mod response;
mod route;
mod router;
pub mod schemaval;
mod stream;

pub use app::App;

pub use body::Body;

pub use extract::{
    Buffered, FromRequest, Header, HeaderError, Input, Mode, Path, PathError, Query, QueryError,
    Streaming, Unused,
};

#[cfg(feature = "json")]
pub use extract::{Json, JsonError};

pub use handler::Handler;

pub use listener::Listener;

pub use request::{Headers, Method, Request};

pub use response::{IntoResponse, Response};

pub use route::{Route, RouteMethods};

pub use schemaval::{
    DecodeOptions, Schema, UnknownFields, ValidationErrors, ValidationIssue, ValidationRule, Value,
    Values,
};

pub use serverkit_macros::Schema;

#[doc(hidden)]
pub use route::Routes;

pub(crate) use router::Router;

pub use stream::{RequestStream, StreamError};

pub mod prelude {
    pub use crate::{
        App, Body, Buffered, DecodeOptions, FromRequest, Handler, Header, HeaderError, Headers,
        Input, IntoResponse, Listener, Method, Mode, Path, PathError, Query, QueryError, Request,
        RequestStream, Response, Route, RouteMethods, Schema, StreamError, Streaming,
        UnknownFields, Unused, ValidationErrors, ValidationIssue, ValidationRule, Value, Values,
    };

    #[cfg(feature = "json")]
    pub use crate::{Json, JsonError};
}
