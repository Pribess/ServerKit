#![forbid(unsafe_code)]
#![allow(async_fn_in_trait)]
#![doc = include_str!("../README.md")]

extern crate self as serverkit;

mod app;
mod body;
mod cookie;
mod extract;
mod handler;
mod middleware;
mod multipart;
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

pub use app::{Config, Router};

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

pub use middleware::{Middleware, Next};

pub use multipart::{Multipart, MultipartError, MultipartField};

pub use openapi::{
    ApiKeyLocation, ExampleValue, OAuthFlow, OAuthFlows, OpenApi, OpenApiDocument, Operation,
    ParameterLocation, Scalar, ScalarDeveloperTools, SecurityRequirement, SecurityScheme, Server,
};

pub use request::{Headers, InvalidHeader, InvalidMethod, Method, Request};

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

pub(crate) use router::{Dispatch, Dispatcher, Scope};

pub use stream::{Chunk, RequestStream, ResponseStream, StreamError};

#[cfg(feature = "websocket")]
pub use websocket::{
    WebSocket, WebSocketError, WebSocketMessage, WebSocketUpgrade, WebSocketUpgradeError,
};

#[doc(hidden)]
pub mod adapter {
    #[cfg(feature = "websocket")]
    pub use crate::websocket::{WebSocketIo, WebSocketPlan};
}

pub mod prelude {
    pub use crate::{
        ApiKeyLocation, Body, Chunk, Config, DecodeOptions, ExampleValue, ExtraFields, FromRequest,
        Handler, Header, HeaderError, Headers, IntoResponse, Method, Middleware, Next, OAuthFlow,
        OAuthFlows, OpenApi, OpenApiDocument, Operation, ParameterLocation, Path, PathError, Query,
        QueryError, Request, RequestStream, Response, ResponseBody, ResponseStream, Route,
        RouteMethods, Router, Scalar, ScalarDeveloperTools, Schema, SchemaField, SchemaKind,
        SchemaMetadata, SecurityRequirement, SecurityScheme, Server, StreamError, UnknownFields,
        ValidationErrors, ValidationIssue, ValidationRule, Value, ValueSchema, Values,
    };

    pub use crate::{
        Bytes, ConnectInfo, Cookie, Cookies, Extension, Form, FormError, InvalidHeader,
        InvalidMethod, MissingConnectInfo, MissingExtension, MissingState, Multipart,
        MultipartError, MultipartField, Redirect, SameSite, Sse, SseEvent, SseStream, State, Text,
        TextError,
    };

    #[cfg(feature = "json")]
    pub use crate::{Json, JsonError};

    #[cfg(feature = "websocket")]
    pub use crate::{
        WebSocket, WebSocketError, WebSocketMessage, WebSocketUpgrade, WebSocketUpgradeError,
    };
}
