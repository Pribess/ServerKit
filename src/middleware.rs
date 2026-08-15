use std::{any::TypeId, future::Future, pin::Pin};

use crate::{Request, Response};

pub trait Middleware: Send + Sync + 'static {
    async fn handle(&self, request: Request, next: Next<'_>) -> Response;
}

pub(crate) type MiddlewareFuture<'request> = Pin<Box<dyn Future<Output = Response> + 'request>>;

pub(crate) struct MiddlewareEntry {
    type_id: TypeId,
    service: Box<dyn MiddlewareService>,
}

impl MiddlewareEntry {
    pub(crate) fn new<M: Middleware>(middleware: M) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            service: Box::new(middleware),
        }
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.type_id
    }
}

pub(crate) trait MiddlewareService: Send + Sync {
    fn call<'request>(
        &'request self,
        request: Request,
        next: Next<'request>,
    ) -> MiddlewareFuture<'request>;
}

impl<M: Middleware> MiddlewareService for M {
    fn call<'request>(
        &'request self,
        request: Request,
        next: Next<'request>,
    ) -> MiddlewareFuture<'request> {
        Box::pin(self.handle(request, next))
    }
}

pub struct Next<'next> {
    middlewares: &'next [&'next MiddlewareEntry],
    terminal: &'next dyn MiddlewareTerminal,
}

impl Next<'_> {
    pub async fn run(self, request: Request) -> Response {
        let Some((middleware, remaining)) = self.middlewares.split_first() else {
            return self.terminal.call(request).await;
        };

        middleware
            .service
            .call(
                request,
                Next {
                    middlewares: remaining,
                    terminal: self.terminal,
                },
            )
            .await
    }
}

pub(crate) trait MiddlewareTerminal {
    fn call(&self, request: Request) -> MiddlewareFuture<'_>;
}

pub(crate) async fn run(
    middlewares: &[&MiddlewareEntry],
    terminal: &dyn MiddlewareTerminal,
    request: Request,
) -> Response {
    Next {
        middlewares,
        terminal,
    }
    .run(request)
    .await
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Waker},
    };

    use crate::{
        Config, Headers, Method, Middleware, Next, Request, RequestStream, Response, RouteMethods,
        Router, StreamError,
    };

    struct EmptyStream;

    impl RequestStream for EmptyStream {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
            Poll::Ready(None)
        }
    }

    struct ParentMiddleware(Arc<Mutex<Vec<&'static str>>>);
    struct ChildMiddleware(Arc<Mutex<Vec<&'static str>>>);
    struct RouteMiddleware(Arc<Mutex<Vec<&'static str>>>);

    macro_rules! record_middleware {
        ($middleware:ident, $before:literal, $after:literal) => {
            impl Middleware for $middleware {
                async fn handle(&self, request: Request, next: Next<'_>) -> Response {
                    self.0.lock().unwrap().push($before);
                    let response = next.run(request).await;
                    self.0.lock().unwrap().push($after);
                    response
                }
            }
        };
    }

    record_middleware!(ParentMiddleware, "parent:before", "parent:after");
    record_middleware!(ChildMiddleware, "child:before", "child:after");
    record_middleware!(RouteMiddleware, "route:before", "route:after");

    struct Authentication(Arc<AtomicUsize>);

    impl Middleware for Authentication {
        async fn handle(&self, request: Request, next: Next<'_>) -> Response {
            self.0.fetch_add(1, Ordering::Relaxed);
            next.run(request).await
        }
    }

    fn request(path: &str) -> Request {
        Request::new(
            Method::new("GET"),
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
    fn runs_parent_child_and_route_middleware_in_scope_order() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handler_events = Arc::clone(&events);
        let child = Router::new(
            Config::new().prefix("/v1"),
            "/health"
                .GET(move || {
                    let events = Arc::clone(&handler_events);

                    async move {
                        events.lock().unwrap().push("handler");
                        "ok"
                    }
                })
                .middleware(RouteMiddleware(Arc::clone(&events))),
        )
        .middleware(ChildMiddleware(Arc::clone(&events)))
        .at("/mounted");
        let router = Router::new(Config::new().prefix("/parent"), ())
            .middleware(ParentMiddleware(Arc::clone(&events)))
            .route(child);

        let response = block_on(router.handle(request("/parent/mounted/v1/health")));

        assert_eq!(response.body(), b"ok");
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "parent:before",
                "child:before",
                "route:before",
                "handler",
                "route:after",
                "child:after",
                "parent:after",
            ],
        );
    }

    #[test]
    fn child_middleware_only_runs_inside_the_child_prefix() {
        let calls = Arc::new(AtomicUsize::new(0));
        let child = Router::new(
            Config::new().prefix("/api"),
            "/inside".GET(|| async { "in" }),
        )
        .middleware(Authentication(Arc::clone(&calls)));
        let router = Router::new(Config::new(), "/outside".GET(|| async { "out" })).route(child);

        assert_eq!(block_on(router.handle(request("/outside"))).body(), b"out",);
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        assert_eq!(
            block_on(router.handle(request("/api/inside"))).body(),
            b"in",
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        assert_eq!(
            block_on(router.handle(request("/api/missing"))).status(),
            404,
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        assert_eq!(block_on(router.handle(request("/missing"))).status(), 404,);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn route_can_exclude_inherited_middleware_by_type() {
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new(
            Config::new(),
            (
                "/private".GET(|| async { "private" }),
                "/public"
                    .GET(|| async { "public" })
                    .without_middleware::<Authentication>(),
            ),
        )
        .middleware(Authentication(Arc::clone(&calls)));

        assert_eq!(
            block_on(router.handle(request("/private"))).body(),
            b"private",
        );
        assert_eq!(
            block_on(router.handle(request("/public"))).body(),
            b"public",
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
