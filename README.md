# ServerKit

ServerKit is a portable Rust HTTP router with an Ohkami-inspired routing API.
The core stays runtime-independent; `serverkit-hyper` provides native HTTP/1.0,
HTTP/1.1, and HTTP/2 serving while `serverkit-worker` connects the same `Router`,
routes, handlers, and extractors to Cloudflare Workers.

## Installation

```toml
[dependencies]
serverkit = { version = "0.1", features = ["json", "websocket"] }
serde = { version = "1", features = ["derive"] }
serverkit-hyper = { version = "0.1", features = ["websocket"] }

# Use these instead of serverkit-hyper on Cloudflare Workers.
serverkit-worker = { version = "0.1", features = ["websocket"] }
worker = "0.8.5"
```

The `json` and `websocket` features are optional. Each runtime adapter keeps its
runtime dependencies out of the `serverkit` core crate.

## Complete native server

The same `Schema` derive decodes and validates path parameters, query
parameters, and headers by name.

```rust,ignore
use serverkit::prelude::*;
use serverkit_hyper::Http;

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
    let router = Router::new(Config::new(), (
        "/health".GET(health),
        "/:organization/users/:id".GET(get_user),
    ));

    router.run(Http::bind("127.0.0.1:3000")?)
}
```

`Http` automatically detects HTTP/1.0, HTTP/1.1, and HTTP/2 after accepting a
connection. TLS and HTTP/3 are separate transport concerns and are not provided
by this adapter.

`Router::new` accepts one route or a convenience tuple. `.route()` can then be
called any number of times, so the number of routes in a router is not
bounded by tuple arity. Handler functions may have zero through sixteen
extractor arguments. Metadata and buffered extractors may appear in any order.
A streaming extractor such as `Body` or `Multipart`, when present, must be the
final argument.

```rust
use serverkit::{Config, Router, RouteMethods};

async fn health() -> &'static str { "ok" }
async fn metrics() -> &'static str { "metrics" }

let router = Router::new(Config::new(), "/health".GET(health))
    .route("/metrics".GET(metrics));
```

## HTTP methods

Routes support `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, and `OPTIONS`.
The same path can register a different handler for each method.

```rust
use serverkit::{Config, Router, RouteMethods};

async fn read() -> &'static str {
    "read"
}

async fn create() -> &'static str {
    "created"
}

fn router() -> Router {
    Router::new(Config::new(), ("/items".GET(read), "/items".POST(create)))
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

fn router() -> Router {
    Router::new(Config::new(), ("/asdf/:id/asdd".GET(item),))
}
```

A scalar schema is a convenience for routes containing exactly one parameter.

```rust
use serverkit::prelude::*;

async fn gpu(Path(id): Path<u64>) -> String {
    id.to_string()
}

fn router() -> Router {
    Router::new(Config::new(), ("/gpus/:id".GET(gpu),))
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

let router = Router::new(Config::new(), "/assets/*path".GET(asset));
```

Matching is deterministic from left to right: static segments precede
parameters, and parameters precede wildcards. Equivalent patterns such as
`/users/:id` and `/users/:name` for the same method are rejected when the router
is built. Empty parameter names, duplicate parameter names, non-terminal
wildcards, queries, fragments, duplicate slashes, and trailing slashes are
also rejected.

`Config::prefix` gives a router its own static prefix. `.at()` adds a mount
outside that prefix, and a child router is registered with the same `.route()`
method used for individual routes:

```rust
use serverkit::{Config, Router, RouteMethods};

async fn users() -> &'static str { "users" }
async fn missing() -> &'static str { "missing" }

let api = Router::new(
    Config::new().prefix("/v1"),
    "/users".GET(users),
)
.at("/service");

let router = Router::new(Config::new().prefix("/root"), ())
    .route(api)
    .fallback(missing);
```

The resulting route is `/root/service/v1/users`. Prefixes always compose in
this order: parent `Config::prefix`, child `.at()`, child `Config::prefix`, and
the route path. Prefixes are static, start with `/`, and cannot end with `/`.
`Config::new()` is required even when no options are set so router construction
keeps one stable shape as configuration grows.

## Middleware

Middleware can be attached to a router scope or to one route. Parent router
middleware wraps child router middleware, which wraps route middleware and the
handler. The response unwinds in reverse order.

```rust
use serverkit::{
    Config, Middleware, Next, Request, Response, RouteMethods, Router,
};

struct Trace;

impl Middleware for Trace {
    async fn handle(&self, request: Request, next: Next<'_>) -> Response {
        let mut response = next.run(request).await;
        response.headers().set("X-Trace", "complete").unwrap();
        response
    }
}

struct Authentication;

impl Middleware for Authentication {
    async fn handle(&self, request: Request, next: Next<'_>) -> Response {
        if request.headers.contains("Authorization") {
            next.run(request).await
        } else {
            Response::text(401, "Unauthorized")
        }
    }
}

struct RequestId;

impl Middleware for RequestId {
    async fn handle(&self, mut request: Request, next: Next<'_>) -> Response {
        request
            .headers
            .set("X-Request-Id", "generated")
            .unwrap();
        next.run(request).await
    }
}

async fn private() -> &'static str { "private" }
async fn public() -> &'static str { "public" }

let api = Router::new(
    Config::new().prefix("/api"),
    (
        "/private".GET(private),
        "/public"
            .GET(public)
            .without_middleware::<Authentication>(),
    ),
)
.middleware(Authentication);

let router = Router::new(Config::new(), ())
    .middleware(Trace)
    .middleware(RequestId)
    .route(api);
```

`Route::without_middleware::<M>()` skips inherited middleware with the exact
concrete type `M` for that route. It does not remove middleware attached
directly to the route. Scoped middleware also runs for a scoped fallback and
for generated responses such as 404, 405, and automatic OPTIONS within that
scope; route middleware only runs after a route is selected.

`Request::method`, `Request::path`, `Request::query`, and `Request::headers` are
public fields, so middleware can replace request metadata before extraction.
Routing and path-parameter capture have already completed before middleware
runs; changing `method` or `path` affects downstream middleware and extractors
but does not select a different route or recalculate path parameters. Request
body replacement remains internal until its streaming transformation API is
defined.

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

fn router() -> Router {
    Router::new(Config::new(), ("/search".GET(search),))
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

fn router() -> Router {
    Router::new(Config::new(), ("/authenticated".GET(authenticated),))
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
types, `f32`, `f64`, `Ipv4Addr`, `Ipv6Addr`, and `IpAddr`. Struct fields support:

- required `T` values;
- optional `Option<T>` values;
- repeated `Vec<T>` values (`Vec<u8>` remains a single byte value);
- one `#[schema(rest)] ExtraFields` field;
- `#[schema(default)]` and `#[schema(default = expression)]`;
- `minimum`, `maximum`, `min_length`, and `max_length`;
- field and whole-struct custom validation.
- nested schemas through dotted input names;
- repeated nested schemas through indexed dotted input names;
- generic schemas, string enums, and tagged data enums;
- OpenAPI formats through `#[schema(format = "...")]`;
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
entire nested object optional. Repeated nested schemas use names such as
`filters.0.name` and `filters.1.name`. A default applies when no value under the
nested prefix is present.

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

#[derive(Default, Schema)]
struct Paging {
    page: u32,
}

#[derive(Schema)]
struct Request {
    #[schema(nested)]
    filters: Vec<Filter>,
    #[schema(nested, default)]
    paging: Paging,
    #[schema(format = "uuid")]
    request_id: String,
}
```

OpenAPI documents repeated nested leaves with an index placeholder such as
`filters.{index}.name` and marks them with `x-serverkit-indexed: true`. Tagged
enums are expanded into their discriminator and variant fields for path, query,
and header parameters; fields that only belong to some variants are optional.

Enums containing data use an explicit discriminator. Unit-only enums keep the
single string representation shown above.

```rust
use serverkit::Schema;

#[derive(Schema)]
#[schema(tag = "type", rename_all = "snake_case")]
enum Selection {
    All,
    Range {
        start: u32,
        end: u32,
    },
}
```

`type=range&start=1&end=10` decodes to `Selection::Range`. OpenAPI emits a
`oneOf` schema with `type` as its discriminator.

`format` changes OpenAPI metadata; it does not by itself validate a string.
Combine it with `validate` for values such as UUIDs. The built-in IP address
types perform real parsing and emit `ipv4` or `ipv6` formats automatically.

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

`Body` is the streaming extractor. Its `next` method borrows one body chunk at
a time directly from the runtime adapter. The slice remains valid until the
next mutable access to that `Body`.

```rust
use serverkit::prelude::*;

async fn upload(mut body: Body) -> Result<Vec<u8>, StreamError> {
    let mut bytes = Vec::new();

    while let Some(chunk) = body.next().await {
        bytes.extend_from_slice(chunk?);
    }

    Ok(bytes)
}

fn router() -> Router {
    Router::new(Config::new(), ("/upload".GET(upload),))
}
```

Only one streaming extractor is permitted in a handler, and it must be last.
The handler implementations enforce this when a route is registered. If any
earlier extractor is buffered, ServerKit reads the incoming stream once, shares
the resulting slice with all buffered extractors, and then moves the same bytes
into a replay stream for `Body`. With no buffered extractor, `Body` receives the
runtime's original stream without pre-reading it.

```compile_fail
use serverkit::{Body, Config, Method, RouteMethods, Router};

async fn invalid_order(_body: Body, _method: Method) {}

fn router() -> Router {
    Router::new(Config::new(), ("/upload".GET(invalid_order),))
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
    ) -> Poll<Option<Result<(), StreamError>>> {
        Poll::Ready(None)
    }

    fn chunk(&self) -> &[u8] {
        &[]
    }
}
```

## Buffered JSON

Enable the `json` feature to deserialize the complete request body. Invalid
JSON returns HTTP 400.

```rust,ignore
use serde::Deserialize;
use serverkit::prelude::*;

#[derive(Deserialize, Schema)]
struct CreateUser {
    name: String,
}

async fn create_user(Json(user): Json<CreateUser>) -> String {
    user.name
}
```

`Json<T>` requires `Content-Type: application/json` or a media type ending in
`+json`. Unsupported media types return 415, malformed JSON returns 400, and
the router body limit is checked before deserialization. Returning
`Json<T>` serializes a JSON response with the matching content type. `T` also
implements `Schema`, allowing request and response types to be emitted into
OpenAPI `components/schemas` and referenced with `$ref`.

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

Set a limit once on the router. Buffered extractors enforce it while
collecting, and streaming extractors enforce it as chunks are read. With no
configured limit, request bodies remain unlimited.

```rust
use serverkit::{Config, Router};

let router = Router::new(Config::new(), ()).body_limit(2 * 1024 * 1024);
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

Router state is stored once and extracted as `State<T>`, which contains an
`Arc<T>`.

```rust
use serverkit::{Config, Router, State};

struct Configuration {
    region: String,
}

async fn region(State(configuration): State<Configuration>) -> String {
    configuration.region.clone()
}

let router = Router::new(Config::new(), ()).state(Configuration {
    region: "ap-northeast-2".to_owned(),
});
```

Runtime-specific values can be inserted into a `Request` and cloned with
`Extension<T>`. The Hyper adapter automatically provides the peer `SocketAddr`
through `ConnectInfo<SocketAddr>`.

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
            .headers
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

Add `serverkit-worker`. The adapter converts the host request before dispatch
and converts the ServerKit response afterward; the router itself stays
runtime-independent.

```rust,ignore
use std::sync::LazyLock;

use serverkit::{Config, Router, RouteMethods};
use serverkit_worker::{WorkerContext, from_request, into_response};
use worker::{Context, Env, Request, Response, Result, event};

static ROUTER: LazyLock<Router> = LazyLock::new(|| {
    Router::new(Config::new(), ("/health".GET(health), "/colo".GET(colo)))
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
    into_response(ROUTER.handle(from_request(request, env, context)?).await)
}
```

`serverkit_worker::from_request` preserves method, path, query, headers, body stream,
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
without buffering by both native HTTP and Cloudflare Workers.
`poll_next` advances the stream, and `chunk` borrows bytes owned by the stream
until its next mutable call. Implementations can therefore reuse one allocation
instead of allocating and transferring a new `Vec<u8>` for every chunk. The
current Hyper and Workers host APIs require owned output values, so their
adapters copy each borrowed chunk at that final boundary.

```rust
use std::task::{Context, Poll};
use serverkit::{Response, ResponseStream, StreamError};

struct Chunks {
    chunk: Vec<u8>,
    sent: bool,
}

impl ResponseStream for Chunks {
    fn poll_next(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<(), StreamError>>> {
        if self.sent {
            Poll::Ready(None)
        } else {
            self.sent = true;
            Poll::Ready(Some(Ok(())))
        }
    }

    fn chunk(&self) -> &[u8] {
        &self.chunk
    }
}

async fn stream() -> Response {
    Response::stream(200, Chunks {
        chunk: b"chunk".to_vec(),
        sent: false,
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
with native HTTP/1.1 and Cloudflare Workers.

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

`Router::openapi` takes the serving path first, generates OpenAPI 3.1 from
registered routes, extractors, Schemaval metadata, validation constraints,
request media types, and response types, then serves a Scalar API Reference at
that path.

```rust
use serverkit::{
    Config, Router, OpenApi, Path, RouteMethods, Scalar, ScalarDeveloperTools, Schema,
    SchemaKind, SchemaMetadata, SecurityRequirement, SecurityScheme, Server,
};

#[derive(Schema)]
struct ItemPath {
    id: u64,
}

async fn item(Path(path): Path<ItemPath>) -> String {
    path.id.to_string()
}

let route = "/items/:id"
    .GET(item)
    .summary("Read an item")
    .description("Reads one item by ID")
    .tag("items")
    .operation_id("readItem")
    .openapi(|operation| {
        operation
            .security(SecurityRequirement::new("bearerAuth"))
            .response_header(
                200,
                "X-Request-Id",
                "Request identifier",
                SchemaMetadata::new(SchemaKind::String).format("uuid"),
            )
            .response_example(200, "text/plain", "sample", "42");
    });

let document = OpenApi::new("Items API", "1.0.0")
    .server(Server::new("https://api.example.com").description("Production"))
    .security_scheme("bearerAuth", SecurityScheme::bearer())
    .security(SecurityRequirement::new("bearerAuth"))
    .scalar_config(
        Scalar::new()
            .theme("moon")
            .show_sidebar(true)
            .developer_tools(ScalarDeveloperTools::Localhost),
    );

let router = Router::new(Config::new(), route).openapi("/docs", document);

assert!(router
    .openapi_document()
    .unwrap()
    .as_str()
    .contains("/items/{id}"));
```

The serving path must be static. The page loads the pinned Scalar browser
bundle `@scalar/api-reference@1.63.0` from jsDelivr and embeds the OpenAPI document generated from the
router's current routes and schemas directly into Scalar's `content`
configuration. It does not read a file or fetch a separate document endpoint.
The page supports GET, HEAD, and OPTIONS; other methods return 405 with an
`Allow` header. `Router::openapi_document` provides direct access to the generated
JSON in memory.

Named Schemaval types, including `Json<T>` request and response bodies, are
deduplicated under `components/schemas` and referenced with `$ref`. Route
builders expose summary, description, tags, operation IDs, and a custom
`openapi` modifier. `OpenApi` supports servers, API key, HTTP bearer, OAuth2,
and OpenID Connect security schemes. `Operation` supports request/response
examples and response header schemas. Examples preserve JSON value types:

```rust
use serverkit::ExampleValue;

let _example = ExampleValue::object([
    ("name", ExampleValue::from("sample")),
    ("count", ExampleValue::from(2_u32)),
    ("active", ExampleValue::from(true)),
]);
```

Pass an `ExampleValue` to `Operation::request_example` or
`Operation::response_example`. String inputs remain accepted directly.

## Listener adapters

`Router::run` dispatches to the `Listener` implementation of the value passed by
the user. `serverkit_hyper::Http` owns a `std::net::TcpListener`, automatically
serves HTTP/1.0, HTTP/1.1, or HTTP/2, and implements that trait. Other runtimes
can expose their own local wrapper type and implement the same trait.

```rust
use serverkit::{Config, Listener, Router};

struct TestListener;

impl Listener for TestListener {
    type Output = (Router, &'static str);

    fn serve(self, router: Router) -> Self::Output {
        (router, "ready")
    }
}

let router = Router::new(Config::new(), ());
let (_router, state) = router.run(TestListener);

assert_eq!(state, "ready");
```

The routing and extraction layer remains independent of the listener and
request-stream adapter used by a native runtime or Worker host. An external
adapter implements `RequestStream`, constructs `Request::from_parts`, calls
`Router::handle`, and consumes the result with `Response::into_parts`.
