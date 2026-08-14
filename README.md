# ServerKit

ServerKit is a portable Rust HTTP application layer with an Ohkami-inspired
routing API. Native HTTP/1 serving is available through `std::net::TcpListener`;
Cloudflare Workers use the same `App`, routes, handlers, and extractors through
the built-in adapter.

## Installation

```toml
[dependencies]
serverkit = "0.1"

# Optional buffered JSON extraction.
serverkit = { version = "0.1", features = ["json"] }
serde = { version = "1", features = ["derive"] }

# Portable WebSocket upgrades on native HTTP/1 and Workers.
serverkit = { version = "0.1", features = ["websocket"] }

# Cloudflare Workers adapter.
serverkit = { version = "0.1", features = ["worker", "websocket"] }
worker = "0.8.5"
```

## Complete native server

The same `Schema` derive decodes and validates path parameters, query
parameters, and headers by name.

```rust,no_run
use std::net::TcpListener;

use serverkit::prelude::*;

#[derive(Schema)]
struct UserPath {
    organization: String,
    id: u64,
}

#[derive(Schema)]
struct UserQuery {
    #[schema(default = 1, minimum = 1)]
    page: u32,
    tag: Vec<String>,
}

#[derive(Schema)]
#[schema(rename_all = "kebab-case")]
struct RequestHeaders {
    authorization: String,
    x_request_id: Option<String>,
}

async fn health() -> &'static str {
    "ok"
}

async fn get_user(
    method: Method,
    Path(path): Path<UserPath>,
    Query(query): Query<UserQuery>,
    Header(headers): Header<RequestHeaders>,
) -> String {
    format!(
        "{} {}:{} page={} tags={} auth={}",
        method.as_str(),
        path.organization,
        path.id,
        query.page,
        query.tag.len(),
        headers.authorization,
    )
}

fn main() -> std::io::Result<()> {
    let application = App::new((
        "/health".GET(health),
        "/:organization/users/:id".GET(get_user),
    ));

    application.run(TcpListener::bind("127.0.0.1:3000")?)
}
```

`App::new` accepts one route or a convenience tuple. `.route()` can then be
called any number of times, so the number of routes in an application is not
bounded by tuple arity. Handler functions may have zero through twelve
extractor arguments. Metadata and buffered extractors may appear in any order.
A streaming extractor such as `Body` or `Multipart`, when present, must be the
final argument.

```rust
use serverkit::{App, RouteMethods};

async fn health() -> &'static str { "ok" }
async fn metrics() -> &'static str { "metrics" }

let application = App::new("/health".GET(health))
    .route("/metrics".GET(metrics));
```

## HTTP methods

Routes support `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, and `OPTIONS`.
The same path can register a different handler for each method.

```rust
use serverkit::{App, RouteMethods};

async fn read() -> &'static str {
    "read"
}

async fn create() -> &'static str {
    "created"
}

fn application() -> App {
    App::new(("/items".GET(read), "/items".POST(create)))
}
```

An unsupported method on a matching path returns `405 Method Not Allowed` with
an `Allow` header. If no explicit `HEAD` route exists, ServerKit executes the
matching `GET` handler, preserves its status and representation headers, and
removes the body. If no explicit `OPTIONS` route exists, ServerKit generates a
`204 No Content` response with `Allow`. Static routes retain precedence over
parameter routes before method selection.

## Path extraction

Parameters can occur at any path segment, and multiple parameters are matched
by name rather than struct-field order.

```rust
use serverkit::prelude::*;

#[derive(Schema)]
struct ItemPath {
    id: u64,
}

async fn item(Path(path): Path<ItemPath>) -> String {
    path.id.to_string()
}

fn application() -> App {
    App::new(("/asdf/:id/asdd".GET(item),))
}
```

A scalar schema is a convenience for routes containing exactly one parameter.

```rust
use serverkit::prelude::*;

async fn gpu(Path(id): Path<u64>) -> String {
    id.to_string()
}

fn application() -> App {
    App::new(("/gpus/:id".GET(gpu),))
}
```

Static routes take precedence over parameter routes. Path values are
percent-decoded before validation.

The final segment can capture the remainder of the path with `*name`:

```rust
use serverkit::prelude::*;

#[derive(Schema)]
struct AssetPath {
    path: String,
}

async fn asset(Path(path): Path<AssetPath>) -> String {
    path.path
}

let application = App::new("/assets/*path".GET(asset));
```

Matching is deterministic from left to right: static segments precede
parameters, and parameters precede wildcards. Equivalent patterns such as
`/users/:id` and `/users/:name` for the same method are rejected when the app
is built. Empty parameter names, duplicate parameter names, non-terminal
wildcards, queries, fragments, duplicate slashes, and trailing slashes are
also rejected.

Applications can be nested under a static prefix and can define a fallback:

```rust
use serverkit::{App, RouteMethods};

async fn users() -> &'static str { "users" }
async fn missing() -> &'static str { "missing" }

let api = App::new("/users".GET(users));
let application = App::new(())
    .nest("/api", api)
    .fallback(missing);
```

## Query extraction

Query schemas ignore undeclared fields by default. Repeated names decode into
`Vec<T>`, optional names decode into `Option<T>`, and defaults apply when a
name is absent.

```rust
use serverkit::prelude::*;

#[derive(Schema)]
struct Search {
    #[schema(rename = "q", min_length = 2, max_length = 64)]
    term: String,
    #[schema(default = 1, minimum = 1, maximum = 100)]
    page: u32,
    tag: Vec<String>,
    exact: Option<bool>,
}

async fn search(Query(search): Query<Search>) -> String {
    format!("{}:{}", search.term, search.page)
}

fn application() -> App {
    App::new(("/search".GET(search),))
}
```

For example, `?q=rust&tag=web&tag=server&debug=true` is valid and `debug` is
ignored. Names and values use form-style percent decoding, including `+` as a
space.

## Header extraction

Headers use the same schema decoder but compare names case-insensitively and
allow undeclared fields. This permits normal protocol headers such as `Host`,
`Accept`, and `User-Agent` while continuing to validate every declared header.

```rust
use serverkit::prelude::*;

#[derive(Schema)]
#[schema(rename_all = "kebab-case")]
struct Authentication {
    authorization: String,
    x_request_id: Option<String>,
}

async fn authenticated(Header(headers): Header<Authentication>) -> String {
    headers.authorization
}

fn application() -> App {
    App::new(("/authenticated".GET(authenticated),))
}
```

`rename_all = "kebab-case"` maps `x_request_id` to `X-Request-Id`. An
individual field can override its input name with `#[schema(rename = "...")]`.

## Unknown fields

The source defaults are:

| Extractor | Default behavior |
| --- | --- |
| `Path<T>` | reject |
| `Query<T>` | ignore |
| `Header<T>` | ignore |

A schema can override its source default without changing the extractor type.

```rust
use serverkit::Schema;

#[derive(Schema)]
#[schema(unknown_fields = "reject")]
struct StrictQuery {
    query: String,
}

#[derive(Schema)]
#[schema(unknown_fields = "ignore")]
struct FlexiblePath {
    id: u64,
}
```

`reject` reports each unmatched name as an `UnknownField` validation issue.
`ignore` accepts and discards unmatched values. To retain them instead, add one
`ExtraFields` rest field:

```rust
use serverkit::{ExtraFields, Query, Schema};

#[derive(Schema)]
struct Search {
    query: String,
    #[schema(rest)]
    extra: ExtraFields,
}

async fn search(Query(search): Query<Search>) -> usize {
    search.extra.get_all("tag").count()
}
```

`ExtraFields` preserves input order and repeated names. `get`, `get_all`, and
`iter` return decoded byte slices; `len` counts entries, including duplicates.
Path and query names remain case-sensitive, while captured header names are
looked up case-insensitively. A rest field cannot be combined with an explicit
`unknown_fields` policy because capture already defines how unmatched values
are handled.

```compile_fail
use serverkit::{ExtraFields, Schema};

#[derive(Schema)]
#[schema(unknown_fields = "ignore")]
struct ConflictingPolicy {
    #[schema(rest)]
    extra: ExtraFields,
}
```

## Schemaval rules

The built-in scalar types are `String`, `Vec<u8>`, `bool`, all standard integer
types, `f32`, and `f64`. Struct fields support:

- required `T` values;
- optional `Option<T>` values;
- repeated `Vec<T>` values (`Vec<u8>` remains a single byte value);
- one `#[schema(rest)] ExtraFields` field;
- `#[schema(default)]` and `#[schema(default = expression)]`;
- `minimum`, `maximum`, `min_length`, and `max_length`;
- field and whole-struct custom validation.
- nested schemas through dotted input names;
- generic schemas and string enums;
- metadata used by the OpenAPI generator.

```rust
use serverkit::{Schema, ValidationIssue};

fn validate_slug(value: &String) -> Result<(), ValidationIssue> {
    value
        .chars()
        .all(|character| character.is_ascii_lowercase() || character == '-')
        .then_some(())
        .ok_or_else(|| ValidationIssue::custom("must be a lowercase slug"))
}

#[derive(Schema)]
struct SlugPath {
    #[schema(validate = validate_slug)]
    slug: String,
}
```

```rust
use serverkit::{Schema, ValidationIssue};

#[derive(Schema)]
#[schema(validate = validate_range)]
struct Range {
    start: u64,
    end: u64,
}

fn validate_range(range: &Range) -> Result<(), ValidationIssue> {
    (range.start <= range.end)
        .then_some(())
        .ok_or_else(|| ValidationIssue::custom("start must not exceed end"))
}
```

Direct `Schema::decode` calls accept `DecodeOptions::reject_unknown()` or
`DecodeOptions::ignore_unknown()`. Extractors start with their source default
and apply `#[schema(unknown_fields = "...")]` when it is present.

Failures are aggregated in `ValidationErrors`. Each `ValidationIssue` exposes
its optional field name, `ValidationRule`, and message. Extractors convert
validation failures into HTTP 400 responses.

Custom value sources can implement `Values` and call the same schema directly.

```rust
use serverkit::{DecodeOptions, Schema, Value, Values};

struct OneValue<'a> {
    name: &'a str,
    value: &'a [u8],
}

impl Values for OneValue<'_> {
    fn len(&self) -> usize {
        1
    }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        (index == 0).then_some(Value {
            name: self.name,
            bytes: self.value,
        })
    }
}

#[derive(Schema)]
struct Identifier {
    id: u64,
}

let values = OneValue {
    name: "id",
    value: b"42",
};
let identifier = Identifier::decode(
    &values,
    DecodeOptions::reject_unknown(),
).unwrap();

assert_eq!(identifier.id, 42);
```

Enums decode from their external string representation. All common rename
rules are supported: `lowercase`, `UPPERCASE`, `camelCase`, `PascalCase`,
`snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`, and
`SCREAMING-KEBAB-CASE`.

```rust
use serverkit::{DecodeOptions, Schema, Value, Values};

#[derive(Debug, PartialEq, Schema)]
#[schema(rename_all = "kebab-case")]
enum Mode {
    FastMode,
    #[schema(rename = "safe")]
    SafeMode,
}

struct One<'a>(&'a [u8]);

impl Values for One<'_> {
    fn len(&self) -> usize { 1 }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        (index == 0).then_some(Value {
            name: "mode",
            bytes: self.0,
        })
    }
}

assert_eq!(
    Mode::decode(&One(b"fast-mode"), DecodeOptions::reject_unknown()).unwrap(),
    Mode::FastMode,
);
```

Nested schemas use dotted names such as `filter.name`. `Option<T>` makes the
entire nested object optional.

```rust
use serverkit::Schema;

#[derive(Schema)]
struct Filter {
    name: String,
    minimum: u32,
}

#[derive(Schema)]
struct Search {
    #[schema(nested)]
    filter: Filter,
    #[schema(nested)]
    paging: Option<Paging>,
}

#[derive(Schema)]
struct Paging {
    page: u32,
}
```

Generic fields receive the required `ValueSchema` or `Schema` bounds from the
derive automatically:

```rust
use serverkit::Schema;

#[derive(Schema)]
struct Wrapper<T> {
    value: T,
}
```

Custom scalar types implement `ValueSchema`; no derive or registration table is
required.

```rust
use serverkit::{SchemaKind, SchemaMetadata, ValueSchema};

struct Identifier(u64);

impl ValueSchema for Identifier {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        let value = std::str::from_utf8(bytes)
            .map_err(|_| "must be UTF-8".to_owned())?
            .parse()
            .map_err(|_| "must be an identifier".to_owned())?;
        Ok(Self(value))
    }

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::Integer)
    }
}
```

## Streaming request bodies

`Body` is the streaming extractor. Its `next` method returns one body chunk at
a time.

```rust
use serverkit::prelude::*;

async fn upload(mut body: Body) -> Result<Vec<u8>, StreamError> {
    let mut bytes = Vec::new();

    while let Some(chunk) = body.next().await {
        bytes.extend(chunk?);
    }

    Ok(bytes)
}

fn application() -> App {
    App::new(("/upload".GET(upload),))
}
```

Only one streaming extractor is permitted in a handler, and it must be last.
The handler implementations enforce this when a route is registered. If any
earlier extractor is buffered, ServerKit reads the incoming stream once, shares
the resulting slice with all buffered extractors, and then moves the same bytes
into a replay stream for `Body`. With no buffered extractor, `Body` receives the
runtime's original stream without pre-reading it.

```compile_fail
use serverkit::{App, Body, Method, RouteMethods};

async fn invalid_order(_body: Body, _method: Method) {}

fn application() -> App {
    App::new(("/upload".GET(invalid_order),))
}
```

Runtime adapters implement `RequestStream` to supply chunks:

```rust
use std::task::{Context, Poll};

use serverkit::{RequestStream, StreamError};

struct EmptyStream;

impl RequestStream for EmptyStream {
    fn poll_next(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        Poll::Ready(None)
    }
}
```

## Buffered JSON

Enable the `json` feature to deserialize the complete request body. Invalid
JSON returns HTTP 400.

```rust,ignore
use serde::Deserialize;
use serverkit::prelude::*;

#[derive(Deserialize)]
struct CreateUser {
    name: String,
}

async fn create_user(Json(user): Json<CreateUser>) -> String {
    user.name
}
```

`Json<T>` requires `Content-Type: application/json` or a media type ending in
`+json`. Unsupported media types return 415, malformed JSON returns 400, and
the application body limit is checked before deserialization. Returning
`Json<T>` serializes a JSON response with the matching content type.

## Text, bytes, and forms

`Text` and `Bytes` buffer the request body once. `Text` validates UTF-8, while
`Bytes` preserves the bytes unchanged.

```rust
use serverkit::{Bytes, Text};

async fn text(Text(body): Text) -> String {
    body
}

async fn bytes(Bytes(body): Bytes) -> Vec<u8> {
    body
}
```

`Form<T>` uses the same name-based `Schema` validation as query extraction and
requires `application/x-www-form-urlencoded`.

```rust
use serverkit::{Form, Schema};

#[derive(Schema)]
struct Login {
    email: String,
    remember: Option<bool>,
}

async fn login(Form(login): Form<Login>) -> String {
    login.email
}
```

Set a limit once on the application. Buffered extractors enforce it while
collecting, and streaming extractors enforce it as chunks are read. With no
configured limit, request bodies remain unlimited.

```rust
use serverkit::App;

let application = App::new(()).body_limit(2 * 1024 * 1024);
```

## Multipart

`Multipart` is a final streaming extractor. Parsing begins only when `next()`
is called, boundaries may span runtime chunks, and only the current field is
buffered. The configured body limit remains active across the complete body.

```rust
use serverkit::{Multipart, MultipartError};

async fn upload(mut multipart: Multipart) -> Result<String, MultipartError> {
    while let Some(field) = multipart.next().await {
        let field = field?;

        if field.name() == Some("title") {
            return Ok(field.text().unwrap_or_default().to_owned());
        }
    }

    Ok(String::new())
}
```

Each `MultipartField` exposes `headers`, `name`, `file_name`, `content_type`,
`bytes`, `into_bytes`, and UTF-8 `text` accessors.

## State, extensions, connection information, and cookies

Application state is stored once and extracted as `State<T>`, which contains an
`Arc<T>`.

```rust
use serverkit::{App, State};

struct Configuration {
    region: String,
}

async fn region(State(configuration): State<Configuration>) -> String {
    configuration.region.clone()
}

let application = App::new(()).state(Configuration {
    region: "ap-northeast-2".to_owned(),
});
```

Runtime-specific values can be inserted into a `Request` and cloned with
`Extension<T>`. The native listener automatically provides the peer
`SocketAddr` through `ConnectInfo<SocketAddr>`.

```rust
use std::net::SocketAddr;
use serverkit::ConnectInfo;

async fn peer(ConnectInfo(address): ConnectInfo<SocketAddr>) -> String {
    address.to_string()
}
```

`Cookies` parses all incoming `Cookie` headers without hiding repeated names.

```rust
use serverkit::Cookies;

async fn session(cookies: Cookies) -> String {
    cookies.get("session").unwrap_or_default().to_owned()
}
```

## Custom extractors

Metadata and buffered extractors implement `FromRequest<(&Request, &[u8])>`.
Set `BUFFERED` only when the extractor needs the complete body; otherwise the
slice is empty and the runtime stream remains untouched.

```rust
use serverkit::{FromRequest, Request, Response};

struct UserAgent(String);

impl<'request> FromRequest<(&'request Request, &'request [u8])> for UserAgent {
    type Error = Response;

    async fn from_request(
        input: (&'request Request, &'request [u8]),
    ) -> Result<Self, Self::Error> {
        let value = input
            .0
            .headers()
            .get("user-agent")
            .ok_or_else(|| Response::text(400, "missing user-agent"))?;
        let value = std::str::from_utf8(value)
            .map_err(|_| Response::text(400, "invalid user-agent"))?;

        Ok(Self(value.to_owned()))
    }
}

async fn handler(user_agent: UserAgent) -> String {
    user_agent.0
}
```

A buffered extractor uses the same signature:

```rust
use std::convert::Infallible;

use serverkit::{FromRequest, Request};

struct RawBody(Vec<u8>);

impl<'request> FromRequest<(&'request Request, &'request [u8])> for RawBody {
    type Error = Infallible;

    const BUFFERED: bool = true;

    async fn from_request(
        input: (&'request Request, &'request [u8]),
    ) -> Result<Self, Self::Error> {
        Ok(Self(input.1.to_vec()))
    }
}
```

`Body` is the owned-request extractor supplied by ServerKit. Keeping the owned
form internal to streaming extraction prevents two handler arguments from
taking the same request stream.

## Cloudflare Workers

Enable the `worker` feature. The adapter converts the host request before
dispatch and converts ServerKit's buffered response afterward; the application
itself stays runtime-independent.

```rust,ignore
use std::sync::LazyLock;

use serverkit::{
    App, RouteMethods,
    cloudflare::{self, WorkerContext},
};
use worker::{Context, Env, Request, Response, Result, event};

static APP: LazyLock<App> = LazyLock::new(|| {
    App::new(("/health".GET(health), "/colo".GET(colo)))
});

async fn health() -> &'static str {
    "ok"
}

async fn colo(context: WorkerContext) -> String {
    context
        .cf()
        .map_or_else(|| "unknown".to_owned(), |cf| cf.colo())
}

#[event(fetch)]
async fn fetch(request: Request, env: Env, context: Context) -> Result<Response> {
    cloudflare::into_response(
        APP.handle(cloudflare::from_request(request, env, context)?)
            .await,
    )
}
```

`cloudflare::from_request` preserves method, path, query, headers, body stream,
`Env`, fetch `Context`, and `Cf`. `WorkerContext` is a normal non-buffering
extractor. Its `env`, `context`, and `cf` accessors expose host data, while
`wait_until` schedules work without delaying the response. The complete
Wrangler package is in `examples/cloudflare-worker`.

## Responses

Handlers may return any `IntoResponse` implementation. ServerKit provides
implementations for `Response`, `()`, `String`, `&str`, `Vec<u8>`,
`Infallible`, and `Result<T, E>` when both sides implement `IntoResponse`.

```rust
use serverkit::Response;

async fn text() -> Response {
    Response::text(201, "created")
}

async fn bytes() -> Vec<u8> {
    vec![1, 2, 3]
}

async fn fallible(ok: bool) -> Result<String, Response> {
    if ok {
        Ok("ok".to_owned())
    } else {
        Err(Response::text(400, "invalid request"))
    }
}
```

`Response::new`, `Response::empty`, `Response::text`, and `Response::bytes`
construct buffered responses. `Content-Type` lives in the same `Headers`
collection as every other header; there is no second content-type field.

```rust
use serverkit::{Cookie, Response, SameSite};

async fn response() -> Response {
    let mut response = Response::text(200, "ok");

    response
        .headers()
        .set("Cache-Control", "no-store")
        .unwrap();
    response
        .headers()
        .append("Vary", "Accept-Encoding")
        .unwrap();
    response
        .set_cookie(
            Cookie::new("session", "abc")
                .path("/")
                .same_site(SameSite::Lax)
                .http_only(true)
                .secure(true),
        )
        .unwrap();

    response
}
```

Header names are case-insensitive. `set` replaces every existing value,
`append` preserves repeated fields such as `Set-Cookie`, and `remove` removes
all values of a name. Public writes validate header names and reject CR/LF/NUL
in values.

`Response::stream` accepts a runtime-neutral `ResponseStream` and is forwarded
without buffering by both native HTTP/1 and Cloudflare Workers.

```rust
use std::task::{Context, Poll};
use serverkit::{Response, ResponseStream, StreamError};

struct Chunks {
    chunk: Option<Vec<u8>>,
}

impl ResponseStream for Chunks {
    fn poll_next(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        Poll::Ready(self.chunk.take().map(Ok))
    }
}

async fn stream() -> Response {
    Response::stream(200, Chunks {
        chunk: Some(b"chunk".to_vec()),
    })
}
```

Redirects have explicit status semantics:

```rust
use serverkit::Redirect;

async fn redirect() -> Redirect {
    Redirect::see_other("/finished")
}
```

## Server-sent events

`Sse<S>` encodes typed `SseEvent` values and sets the required response
headers. The source implements the same poll-based shape as other streams.

```rust
use std::task::{Context, Poll};
use serverkit::{Sse, SseEvent, SseStream, StreamError};

struct Events(bool);

impl SseStream for Events {
    fn poll_next(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<SseEvent, StreamError>>> {
        if std::mem::replace(&mut self.0, false) {
            Poll::Ready(Some(Ok(SseEvent::data("ready").event("status"))))
        } else {
            Poll::Ready(None)
        }
    }
}

async fn events() -> Sse<Events> {
    Sse::new(Events(true))
}
```

## WebSockets

Enable the `websocket` feature. The same upgrade handler and message API works
with native HTTP/1 and Cloudflare Workers.

```rust,ignore
use serverkit::{Response, WebSocketMessage, WebSocketUpgrade};

async fn websocket(upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(|mut socket| async move {
        while let Some(message) = socket.next().await {
            match message {
                Ok(WebSocketMessage::Text(text)) => {
                    if socket.send_text(text).await.is_err() {
                        break;
                    }
                }
                Ok(WebSocketMessage::Binary(bytes)) => {
                    if socket.send_binary(bytes).await.is_err() {
                        break;
                    }
                }
                Ok(WebSocketMessage::Close { .. }) | Err(_) => break,
                Ok(WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_)) => {}
            }
        }
    })
}
```

`WebSocketUpgrade::protocol` selects only a protocol present in the client's
`Sec-WebSocket-Protocol` request. The native adapter performs the HTTP upgrade
and WebSocket handshake; the Workers adapter creates and accepts a
`WebSocketPair`. Workers manages ping and pong control frames itself.

## OpenAPI

`App::openapi` takes the serving path first, generates OpenAPI 3.1 from
registered routes, extractors, Schemaval metadata, validation constraints,
request media types, and response types, then serves a Scalar API Reference at
that path.

```rust
use serverkit::{App, OpenApi, Path, RouteMethods, Schema};

#[derive(Schema)]
struct ItemPath {
    id: u64,
}

async fn item(Path(path): Path<ItemPath>) -> String {
    path.id.to_string()
}

let application = App::new("/items/:id".GET(item))
    .openapi("/docs", OpenApi::new("Items API", "1.0.0"));

assert!(application
    .openapi_document()
    .unwrap()
    .as_str()
    .contains("/items/{id}"));
```

The serving path must be static. The page loads Scalar's official browser
bundle from jsDelivr and embeds the OpenAPI document generated from the
application's current routes and schemas directly into Scalar's `content`
configuration. It does not read a file or fetch a separate document endpoint.
The page supports GET, HEAD, and OPTIONS; other methods return 405 with an
`Allow` header. `App::openapi_document` provides direct access to the generated
JSON in memory.

Schemaval objects, nested objects, arrays, enums, scalar types, required fields,
numeric limits, and length limits are rendered inline. `Json<T>` currently
documents its media type but does not infer a JSON object schema from Serde
alone.

## Listener adapters

`App::run` dispatches to the `Listener` implementation of the value passed by
the user. `std::net::TcpListener` is implemented by ServerKit. Other runtimes
can expose their own local wrapper type and implement the same trait.

```rust
use serverkit::{App, Listener};

struct TestListener;

impl Listener for TestListener {
    type Output = (App, &'static str);

    fn serve(self, application: App) -> Self::Output {
        (application, "ready")
    }
}

let application = App::new(());
let (_application, state) = application.run(TestListener);

assert_eq!(state, "ready");
```

The application-level routing and extraction layer remains independent of the
listener and request-stream adapter used by a native runtime or Worker host.
