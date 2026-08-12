# ServerKit

ServerKit is a portable Rust HTTP application layer with an Ohkami-inspired
routing API. Native HTTP/1 serving is available through `std::net::TcpListener`;
additional listeners and request adapters can implement ServerKit's public
traits without changing application code.

## Installation

```toml
[dependencies]
serverkit = "0.1"

# Optional buffered JSON extraction.
serverkit = { version = "0.1", features = ["json"] }
serde = { version = "1", features = ["derive"] }
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

`App::new` accepts a route or a tuple containing up to twelve routes. Handler
functions may have zero through twelve extractor arguments. Extractor order is
not significant.

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

## Query extraction

Query schemas reject undeclared fields by default. Repeated names decode into
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

For example, `?q=rust&tag=web&tag=server` is valid. Adding an undeclared
`debug=true` field returns HTTP 400 with an `UnknownField` validation issue.
Names and values use form-style percent decoding, including `+` as a space.

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

## Schemaval rules

The built-in scalar types are `String`, `Vec<u8>`, `bool`, all standard integer
types, `f32`, and `f64`. Struct fields support:

- required `T` values;
- optional `Option<T>` values;
- repeated `Vec<T>` values (`Vec<u8>` remains a single byte value);
- `#[schema(default)]` and `#[schema(default = expression)]`;
- `minimum`, `maximum`, `min_length`, and `max_length`;
- field and whole-struct custom validation.

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

`Schema::decode` accepts `DecodeOptions::reject_unknown()` or
`DecodeOptions::allow_unknown()`. ServerKit fixes the extractor policies as
follows:

| Extractor | Unknown input names |
| --- | --- |
| `Path<T>` | rejected |
| `Query<T>` | rejected |
| `Header<T>` | allowed |

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

Only one streaming extractor is permitted in a handler. This is enforced by
the handler's compile-time body-ownership state. If any buffered extractor is
also present, ServerKit buffers the incoming body once and gives `Body` a
replay stream beginning at offset zero.

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

## Custom extractors

`FromRequest` receives public request metadata through `Input`. An extractor
that does not consume the body uses the default `Unused` mode.

```rust
use serverkit::{
    FromRequest, Input, IntoResponse, Response,
};

struct UserAgent(String);

impl FromRequest for UserAgent {
    type Error = Response;

    async fn from_request(
        input: Input<'_, serverkit::Unused>,
    ) -> Result<Self, Self::Error> {
        let value = input
            .request()
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

Body-consuming custom extractors implement `FromRequest<Buffered>` to receive
`&[u8]`, or `FromRequest<Streaming>` to receive `Box<dyn RequestStream>`.

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

`Response::empty`, `Response::text`, and `Response::bytes` construct buffered
responses. The `status`, `content_type`, and `body` methods inspect them.

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
