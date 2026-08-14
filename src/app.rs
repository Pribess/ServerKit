use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

use crate::{Handler, Listener, OpenApi, OpenApiDocument, Request, Response, Router, Routes};

pub struct App {
    router: Router,
    states: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    body_limit: Option<usize>,
    openapi: Option<PublishedOpenApi>,
}

struct PublishedOpenApi {
    path: String,
    configuration: OpenApi,
    document: OpenApiDocument,
    scalar_page: String,
}

impl App {
    pub fn new(routes: impl Routes) -> Self {
        let mut application = Self {
            router: Router::new(),
            states: HashMap::new(),
            body_limit: None,
            openapi: None,
        };

        routes.apply(&mut application);

        application
    }

    pub(crate) fn router_mut(&mut self) -> &mut Router {
        &mut self.router
    }

    pub fn route(mut self, routes: impl Routes) -> Self {
        routes.apply(&mut self);
        self.refresh_openapi();
        self
    }

    pub fn nest(mut self, prefix: &str, application: App) -> Self {
        self.router.nest(prefix, application.router);
        self.refresh_openapi();
        self
    }

    pub fn fallback<Arguments: 'static, Input: 'static>(
        mut self,
        handler: impl Handler<Arguments, Input> + Send + Sync + 'static,
    ) -> Self {
        self.router.set_fallback(handler);
        self.refresh_openapi();
        self
    }

    pub fn state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.states.insert(TypeId::of::<T>(), Arc::new(state));
        self
    }

    pub fn body_limit(mut self, limit: usize) -> Self {
        self.body_limit = Some(limit);
        self
    }

    pub fn openapi(mut self, path: impl Into<String>, configuration: OpenApi) -> Self {
        self.publish_openapi(path.into(), configuration);
        self
    }

    pub fn openapi_document(&self) -> Option<&OpenApiDocument> {
        self.openapi.as_ref().map(|published| &published.document)
    }

    pub fn run<L: Listener>(self, listener: L) -> L::Output {
        listener.serve(self)
    }

    pub async fn handle(&self, mut request: Request) -> Response {
        if let Some(response) = self.handle_openapi(&request) {
            return response;
        }

        request.set_states(self.states.clone());
        request.set_body_limit(self.body_limit);
        self.router.handle(request).await
    }

    fn publish_openapi(&mut self, path: String, configuration: OpenApi) {
        self.router.validate_openapi_path(&path);
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
        configuration.build(self.router.openapi_routes())
    }

    fn handle_openapi(&self, request: &Request) -> Option<Response> {
        let published = self.openapi.as_ref()?;

        if request.path() != published.path {
            return None;
        }

        let mut response = Response::bytes(200, published.scalar_page.as_bytes().to_vec());
        response.set_header("Content-Type", "text/html; charset=utf-8");

        let mut response = match request.method().as_str() {
            "GET" => response,
            "HEAD" => response.without_body(),
            "OPTIONS" => Response::empty(),
            _ => Response::text(405, "Method Not Allowed"),
        };
        response.set_header("Allow", "GET, HEAD, OPTIONS");
        Some(response)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use crate::{
        App, Form, Headers, Method, OpenApi, Path, Query, Request, RequestStream, RouteMethods,
        StreamError,
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
        ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
            Poll::Ready(None)
        }
    }

    async fn item(Path(path): Path<ItemPath>, Query(query): Query<SearchQuery>) -> String {
        format!("{}:{}", path.id, query.query)
    }

    async fn create(Form(item): Form<CreateItem>) -> String {
        item.name
    }

    #[cfg(feature = "json")]
    async fn create_json(crate::Json(item): crate::Json<JsonItem>) -> crate::Json<JsonItem> {
        crate::Json(item)
    }

    fn request(method: &str, path: &str) -> Request {
        Request::new(
            Method::new(method),
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
        let application = App::new((
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
        ))
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
    fn serves_the_openapi_reference_with_head_and_method_handling() {
        let application = App::new(()).openapi("/docs", OpenApi::new("Empty", "1.0"));
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
        let application = App::new("/items/:id".GET(item))
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

    #[cfg(feature = "json")]
    #[test]
    fn derives_json_request_and_response_components_from_schema() {
        let application =
            App::new("/items".POST(create_json)).openapi("/docs", OpenApi::new("Items", "1.0"));
        let document = application.openapi_document().unwrap().as_str();

        assert!(document.contains("\"application/json\":{\"schema\":{\"$ref\":"));
        assert!(document.contains("\"components\":{\"schemas\":"));
        serde_json::from_str::<serde_json::Value>(document).unwrap();
    }
}
