use std::{any::TypeId, collections::HashMap, fmt, sync::Arc};

use crate::{
    Dispatch, Dispatcher, ErrorFormat, Handler, HttpError, IntoResponse, Middleware, OpenApi,
    OpenApiDocument, Request, Response, Routes, Scope,
    error::JsonErrorFormat,
    middleware::{MiddlewareEntry, MiddlewareTerminal, run as run_middleware},
    router::{join_paths, validate_scope_prefix},
};

#[derive(Clone)]
pub struct Config {
    prefix: String,
    error_format: Arc<dyn ErrorFormat>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            error_format: Arc::new(JsonErrorFormat),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    pub fn error_format(mut self, format: impl ErrorFormat) -> Self {
        self.error_format = Arc::new(format);
        self
    }
}

pub struct Router {
    dispatcher: Dispatcher,
    scope: Scope,
    openapi: Option<PublishedOpenApi>,
    error_format: Arc<dyn ErrorFormat>,
}

struct PublishedOpenApi {
    path: String,
    configuration: OpenApi,
    document: OpenApiDocument,
    scalar_page: String,
}

impl Router {
    pub fn new(config: Config, routes: impl Routes) -> Self {
        let Config {
            prefix,
            error_format,
        } = config;
        let prefix = normalize_prefix(prefix);
        validate_scope_prefix(&prefix)
            .unwrap_or_else(|error| panic!("invalid Router prefix `{prefix}`: {error}"));
        let mut router = Self {
            dispatcher: Dispatcher::new(),
            scope: Scope::new(prefix),
            openapi: None,
            error_format,
        };

        routes.apply(&mut router);

        router
    }

    pub(crate) fn register<
        Arguments: 'static,
        Input: 'static,
        H: Handler<Arguments, Input> + Send + Sync + 'static,
    >(
        &mut self,
        method: crate::Method,
        path: &'static str,
        handler: H,
        operation: crate::Operation,
        middlewares: Vec<MiddlewareEntry>,
        excluded_middlewares: Vec<TypeId>,
    ) {
        let path = join_paths(self.scope.prefix(), path);
        self.dispatcher.register(
            method,
            path,
            handler,
            operation,
            middlewares,
            excluded_middlewares,
        );
    }

    pub fn route(mut self, routes: impl Routes) -> Self {
        routes.apply(&mut self);
        self.refresh_openapi();
        self
    }

    pub fn at(mut self, prefix: impl Into<String>) -> Self {
        let prefix = normalize_prefix(prefix.into());
        validate_scope_prefix(&prefix)
            .unwrap_or_else(|error| panic!("invalid Router mount `{prefix}`: {error}"));

        if prefix.is_empty() {
            return self;
        }

        self.dispatcher.prepend(&prefix);
        self.scope.prepend(&prefix);
        if let Some(published) = &mut self.openapi {
            published.path = join_paths(&prefix, &published.path);
        }
        self.refresh_openapi();
        self
    }

    pub(crate) fn register_router(&mut self, mut router: Router) {
        let parent_prefix = self.scope.prefix().to_owned();
        if !parent_prefix.is_empty() {
            router = router.at(parent_prefix);
        }

        self.dispatcher.add_scope(router.scope);
        self.dispatcher.merge(router.dispatcher);
    }

    pub fn fallback<Arguments: 'static, Input: 'static>(
        mut self,
        handler: impl Handler<Arguments, Input> + Send + Sync + 'static,
    ) -> Self {
        self.dispatcher.set_fallback(self.scope.prefix(), handler);
        self.refresh_openapi();
        self
    }

    pub fn state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.scope.state(state);
        self
    }

    pub fn body_limit(mut self, limit: usize) -> Self {
        self.scope.body_limit(limit);
        self
    }

    pub fn middleware<M: Middleware>(mut self, middleware: M) -> Self {
        self.scope.middleware(MiddlewareEntry::new(middleware));
        self
    }

    pub fn openapi(mut self, path: impl Into<String>, configuration: OpenApi) -> Self {
        let path = join_paths(self.scope.prefix(), &path.into());
        self.publish_openapi(path, configuration);
        self
    }

    pub fn openapi_document(&self) -> Option<&OpenApiDocument> {
        self.openapi.as_ref().map(|published| &published.document)
    }

    pub async fn handle(&self, mut request: Request) -> Response {
        let head = request.method == crate::Method::HEAD;
        let terminal = match self.openapi.as_ref() {
            Some(published) if request.path == published.path => RouterTerminal::OpenApi(published),
            _ => RouterTerminal::Dispatch(self.dispatcher.resolve(&request)),
        };
        let exclusions = terminal.excluded_middlewares();
        let mut states = HashMap::new();
        let mut body_limit = None;
        let mut middlewares = Vec::new();

        if self.scope.matches(&request.path) {
            apply_scope(
                &self.scope,
                exclusions,
                &mut states,
                &mut body_limit,
                &mut middlewares,
            );
        }

        for scope in self.dispatcher.matching_scopes(&request.path) {
            apply_scope(
                scope,
                exclusions,
                &mut states,
                &mut body_limit,
                &mut middlewares,
            );
        }

        middlewares.extend(terminal.route_middlewares());
        request.set_states(states);
        request.set_body_limit(body_limit);

        let response = run_middleware(&middlewares, &terminal, request).await;
        let response = self.finalize_error(response);

        if head {
            response.without_body()
        } else {
            response
        }
    }

    fn finalize_error(&self, mut response: Response) -> Response {
        let error = match response.take_error() {
            Some(error) => error,
            None if (400..=599).contains(&response.status()) => HttpError::new(
                response.status(),
                format!("http.{}", response.status()),
                response_error_message(&response),
            ),
            None => return response,
        };
        let status = error.status();
        let mut headers = response.take_headers();
        remove_representation_headers(&mut headers);
        let mut formatted = self.error_format.format(&error);

        if let Some(format_error) = formatted.take_error() {
            formatted = JsonErrorFormat.format(&format_error);
        }

        formatted.set_status(status);
        formatted.merge_headers(headers);
        formatted
    }

    fn publish_openapi(&mut self, path: String, configuration: OpenApi) {
        self.dispatcher.validate_openapi_path(&path);
        let document = self.build_openapi(&configuration);
        let scalar_page = configuration.scalar_page(&document);
        self.openapi = Some(PublishedOpenApi {
            path,
            configuration,
            document,
            scalar_page,
        });
    }

    fn refresh_openapi(&mut self) {
        let Some((path, configuration)) = self
            .openapi
            .as_ref()
            .map(|published| (published.path.clone(), published.configuration.clone()))
        else {
            return;
        };

        self.publish_openapi(path, configuration);
    }

    fn build_openapi(&self, configuration: &OpenApi) -> OpenApiDocument {
        configuration.build(self.dispatcher.openapi_routes())
    }
}

fn response_error_message(response: &Response) -> String {
    std::str::from_utf8(response.body())
        .ok()
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| status_message(response.status()).to_owned())
}

fn status_message(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        422 => "Unprocessable Content",
        426 => "Upgrade Required",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "HTTP Error",
    }
}

fn remove_representation_headers(headers: &mut crate::Headers) {
    for name in [
        "Content-Encoding",
        "Content-Length",
        "Content-Type",
        "Transfer-Encoding",
    ] {
        headers.remove(name);
    }
}

fn normalize_prefix(prefix: String) -> String {
    if prefix == "/" { String::new() } else { prefix }
}

fn apply_scope<'scope>(
    scope: &'scope Scope,
    exclusions: &[TypeId],
    states: &mut HashMap<TypeId, std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    body_limit: &mut Option<usize>,
    middlewares: &mut Vec<&'scope MiddlewareEntry>,
) {
    states.extend(
        scope
            .states()
            .iter()
            .map(|(type_id, state)| (*type_id, state.clone())),
    );
    if let Some(limit) = scope.configured_body_limit() {
        *body_limit = Some(limit);
    }
    middlewares.extend(
        scope
            .middlewares()
            .iter()
            .filter(|middleware| !exclusions.contains(&middleware.type_id())),
    );
}

enum RouterTerminal<'router> {
    OpenApi(&'router PublishedOpenApi),
    Dispatch(Dispatch<'router>),
}

impl RouterTerminal<'_> {
    fn excluded_middlewares(&self) -> &[TypeId] {
        match self {
            Self::OpenApi(_) => &[],
            Self::Dispatch(dispatch) => dispatch.excluded_middlewares(),
        }
    }

    fn route_middlewares(&self) -> &[MiddlewareEntry] {
        match self {
            Self::OpenApi(_) => &[],
            Self::Dispatch(dispatch) => dispatch.route_middlewares(),
        }
    }
}

impl MiddlewareTerminal for RouterTerminal<'_> {
    fn call(&self, request: Request) -> crate::middleware::MiddlewareFuture<'_> {
        Box::pin(async move {
            match self {
                Self::OpenApi(published) => {
                    let mut response =
                        Response::bytes(200, published.scalar_page.as_bytes().to_vec());
                    response.set_header("Content-Type", "text/html; charset=utf-8");

                    let mut response = match request.method.as_str() {
                        "GET" => response,
                        "HEAD" => response,
                        "OPTIONS" => Response::empty(),
                        _ => HttpError::new(405, "route.method_not_allowed", "Method Not Allowed")
                            .into_response(),
                    };
                    response.set_header("Allow", "GET, HEAD, OPTIONS");
                    response
                }
                Self::Dispatch(dispatch) => dispatch.call(request).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use crate::{
        Config, Form, Headers, HttpError, Method, Middleware, Next, OpenApi, Path, Query, Request,
        RequestStream, Response, RouteMethods, Router, StreamError,
    };

    #[derive(crate::Schema)]
    struct ItemPath {
        id: u64,
    }

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct SearchQuery {
        query: String,
        page: Option<u32>,
    }

    #[derive(crate::Schema)]
    struct CreateItem {
        name: String,
    }

    #[cfg(feature = "json")]
    #[derive(crate::Schema, serde::Deserialize, serde::Serialize)]
    struct JsonItem {
        name: String,
    }

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

    async fn item(Path(path): Path<ItemPath>, Query(query): Query<SearchQuery>) -> String {
        format!("{}:{}", path.id, query.query)
    }

    async fn create(Form(item): Form<CreateItem>) -> String {
        item.name
    }

    async fn failure() -> Result<&'static str, HttpError> {
        Err(HttpError::new(
            409,
            "sample.failed",
            "The sample operation failed",
        ))
    }

    async fn raw_error() -> Response {
        Response::text(418, "custom raw error")
    }

    struct AddErrorHeader;

    impl Middleware for AddErrorHeader {
        async fn handle(&self, request: Request, next: Next<'_>) -> Response {
            let mut response = next.run(request).await;
            response
                .headers()
                .set("X-Error-Scope", "middleware")
                .unwrap();
            response
        }
    }

    #[cfg(feature = "json")]
    async fn create_json(crate::Json(item): crate::Json<JsonItem>) -> crate::Json<JsonItem> {
        crate::Json(item)
    }

    fn request(method: &str, path: &str) -> Request {
        Request::from_parts(
            Method::try_from(method).unwrap(),
            path,
            None,
            Headers::new(),
            Box::new(EmptyStream),
        )
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn publishes_route_and_schema_metadata() {
        let application = Router::new(
            Config::new(),
            (
                "/items/:id"
                    .GET(item)
                    .summary("Read an item")
                    .description("Reads one item by ID")
                    .tag("items")
                    .operation_id("readItem")
                    .openapi(|operation| {
                        operation.response_header(
                            200,
                            "X-Request-Id",
                            "Request identifier",
                            crate::SchemaMetadata::new(crate::SchemaKind::String).format("uuid"),
                        );
                    }),
                "/items".POST(create),
            ),
        )
        .openapi("/docs", OpenApi::new("Items", "1.0"));
        let document = application.openapi_document().unwrap().as_str();

        assert!(document.contains("\"/items/{id}\""));
        assert!(document.contains("\"name\":\"id\",\"in\":\"path\""));
        assert!(document.contains("\"name\":\"query\",\"in\":\"query\""));
        assert!(document.contains("application/x-www-form-urlencoded"));
        assert!(document.contains("\"413\""));
        assert!(document.contains("\"summary\":\"Read an item\""));
        assert!(document.contains("\"operationId\":\"readItem\""));
        assert!(document.contains("\"X-Request-Id\""));

        #[cfg(feature = "json")]
        serde_json::from_str::<serde_json::Value>(document).unwrap();
    }

    #[test]
    fn applies_one_custom_error_format_and_preserves_http_semantics() {
        let application = Router::new(
            Config::new().error_format(|error: &HttpError| {
                Response::text(200, format!("{}:{}", error.code(), error.message()))
            }),
            "/failure".GET(failure),
        )
        .middleware(AddErrorHeader);
        let mut response = block_on(application.handle(request("GET", "/failure")));

        assert_eq!(response.status(), 409);
        assert_eq!(response.content_type(), Some("text/plain; charset=utf-8"));
        assert_eq!(
            response.headers().get("x-error-scope"),
            Some(b"middleware".as_slice()),
        );
        assert_eq!(
            response.body(),
            b"sample.failed:The sample operation failed",
        );
    }

    #[test]
    fn normalizes_raw_error_responses_with_the_default_json_format() {
        let application = Router::new(Config::new(), "/failure".GET(raw_error));
        let response = block_on(application.handle(request("GET", "/failure")));

        assert_eq!(response.status(), 418);
        assert_eq!(response.content_type(), Some("application/json"));
        assert_eq!(
            response.body(),
            br#"{"error":{"code":"http.418","message":"custom raw error","fields":[]}}"#,
        );
    }

    #[test]
    fn suppresses_a_formatted_error_body_for_head_requests() {
        let application = Router::new(Config::new(), "/failure".GET(failure));
        let get = block_on(application.handle(request("GET", "/failure")));
        let expected_length = get.body().len().to_string();
        let mut head = block_on(application.handle(request("HEAD", "/failure")));

        assert_eq!(head.status(), 409);
        assert_eq!(
            head.headers().get("content-length"),
            Some(expected_length.as_bytes()),
        );
        assert!(head.body().is_empty());
    }

    #[test]
    fn suppresses_a_not_found_body_for_head_requests() {
        let application = Router::new(Config::new(), "/ok".GET(|| async { "ok" }));
        let get = block_on(application.handle(request("GET", "/missing")));
        let expected_length = get.body().len().to_string();
        let mut head = block_on(application.handle(request("HEAD", "/missing")));

        assert_eq!(head.status(), 404);
        assert_eq!(head.content_type(), Some("application/json"));
        assert_eq!(
            head.headers().get("content-length"),
            Some(expected_length.as_bytes()),
        );
        assert!(head.body().is_empty());
    }

    #[test]
    fn the_parent_router_controls_nested_error_formatting() {
        let child = Router::new(
            Config::new().error_format(|error: &HttpError| {
                Response::text(200, format!("child:{}", error.code()))
            }),
            "/failure".GET(failure),
        )
        .at("/child");
        let application = Router::new(
            Config::new().error_format(|error: &HttpError| {
                Response::text(200, format!("parent:{}", error.code()))
            }),
            child,
        );
        let response = block_on(application.handle(request("GET", "/child/failure")));

        assert_eq!(response.status(), 409);
        assert_eq!(response.body(), b"parent:sample.failed");
    }

    #[test]
    fn omits_methods_without_openapi_operation_fields() {
        let propfind = Method::from_bytes(b"PROPFIND").unwrap();
        let application = Router::new(
            Config::new(),
            (
                "/visible".GET(|| async { "visible" }),
                "/tunnel".CONNECT(|| async { "tunnel" }),
                "/properties".on(propfind, || async { "properties" }),
            ),
        )
        .openapi("/docs", OpenApi::new("Methods", "1.0"));
        let document = application.openapi_document().unwrap().as_str();

        assert!(document.contains("\"/visible\""));
        assert!(!document.contains("\"/tunnel\""));
        assert!(!document.contains("\"/properties\""));
    }

    #[test]
    fn serves_the_openapi_reference_with_head_and_method_handling() {
        let application =
            Router::new(Config::new(), ()).openapi("/docs", OpenApi::new("Empty", "1.0"));
        let response = block_on(application.handle(request("GET", "/docs")));

        assert_eq!(response.status(), 200);
        assert_eq!(response.content_type(), Some("text/html; charset=utf-8"));
        assert!(!response.body().is_empty());

        let response = block_on(application.handle(request("HEAD", "/docs")));
        assert_eq!(response.status(), 200);
        assert!(response.body().is_empty());

        let mut response = block_on(application.handle(request("POST", "/docs")));
        assert_eq!(response.status(), 405);
        assert_eq!(
            response.headers().get("allow"),
            Some(b"GET, HEAD, OPTIONS".as_slice()),
        );
    }

    #[test]
    fn serves_a_scalar_reference_for_the_openapi_document() {
        let application = Router::new(Config::new(), "/items/:id".GET(item))
            .openapi("/reference", OpenApi::new("Items & API", "1.0"));
        let response = block_on(application.handle(request("GET", "/reference")));
        let body = std::str::from_utf8(response.body()).unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.content_type(), Some("text/html; charset=utf-8"));
        assert!(body.contains("<title>Items &amp; API</title>"));
        assert!(body.contains("Scalar.createApiReference('#app',{content:\"{"));
        assert!(body.contains("\\\"openapi\\\":\\\"3.1.0\\\""));
        assert!(body.contains("/items/{id}"));
        assert!(!body.contains("Scalar.createApiReference('#app',{url:"));
        assert!(body.contains("https://cdn.jsdelivr.net/npm/@scalar/api-reference"));
    }

    #[test]
    fn scopes_openapi_routes_and_reference_with_config_and_mount_prefixes() {
        let router = Router::new(Config::new().prefix("/v1"), "/items/:id".GET(item))
            .openapi("/docs", OpenApi::new("Items", "1.0"))
            .at("/service");
        let document = router.openapi_document().unwrap().as_str();

        assert!(document.contains("\"/service/v1/items/{id}\""));
        assert_eq!(
            block_on(router.handle(request("GET", "/service/v1/docs"))).status(),
            200,
        );
        assert_eq!(
            block_on(router.handle(request("GET", "/v1/service/docs"))).status(),
            404,
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn derives_json_request_and_response_components_from_schema() {
        let application = Router::new(Config::new(), "/items".POST(create_json))
            .openapi("/docs", OpenApi::new("Items", "1.0"));
        let document = application.openapi_document().unwrap().as_str();

        assert!(document.contains("\"application/json\":{\"schema\":{\"$ref\":"));
        assert!(document.contains("\"components\":{\"schemas\":"));
        serde_json::from_str::<serde_json::Value>(document).unwrap();
    }
}
