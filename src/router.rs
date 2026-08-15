use std::{
    any::{Any, TypeId},
    cmp::Ordering,
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};

use crate::{
    Handler, Method, Request, Response,
    middleware::{MiddlewareEntry, MiddlewareFuture, MiddlewareTerminal},
    openapi::{Operation, RouteDescription},
};

type HandlerFuture<'handler> = Pin<Box<dyn Future<Output = Response> + 'handler>>;

trait ErasedHandler: Send + Sync {
    fn call(&self, request: Request) -> HandlerFuture<'_>;
}

struct HandlerAdapter<H, Arguments, Input> {
    handler: H,
    signature: PhantomData<fn() -> (Arguments, Input)>,
}

impl<H, Arguments, Input> HandlerAdapter<H, Arguments, Input> {
    fn new(handler: H) -> Self {
        Self {
            handler,
            signature: PhantomData,
        }
    }
}

impl<Arguments, Input, H: Handler<Arguments, Input> + Send + Sync> ErasedHandler
    for HandlerAdapter<H, Arguments, Input>
{
    fn call(&self, request: Request) -> HandlerFuture<'_> {
        Box::pin(self.handler.call(request))
    }
}

pub(crate) struct RegisteredRoute {
    method: Method,
    path: RoutePath,
    handler: Box<dyn ErasedHandler>,
    operation: Operation,
    middlewares: Vec<MiddlewareEntry>,
    excluded_middlewares: Vec<TypeId>,
}

struct RoutePath {
    source: String,
    segments: Vec<RouteSegment>,
}

enum RouteSegment {
    Static(String),
    Parameter(String),
    Wildcard(String),
}

impl RoutePath {
    fn parse(source: impl Into<String>) -> Result<Self, String> {
        let source = source.into();

        if !source.starts_with('/') {
            return Err("route paths must start with `/`".to_owned());
        }

        if source.contains('?') || source.contains('#') {
            return Err("route paths cannot contain a query or fragment".to_owned());
        }

        if source.len() > 1 && source.ends_with('/') {
            return Err("route paths cannot end with `/`".to_owned());
        }

        if source.contains("//") {
            return Err("route paths cannot contain empty segments".to_owned());
        }

        let mut names = Vec::<String>::new();
        let raw_segments = path_segments(&source);
        let mut segments = Vec::with_capacity(raw_segments.len());

        for (index, segment) in raw_segments.into_iter().enumerate() {
            let parsed = if let Some(name) = segment.strip_prefix(':') {
                validate_parameter(name)?;
                register_parameter(&mut names, name)?;
                RouteSegment::Parameter(name.to_owned())
            } else if let Some(name) = segment.strip_prefix('*') {
                validate_parameter(name)?;
                register_parameter(&mut names, name)?;

                if index + 1 != path_segments(&source).len() {
                    return Err("a wildcard must be the final route segment".to_owned());
                }

                RouteSegment::Wildcard(name.to_owned())
            } else {
                if segment.starts_with(':') || segment.starts_with('*') {
                    return Err("route parameter names cannot be empty".to_owned());
                }

                RouteSegment::Static(segment.to_owned())
            };

            segments.push(parsed);
        }

        Ok(Self { source, segments })
    }

    fn captures(&self, path: &str) -> Option<Vec<(String, String)>> {
        let actual = path_segments(path);
        let mut captures = Vec::new();
        let mut index = 0;

        for expected in &self.segments {
            match expected {
                RouteSegment::Static(expected) => {
                    if actual.get(index).copied() != Some(expected.as_str()) {
                        return None;
                    }
                    index += 1;
                }
                RouteSegment::Parameter(name) => {
                    let value = actual.get(index)?;

                    if value.is_empty() {
                        return None;
                    }

                    captures.push((name.clone(), (*value).to_owned()));
                    index += 1;
                }
                RouteSegment::Wildcard(name) => {
                    captures.push((name.clone(), actual[index..].join("/")));
                    index = actual.len();
                    break;
                }
            }
        }

        (index == actual.len()).then_some(captures)
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        self.segments.len() == other.segments.len()
            && self
                .segments
                .iter()
                .zip(&other.segments)
                .all(|(left, right)| match (left, right) {
                    (RouteSegment::Static(left), RouteSegment::Static(right)) => left == right,
                    (RouteSegment::Parameter(_), RouteSegment::Parameter(_))
                    | (RouteSegment::Wildcard(_), RouteSegment::Wildcard(_)) => true,
                    _ => false,
                })
    }

    fn specificity(&self, other: &Self) -> Ordering {
        let maximum = self.segments.len().max(other.segments.len());

        for index in 0..=maximum {
            let left = segment_priority(self.segments.get(index));
            let right = segment_priority(other.segments.get(index));

            match left.cmp(&right) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }

        Ordering::Equal
    }

    fn openapi_path(&self) -> (String, Vec<String>) {
        if self.segments.is_empty() {
            return ("/".to_owned(), Vec::new());
        }

        let mut path = String::new();
        let mut parameters = Vec::new();

        for segment in &self.segments {
            path.push('/');

            match segment {
                RouteSegment::Static(value) => path.push_str(value),
                RouteSegment::Parameter(name) | RouteSegment::Wildcard(name) => {
                    path.push('{');
                    path.push_str(name);
                    path.push('}');
                    parameters.push(name.clone());
                }
            }
        }

        (path, parameters)
    }
}

fn validate_parameter(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("route parameter names cannot be empty".to_owned());
    }

    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!("invalid route parameter name `{name}`"));
    }

    Ok(())
}

fn register_parameter(names: &mut Vec<String>, name: &str) -> Result<(), String> {
    if names.iter().any(|existing| existing == name) {
        return Err(format!("duplicate route parameter `{name}`"));
    }

    names.push(name.to_owned());
    Ok(())
}

fn segment_priority(segment: Option<&RouteSegment>) -> u8 {
    match segment {
        None => 3,
        Some(RouteSegment::Static(_)) => 2,
        Some(RouteSegment::Parameter(_)) => 1,
        Some(RouteSegment::Wildcard(_)) => 0,
    }
}

fn path_segments(path: &str) -> Vec<&str> {
    if path == "/" {
        Vec::new()
    } else {
        path.strip_prefix('/').unwrap_or(path).split('/').collect()
    }
}

pub(crate) struct Dispatcher {
    routes: Vec<RegisteredRoute>,
    fallbacks: Vec<RegisteredFallback>,
    scopes: Vec<Scope>,
}

pub(crate) struct Scope {
    prefix: String,
    middlewares: Vec<MiddlewareEntry>,
    states: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    body_limit: Option<usize>,
}

impl Scope {
    pub(crate) fn new(prefix: String) -> Self {
        Self {
            prefix,
            middlewares: Vec::new(),
            states: HashMap::new(),
            body_limit: None,
        }
    }

    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    pub(crate) fn middlewares(&self) -> &[MiddlewareEntry] {
        &self.middlewares
    }

    pub(crate) fn states(&self) -> &HashMap<TypeId, Arc<dyn Any + Send + Sync>> {
        &self.states
    }

    pub(crate) fn configured_body_limit(&self) -> Option<usize> {
        self.body_limit
    }

    pub(crate) fn middleware(&mut self, middleware: MiddlewareEntry) {
        self.middlewares.push(middleware);
    }

    pub(crate) fn state<T: Send + Sync + 'static>(&mut self, state: T) {
        self.states.insert(TypeId::of::<T>(), Arc::new(state));
    }

    pub(crate) fn body_limit(&mut self, limit: usize) {
        self.body_limit = Some(limit);
    }

    pub(crate) fn matches(&self, path: &str) -> bool {
        prefix_matches(&self.prefix, path)
    }

    pub(crate) fn prepend(&mut self, prefix: &str) {
        self.prefix = join_prefixes(prefix, &self.prefix);
    }
}

pub(crate) struct RegisteredFallback {
    prefix: String,
    handler: Box<dyn ErasedHandler>,
}

impl Dispatcher {
    pub(crate) fn new() -> Self {
        Self {
            routes: Vec::new(),
            fallbacks: Vec::new(),
            scopes: Vec::new(),
        }
    }

    pub(crate) fn register<
        Arguments: 'static,
        Input: 'static,
        H: Handler<Arguments, Input> + Send + Sync + 'static,
    >(
        &mut self,
        method: Method,
        path: impl Into<String>,
        handler: H,
        operation: Operation,
        middlewares: Vec<MiddlewareEntry>,
        excluded_middlewares: Vec<TypeId>,
    ) {
        let path = path.into();
        let path = RoutePath::parse(&path)
            .unwrap_or_else(|error| panic!("invalid route `{path}`: {error}"));

        if self
            .routes
            .iter()
            .any(|route| route.method == method && route.path.conflicts_with(&path))
        {
            panic!("conflicting {} route `{}`", method.as_str(), path.source,);
        }

        self.routes.push(RegisteredRoute {
            method,
            path,
            operation,
            middlewares,
            excluded_middlewares,
            handler: Box::new(HandlerAdapter::<H, Arguments, Input>::new(handler)),
        });
    }

    pub(crate) fn set_fallback<
        Arguments: 'static,
        Input: 'static,
        H: Handler<Arguments, Input> + Send + Sync + 'static,
    >(
        &mut self,
        prefix: &str,
        handler: H,
    ) {
        if self
            .fallbacks
            .iter()
            .any(|fallback| fallback.prefix == prefix)
        {
            panic!("duplicate fallback for `{prefix}`");
        }

        self.fallbacks.push(RegisteredFallback {
            prefix: prefix.to_owned(),
            handler: Box::new(HandlerAdapter::<H, Arguments, Input>::new(handler)),
        });
    }

    pub(crate) fn merge(&mut self, dispatcher: Dispatcher) {
        for route in dispatcher.routes {
            if self.routes.iter().any(|existing| {
                existing.method == route.method && existing.path.conflicts_with(&route.path)
            }) {
                panic!(
                    "conflicting {} route `{}`",
                    route.method.as_str(),
                    route.path.source,
                );
            }

            self.routes.push(route);
        }

        for fallback in dispatcher.fallbacks {
            if self
                .fallbacks
                .iter()
                .any(|existing| existing.prefix == fallback.prefix)
            {
                panic!("duplicate fallback for `{}`", fallback.prefix);
            }

            self.fallbacks.push(fallback);
        }

        self.scopes.extend(dispatcher.scopes);
    }

    pub(crate) fn prepend(&mut self, prefix: &str) {
        validate_scope_prefix(prefix)
            .unwrap_or_else(|error| panic!("invalid Router prefix `{prefix}`: {error}"));

        for route in &mut self.routes {
            let source = join_paths(prefix, &route.path.source);
            route.path = RoutePath::parse(&source)
                .unwrap_or_else(|error| panic!("invalid scoped route `{source}`: {error}"));
        }

        for fallback in &mut self.fallbacks {
            fallback.prefix = join_prefixes(prefix, &fallback.prefix);
        }

        for scope in &mut self.scopes {
            scope.prepend(prefix);
        }
    }

    pub(crate) fn add_scope(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    pub(crate) fn resolve(&self, request: &Request) -> Dispatch<'_> {
        let mut matched = Vec::new();
        let mut best_path: Option<&RoutePath> = None;

        for route in &self.routes {
            let Some(captures) = route.path.captures(request.path()) else {
                continue;
            };

            match best_path {
                Some(current) if route.path.specificity(current) == Ordering::Less => continue,
                Some(current) if route.path.specificity(current) == Ordering::Equal => {}
                _ => {
                    best_path = Some(&route.path);
                    matched.clear();
                }
            }

            matched.push((route, captures));
        }

        if matched.is_empty() {
            if let Some(fallback) = self.fallback(request.path()) {
                return Dispatch::Fallback(fallback);
            }

            return Dispatch::NotFound;
        }

        let allow = allow_header(&matched);
        let requested = request.method().as_str();
        let is_head = requested == "HEAD";

        if requested == "OPTIONS" {
            if let Some(index) = route_index(&matched, "OPTIONS") {
                let (route, captures) = matched.swap_remove(index);
                return Dispatch::Route {
                    route,
                    captures,
                    head: false,
                    allow: Some(allow),
                };
            }

            return Dispatch::Options { allow };
        }

        let selected = if is_head {
            route_index(&matched, "HEAD").or_else(|| route_index(&matched, "GET"))
        } else {
            route_index(&matched, requested)
        };

        let Some(index) = selected else {
            return Dispatch::MethodNotAllowed { allow };
        };

        let (route, captures) = matched.swap_remove(index);

        Dispatch::Route {
            route,
            captures,
            head: is_head,
            allow: None,
        }
    }

    pub(crate) fn matching_scopes(&self, path: &str) -> Vec<&Scope> {
        let mut scopes = self
            .scopes
            .iter()
            .filter(|scope| prefix_matches(&scope.prefix, path))
            .collect::<Vec<_>>();
        scopes.sort_by_key(|scope| scope.prefix.len());
        scopes
    }

    fn fallback(&self, path: &str) -> Option<&RegisteredFallback> {
        self.fallbacks
            .iter()
            .filter(|fallback| prefix_matches(&fallback.prefix, path))
            .max_by_key(|fallback| fallback.prefix.len())
    }

    pub(crate) fn openapi_routes(&self) -> Vec<RouteDescription> {
        self.routes
            .iter()
            .map(|route| {
                let (path, path_parameters) = route.path.openapi_path();

                RouteDescription {
                    path,
                    method: route.method.as_str().to_owned(),
                    path_parameters,
                    operation: route.operation.clone(),
                }
            })
            .collect()
    }

    pub(crate) fn validate_openapi_path(&self, path: &str) {
        self.validate_reserved_path(path, "OpenAPI reference");
    }

    fn validate_reserved_path(&self, path: &str, name: &str) {
        let parsed = RoutePath::parse(path)
            .unwrap_or_else(|error| panic!("invalid {name} path `{path}`: {error}"));

        if parsed
            .segments
            .iter()
            .any(|segment| !matches!(segment, RouteSegment::Static(_)))
        {
            panic!("the {name} path must be static");
        }

        if self
            .routes
            .iter()
            .any(|route| route.path.conflicts_with(&parsed))
        {
            panic!("the {name} path conflicts with an existing route");
        }
    }
}

pub(crate) enum Dispatch<'dispatcher> {
    Route {
        route: &'dispatcher RegisteredRoute,
        captures: Vec<(String, String)>,
        head: bool,
        allow: Option<String>,
    },
    Fallback(&'dispatcher RegisteredFallback),
    NotFound,
    MethodNotAllowed {
        allow: String,
    },
    Options {
        allow: String,
    },
}

impl Dispatch<'_> {
    pub(crate) fn excluded_middlewares(&self) -> &[TypeId] {
        match self {
            Self::Route { route, .. } => &route.excluded_middlewares,
            _ => &[],
        }
    }

    pub(crate) fn route_middlewares(&self) -> &[MiddlewareEntry] {
        match self {
            Self::Route { route, .. } => &route.middlewares,
            _ => &[],
        }
    }
}

impl MiddlewareTerminal for Dispatch<'_> {
    fn call(&self, mut request: Request) -> MiddlewareFuture<'_> {
        Box::pin(async move {
            match self {
                Self::Route {
                    route,
                    captures,
                    head,
                    allow,
                } => {
                    request.set_params(captures.clone());
                    let mut response = route.handler.call(request).await;

                    if let Some(allow) = allow {
                        response.set_header("Allow", allow.clone());
                    }

                    if *head {
                        response.without_body()
                    } else {
                        response
                    }
                }
                Self::Fallback(fallback) => fallback.handler.call(request).await,
                Self::NotFound => Response::text(404, "Not Found"),
                Self::MethodNotAllowed { allow } => {
                    let mut response = Response::text(405, "Method Not Allowed");
                    response.set_header("Allow", allow.clone());
                    response
                }
                Self::Options { allow } => {
                    let mut response = Response::empty();
                    response.set_header("Allow", allow.clone());
                    response
                }
            }
        })
    }
}

fn route_index(
    routes: &[(&RegisteredRoute, Vec<(String, String)>)],
    method: &str,
) -> Option<usize> {
    routes
        .iter()
        .position(|(route, _)| route.method.as_str() == method)
}

fn allow_header(routes: &[(&RegisteredRoute, Vec<(String, String)>)]) -> String {
    const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

    METHODS
        .into_iter()
        .filter(|method| match *method {
            "HEAD" => route_index(routes, "HEAD").is_some() || route_index(routes, "GET").is_some(),
            "OPTIONS" => true,
            method => route_index(routes, method).is_some(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn validate_scope_prefix(prefix: &str) -> Result<(), String> {
    if prefix.is_empty() || prefix == "/" {
        return Ok(());
    }

    let path = RoutePath::parse(prefix)?;

    if path
        .segments
        .iter()
        .any(|segment| !matches!(segment, RouteSegment::Static(_)))
    {
        return Err("Router prefixes must contain only static segments".to_owned());
    }

    Ok(())
}

pub(crate) fn join_paths(prefix: &str, path: &str) -> String {
    if prefix == "/" || prefix.is_empty() {
        path.to_owned()
    } else if path == "/" {
        prefix.to_owned()
    } else {
        format!("{prefix}{path}")
    }
}

pub(crate) fn join_prefixes(prefix: &str, nested: &str) -> String {
    match (prefix, nested) {
        ("/", "") => String::new(),
        (_, "") => prefix.to_owned(),
        _ => join_paths(prefix, nested),
    }
}

fn prefix_matches(prefix: &str, path: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remaining| remaining.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use crate::{
        Config, ExtraFields, Header, Headers, Method, Path, Query, Request, RequestStream,
        RouteMethods, Router, StreamError,
    };

    #[derive(crate::Schema)]
    struct ItemPath {
        id: u64,
    }

    #[derive(crate::Schema)]
    struct UserPath {
        organization: String,
        id: u64,
    }

    #[derive(crate::Schema)]
    struct WildcardPath {
        path: String,
    }

    #[derive(crate::Schema)]
    struct SearchQuery {
        q: String,
    }

    #[derive(crate::Schema)]
    #[schema(unknown_fields = "reject")]
    struct StrictSearchQuery {
        q: String,
    }

    #[derive(crate::Schema)]
    #[schema(unknown_fields = "ignore")]
    struct FlexibleItemPath {
        id: u64,
    }

    #[derive(crate::Schema)]
    struct CapturedQuery {
        q: String,
        #[schema(rest)]
        extra: ExtraFields,
    }

    #[derive(crate::Schema)]
    struct RequestHeaders {
        authorization: String,
    }

    #[derive(crate::Schema)]
    #[schema(unknown_fields = "reject")]
    struct StrictRequestHeaders {
        authorization: String,
    }

    struct EmptyStream;

    impl RequestStream for EmptyStream {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
            Poll::Ready(None)
        }
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

    async fn health() -> &'static str {
        "ok"
    }

    async fn item(Path(path): Path<ItemPath>) -> String {
        path.id.to_string()
    }

    async fn user(Path(path): Path<UserPath>) -> String {
        format!("{}:{}", path.organization, path.id)
    }

    async fn literal() -> &'static str {
        "literal"
    }

    async fn wildcard(Path(path): Path<WildcardPath>) -> String {
        path.path
    }

    async fn fallback() -> &'static str {
        "fallback"
    }

    async fn nested_fallback() -> &'static str {
        "nested-fallback"
    }

    async fn search(Query(query): Query<SearchQuery>) -> String {
        query.q
    }

    async fn strict_search(Query(query): Query<StrictSearchQuery>) -> String {
        query.q
    }

    async fn flexible_item(Path(path): Path<FlexibleItemPath>) -> String {
        path.id.to_string()
    }

    async fn captured_query(Query(query): Query<CapturedQuery>) -> String {
        format!(
            "{}:{}:{}",
            query.q,
            query.extra.get("debug").is_some(),
            query.extra.get_all("tag").count(),
        )
    }

    async fn authenticated(Header(headers): Header<RequestHeaders>) -> String {
        headers.authorization
    }

    async fn strictly_authenticated(Header(headers): Header<StrictRequestHeaders>) -> String {
        headers.authorization
    }

    async fn get_method() -> &'static str {
        "get"
    }

    async fn post_method() -> &'static str {
        "post"
    }

    async fn put_method() -> &'static str {
        "put"
    }

    async fn patch_method() -> &'static str {
        "patch"
    }

    async fn delete_method() -> &'static str {
        "delete"
    }

    async fn explicit_head() -> crate::Response {
        crate::Response::text(201, "explicit-head")
    }

    async fn explicit_options() -> crate::Response {
        crate::Response::text(202, "explicit-options")
    }

    fn request(path: &str) -> Request {
        request_with_method("GET", path)
    }

    fn request_with_method(method: &str, path: &str) -> Request {
        Request::new(
            Method::new(method),
            path,
            None,
            Headers::new(),
            Box::new(EmptyStream),
        )
    }

    fn request_with(
        path: &str,
        query: Option<&str>,
        headers: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Request {
        let mut request_headers = Headers::new();

        for (name, value) in headers {
            request_headers.append(name, value).unwrap();
        }

        Request::new(
            Method::new("GET"),
            path,
            query.map(str::to_owned),
            request_headers,
            Box::new(EmptyStream),
        )
    }

    #[test]
    fn dispatches_a_matching_route() {
        let application = Router::new(Config::new(), ("/health".GET(health),));
        let response = block_on(application.handle(request("/health")));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"ok");
    }

    #[test]
    fn returns_not_found_for_an_unknown_route() {
        let application = Router::new(Config::new(), ("/health".GET(health),));
        let response = block_on(application.handle(request("/missing")));

        assert_eq!(response.status(), 404);
        assert_eq!(response.body(), b"Not Found");
    }

    #[test]
    fn captures_parameters_at_any_segment() {
        let application = Router::new(
            Config::new(),
            (
                "/asdf/:id/asdd".GET(item),
                "/:organization/users/:id".GET(user),
            ),
        );

        let response = block_on(application.handle(request("/asdf/42/asdd")));
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"42");

        let response = block_on(application.handle(request("/acme/users/7")));
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"acme:7");
    }

    #[test]
    fn prefers_static_routes_over_parameter_routes() {
        let application = Router::new(
            Config::new(),
            ("/items/:id".GET(item), "/items/new".GET(literal)),
        );
        let response = block_on(application.handle(request("/items/new")));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"literal");
    }

    #[test]
    fn query_extractor_ignores_unknown_fields_by_default() {
        let application = Router::new(Config::new(), ("/search".GET(search),));
        let response =
            block_on(application.handle(request_with("/search", Some("q=rust&debug=true"), [])));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"rust");
    }

    #[test]
    fn query_schema_can_reject_unknown_fields() {
        let application = Router::new(Config::new(), ("/search".GET(strict_search),));
        let response =
            block_on(application.handle(request_with("/search", Some("q=rust&debug=true"), [])));

        assert_eq!(response.status(), 400);
        assert!(String::from_utf8_lossy(response.body()).contains("debug"));
    }

    #[test]
    fn path_rejects_unknown_fields_by_default_and_can_ignore_them() {
        let strict = Router::new(Config::new(), ("/:organization/items/:id".GET(item),));
        let response = block_on(strict.handle(request("/acme/items/42")));

        assert_eq!(response.status(), 400);
        assert!(String::from_utf8_lossy(response.body()).contains("organization"));

        let flexible = Router::new(
            Config::new(),
            ("/:organization/items/:id".GET(flexible_item),),
        );
        let response = block_on(flexible.handle(request("/acme/items/42")));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"42");
    }

    #[test]
    fn query_schema_can_capture_unknown_fields() {
        let application = Router::new(Config::new(), ("/search".GET(captured_query),));
        let response = block_on(application.handle(request_with(
            "/search",
            Some("q=rust&debug=true&tag=web&tag=server"),
            [],
        )));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"rust:true:2");
    }

    #[test]
    fn header_extractor_allows_unknown_fields() {
        let application = Router::new(Config::new(), ("/authenticated".GET(authenticated),));
        let response = block_on(application.handle(request_with(
            "/authenticated",
            None,
            [("Authorization", "Bearer token"), ("Host", "example.com")],
        )));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"Bearer token");
    }

    #[test]
    fn header_schema_can_reject_unknown_fields() {
        let application = Router::new(
            Config::new(),
            ("/authenticated".GET(strictly_authenticated),),
        );
        let response = block_on(application.handle(request_with(
            "/authenticated",
            None,
            [("Authorization", "Bearer token"), ("Host", "example.com")],
        )));

        assert_eq!(response.status(), 400);
        assert!(String::from_utf8_lossy(response.body()).contains("Host"));
    }

    #[test]
    fn dispatches_each_method_on_the_same_path() {
        let application = Router::new(
            Config::new(),
            (
                "/resource".GET(get_method),
                "/resource".POST(post_method),
                "/resource".PUT(put_method),
                "/resource".PATCH(patch_method),
                "/resource".DELETE(delete_method),
            ),
        );

        for (method, expected) in [
            ("GET", b"get".as_slice()),
            ("POST", b"post".as_slice()),
            ("PUT", b"put".as_slice()),
            ("PATCH", b"patch".as_slice()),
            ("DELETE", b"delete".as_slice()),
        ] {
            let response = block_on(application.handle(request_with_method(method, "/resource")));

            assert_eq!(response.status(), 200);
            assert_eq!(response.body(), expected);
        }
    }

    #[test]
    fn returns_method_not_allowed_with_allow_header() {
        let application = Router::new(
            Config::new(),
            ("/resource".GET(get_method), "/resource".POST(post_method)),
        );
        let response = block_on(application.handle(request_with_method("DELETE", "/resource")));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 405);
        assert_eq!(
            headers.get("allow"),
            Some(b"GET, HEAD, POST, OPTIONS".as_slice())
        );
        assert_eq!(body.buffered(), Some(b"Method Not Allowed".as_slice()));
    }

    #[test]
    fn head_uses_get_without_returning_its_body() {
        let application = Router::new(Config::new(), ("/resource".GET(get_method),));
        let response = block_on(application.handle(request_with_method("HEAD", "/resource")));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 200);
        assert_eq!(
            headers.get("content-type"),
            Some(b"text/plain; charset=utf-8".as_slice()),
        );
        assert_eq!(headers.get("content-length"), Some(b"3".as_slice()));
        assert_eq!(body.buffered(), Some(b"".as_slice()));
    }

    #[test]
    fn explicit_head_takes_precedence_over_get() {
        let application = Router::new(
            Config::new(),
            ("/resource".GET(get_method), "/resource".HEAD(explicit_head)),
        );
        let response = block_on(application.handle(request_with_method("HEAD", "/resource")));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 201);
        assert_eq!(headers.get("content-length"), Some(b"13".as_slice()));
        assert_eq!(body.buffered(), Some(b"".as_slice()));
    }

    #[test]
    fn options_is_generated_unless_an_explicit_route_exists() {
        let generated = Router::new(
            Config::new(),
            ("/resource".GET(get_method), "/resource".PATCH(patch_method)),
        );
        let response = block_on(generated.handle(request_with_method("OPTIONS", "/resource")));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 204);
        assert_eq!(
            headers.get("allow"),
            Some(b"GET, HEAD, PATCH, OPTIONS".as_slice())
        );
        assert_eq!(body.buffered(), Some(b"".as_slice()));

        let explicit = Router::new(Config::new(), ("/resource".OPTIONS(explicit_options),));
        let response = block_on(explicit.handle(request_with_method("OPTIONS", "/resource")));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 202);
        assert_eq!(headers.get("allow"), Some(b"OPTIONS".as_slice()));
        assert_eq!(body.buffered(), Some(b"explicit-options".as_slice()));
    }

    #[test]
    fn method_matching_respects_static_route_precedence() {
        let application = Router::new(
            Config::new(),
            ("/items/new".GET(get_method), "/items/:id".POST(post_method)),
        );
        let response = block_on(application.handle(request_with_method("POST", "/items/new")));
        let (status, headers, _) = response.into_parts();

        assert_eq!(status, 405);
        assert_eq!(headers.get("allow"), Some(b"GET, HEAD, OPTIONS".as_slice()));
    }

    #[test]
    fn chooses_specificity_from_left_to_right() {
        let application = Router::new(
            Config::new(),
            (
                "/:section/settings".GET(health),
                "/users/:page".GET(literal),
            ),
        );
        let response = block_on(application.handle(request("/users/settings")));

        assert_eq!(response.body(), b"literal");
    }

    #[test]
    fn wildcard_captures_the_remaining_path() {
        let application = Router::new(Config::new(), ("/assets/*path".GET(wildcard),));
        let response = block_on(application.handle(request("/assets/css/app.css")));

        assert_eq!(response.body(), b"css/app.css");
    }

    #[test]
    fn static_and_parameter_routes_precede_wildcards() {
        let application = Router::new(
            Config::new(),
            (
                "/files/*path".GET(wildcard),
                "/files/:path".GET(item),
                "/files/new".GET(literal),
            ),
        );

        let response = block_on(application.handle(request("/files/new")));
        assert_eq!(response.body(), b"literal");
    }

    #[test]
    fn route_adds_routes_without_a_tuple_limit() {
        let application = Router::new(Config::new(), ())
            .route("/one".GET(health))
            .route("/two".GET(literal));

        assert_eq!(
            block_on(application.handle(request("/two"))).body(),
            b"literal",
        );
    }

    #[test]
    fn routes_a_router_with_parent_mount_and_config_prefixes() {
        let api = Router::new(Config::new().prefix("/v1"), ("/health".GET(health),)).at("/mounted");
        let application = Router::new(Config::new().prefix("/parent"), ()).route(api);

        assert_eq!(
            block_on(application.handle(request("/parent/mounted/v1/health"))).body(),
            b"ok",
        );
        assert_eq!(
            block_on(application.handle(request("/parent/v1/mounted/health"))).status(),
            404,
        );
    }

    #[test]
    fn fallback_handles_unmatched_paths() {
        let application = Router::new(Config::new(), ("/health".GET(health),)).fallback(fallback);

        assert_eq!(
            block_on(application.handle(request("/missing"))).body(),
            b"fallback",
        );
    }

    #[test]
    fn child_fallback_only_handles_paths_inside_its_prefix() {
        let child = Router::new(Config::new().prefix("/api"), ()).fallback(nested_fallback);
        let router = Router::new(Config::new(), ())
            .fallback(fallback)
            .route(child);

        assert_eq!(
            block_on(router.handle(request("/api/missing"))).body(),
            b"nested-fallback",
        );
        assert_eq!(
            block_on(router.handle(request("/missing"))).body(),
            b"fallback",
        );
    }

    #[test]
    #[should_panic(expected = "conflicting GET route")]
    fn rejects_equivalent_parameter_routes() {
        let _application = Router::new(
            Config::new(),
            ("/users/:id".GET(health), "/users/:name".GET(literal)),
        );
    }

    #[test]
    #[should_panic(expected = "wildcard must be the final")]
    fn rejects_non_terminal_wildcards() {
        let _application = Router::new(Config::new(), ("/assets/*path/edit".GET(health),));
    }
}
