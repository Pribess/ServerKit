use std::{future::Future, marker::PhantomData, pin::Pin};

use crate::{Handler, Method, Request, RequestStream, Response};

type HandlerFuture<'handler> = Pin<Box<dyn Future<Output = Response> + 'handler>>;

trait ErasedHandler {
    fn call(&self, request: Request, stream: Box<dyn RequestStream>) -> HandlerFuture<'_>;
}

struct HandlerAdapter<H, Arguments, Modes> {
    handler: H,
    signature: PhantomData<fn() -> (Arguments, Modes)>,
}

impl<H, Arguments, Modes> HandlerAdapter<H, Arguments, Modes> {
    fn new(handler: H) -> Self {
        Self {
            handler,
            signature: PhantomData,
        }
    }
}

impl<Arguments, Modes, H: Handler<Arguments, Modes>> ErasedHandler
    for HandlerAdapter<H, Arguments, Modes>
{
    fn call(&self, request: Request, stream: Box<dyn RequestStream>) -> HandlerFuture<'_> {
        Box::pin(self.handler.call(request, stream))
    }
}

struct RegisteredRoute {
    method: Method,
    path: RoutePath,
    handler: Box<dyn ErasedHandler>,
}

struct RoutePath {
    source: &'static str,
    segments: Vec<RouteSegment>,
    static_segments: usize,
}

enum RouteSegment {
    Static(&'static str),
    Parameter(&'static str),
}

impl RoutePath {
    fn new(source: &'static str) -> Self {
        let segments = path_segments(source)
            .into_iter()
            .map(|segment| match segment.strip_prefix(':') {
                Some(name) if !name.is_empty() => RouteSegment::Parameter(name),
                _ => RouteSegment::Static(segment),
            })
            .collect::<Vec<_>>();
        let static_segments = segments
            .iter()
            .filter(|segment| matches!(segment, RouteSegment::Static(_)))
            .count();

        Self {
            source,
            segments,
            static_segments,
        }
    }

    fn is_static(&self) -> bool {
        self.static_segments == self.segments.len()
    }

    fn captures(&self, path: &str) -> Option<Vec<(&'static str, String)>> {
        let actual = path_segments(path);

        if self.segments.len() != actual.len() {
            return None;
        }

        let mut captures = Vec::new();

        for (expected, actual) in self.segments.iter().zip(actual) {
            match expected {
                RouteSegment::Static(expected) if *expected != actual => return None,
                RouteSegment::Static(_) => {}
                RouteSegment::Parameter(_) if actual.is_empty() => return None,
                RouteSegment::Parameter(name) => captures.push((*name, actual.to_owned())),
            }
        }

        Some(captures)
    }
}

fn path_segments(path: &str) -> Vec<&str> {
    if path == "/" {
        Vec::new()
    } else {
        path.strip_prefix('/').unwrap_or(path).split('/').collect()
    }
}

pub(crate) struct Router {
    routes: Vec<RegisteredRoute>,
}

impl Router {
    pub(crate) fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub(crate) fn register<
        Arguments: 'static,
        Modes: 'static,
        H: Handler<Arguments, Modes> + 'static,
    >(
        &mut self,
        method: Method,
        path: &'static str,
        handler: H,
    ) {
        self.routes.push(RegisteredRoute {
            method,
            path: RoutePath::new(path),
            handler: Box::new(HandlerAdapter::<H, Arguments, Modes>::new(handler)),
        });
    }

    pub(crate) async fn handle(
        &self,
        mut request: Request,
        stream: Box<dyn RequestStream>,
    ) -> Response {
        let exact = self.routes.iter().find(|route| {
            route.method == *request.method()
                && route.path.is_static()
                && route.path.source == request.path()
        });

        if let Some(route) = exact {
            return route.handler.call(request, stream).await;
        }

        let mut matched = None;

        for route in self
            .routes
            .iter()
            .filter(|route| route.method == *request.method() && !route.path.is_static())
        {
            let Some(captures) = route.path.captures(request.path()) else {
                continue;
            };

            if matched
                .as_ref()
                .is_none_or(|(matched_route, _): &(&RegisteredRoute, Vec<_>)| {
                    route.path.static_segments > matched_route.path.static_segments
                })
            {
                matched = Some((route, captures));
            }
        }

        let Some((route, captures)) = matched else {
            return Response::text(404, "Not Found");
        };

        request.set_path_parameters(captures);
        route.handler.call(request, stream).await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use crate::{
        App, Header, Headers, Method, Path, Query, Request, RequestStream, RouteMethods,
        StreamError,
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
    struct SearchQuery {
        q: String,
    }

    #[derive(crate::Schema)]
    struct RequestHeaders {
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

    async fn search(Query(query): Query<SearchQuery>) -> String {
        query.q
    }

    async fn authenticated(Header(headers): Header<RequestHeaders>) -> String {
        headers.authorization
    }

    fn request(path: &str) -> Request {
        Request::new(Method::new("GET"), path, None, Headers::new())
    }

    fn request_with(
        path: &str,
        query: Option<&str>,
        headers: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Request {
        let mut request_headers = Headers::new();

        for (name, value) in headers {
            request_headers.append(name, value);
        }

        Request::new(
            Method::new("GET"),
            path,
            query.map(str::to_owned),
            request_headers,
        )
    }

    #[test]
    fn dispatches_a_matching_route() {
        let application = App::new(("/health".GET(health),));
        let response = block_on(application.handle(request("/health"), Box::new(EmptyStream)));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"ok");
    }

    #[test]
    fn returns_not_found_for_an_unknown_route() {
        let application = App::new(("/health".GET(health),));
        let response = block_on(application.handle(request("/missing"), Box::new(EmptyStream)));

        assert_eq!(response.status(), 404);
        assert_eq!(response.body(), b"Not Found");
    }

    #[test]
    fn captures_parameters_at_any_segment() {
        let application = App::new((
            "/asdf/:id/asdd".GET(item),
            "/:organization/users/:id".GET(user),
        ));

        let response =
            block_on(application.handle(request("/asdf/42/asdd"), Box::new(EmptyStream)));
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"42");

        let response =
            block_on(application.handle(request("/acme/users/7"), Box::new(EmptyStream)));
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"acme:7");
    }

    #[test]
    fn prefers_static_routes_over_parameter_routes() {
        let application = App::new(("/items/:id".GET(item), "/items/new".GET(literal)));
        let response = block_on(application.handle(request("/items/new"), Box::new(EmptyStream)));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"literal");
    }

    #[test]
    fn query_extractor_rejects_unknown_fields() {
        let application = App::new(("/search".GET(search),));
        let response = block_on(application.handle(
            request_with("/search", Some("q=rust&debug=true"), []),
            Box::new(EmptyStream),
        ));

        assert_eq!(response.status(), 400);
        assert!(String::from_utf8_lossy(response.body()).contains("debug"));
    }

    #[test]
    fn header_extractor_allows_unknown_fields() {
        let application = App::new(("/authenticated".GET(authenticated),));
        let response = block_on(application.handle(
            request_with(
                "/authenticated",
                None,
                [("Authorization", "Bearer token"), ("Host", "example.com")],
            ),
            Box::new(EmptyStream),
        ));

        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"Bearer token");
    }
}
