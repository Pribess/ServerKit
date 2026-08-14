#![forbid(unsafe_code)]
#![allow(async_fn_in_trait)]
#![doc = include_str!("../README.md")]

extern crate self as serverkit;

mod app;
mod body;
#[cfg(all(feature = "worker", target_arch = "wasm32"))]
pub mod cloudflare;
mod cookie;
mod extract;
mod handler;
mod listener;
mod multipart;
#[cfg(not(target_family = "wasm"))]
mod native;
pub mod openapi;
mod redirect;
mod request;
mod response;
mod route;
mod router;
pub mod schemaval;
mod sse;
mod stream;
#[cfg(feature = "websocket")]
mod websocket;

pub use app::App;

pub use body::Body;

pub use cookie::{Cookie, Cookies, SameSite};

pub use extract::{
    Bytes, ConnectInfo, Extension, Form, FormError, FromRequest, Header, HeaderError,
    MissingConnectInfo, MissingExtension, MissingState, Path, PathError, Query, QueryError, State,
    Text, TextError,
};

#[cfg(feature = "json")]
pub use extract::{Json, JsonError};

pub use handler::Handler;

pub use listener::Listener;

pub use multipart::{Multipart, MultipartError, MultipartField};

pub use openapi::{OpenApi, OpenApiDocument};

pub use request::{Headers, InvalidHeader, Method, Request};

pub use redirect::Redirect;

pub use response::{IntoResponse, Response, ResponseBody};

pub use route::{Route, RouteMethods};

pub use schemaval::{
    DecodeOptions, ExtraFields, Schema, SchemaField, SchemaKind, SchemaMetadata, UnknownFields,
    ValidationErrors, ValidationIssue, ValidationRule, Value, ValueSchema, Values,
};

pub use sse::{Sse, SseEvent, SseStream};

pub use serverkit_macros::Schema;

#[doc(hidden)]
pub use route::Routes;

pub(crate) use router::Router;

#[cfg(all(feature = "worker", target_arch = "wasm32"))]
pub(crate) use stream::EmptyRequestStream;
pub use stream::{RequestStream, ResponseStream, StreamError};

#[cfg(feature = "websocket")]
pub use websocket::{
    WebSocket, WebSocketError, WebSocketMessage, WebSocketUpgrade, WebSocketUpgradeError,
};

pub mod prelude {
    pub use crate::{
        App, Body, DecodeOptions, ExtraFields, FromRequest, Handler, Header, HeaderError, Headers,
        IntoResponse, Listener, Method, OpenApi, OpenApiDocument, Path, PathError, Query,
        QueryError, Request, RequestStream, Response, ResponseBody, ResponseStream, Route,
        RouteMethods, Schema, SchemaField, SchemaKind, SchemaMetadata, StreamError, UnknownFields,
        ValidationErrors, ValidationIssue, ValidationRule, Value, ValueSchema, Values,
    };

    pub use crate::{
        Bytes, ConnectInfo, Cookie, Cookies, Extension, Form, FormError, InvalidHeader,
        MissingConnectInfo, MissingExtension, MissingState, Multipart, MultipartError,
        MultipartField, Redirect, SameSite, Sse, SseEvent, SseStream, State, Text, TextError,
    };

    #[cfg(feature = "json")]
    pub use crate::{Json, JsonError};

    #[cfg(feature = "websocket")]
    pub use crate::{
        WebSocket, WebSocketError, WebSocketMessage, WebSocketUpgrade, WebSocketUpgradeError,
    };
}
