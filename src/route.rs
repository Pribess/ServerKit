use std::{any::TypeId, marker::PhantomData};

use crate::{Handler, Method, Middleware, Router, middleware::MiddlewareEntry, openapi::Operation};

type OperationModifier = Box<dyn FnOnce(&mut Operation)>;

pub struct Route<H, Arguments, Input> {
    path: &'static str,
    method: Method,
    handler: H,
    operation_modifiers: Vec<OperationModifier>,
    middlewares: Vec<MiddlewareEntry>,
    excluded_middlewares: Vec<TypeId>,
    signature: PhantomData<fn() -> (Arguments, Input)>,
}

impl<H, Arguments, Input> Route<H, Arguments, Input> {
    pub fn summary(self, summary: impl Into<String>) -> Self {
        let summary = summary.into();
        self.openapi(move |operation| {
            operation.summary(summary);
        })
    }

    pub fn description(self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.openapi(move |operation| {
            operation.description(description);
        })
    }

    pub fn tag(self, tag: impl Into<String>) -> Self {
        let tag = tag.into();
        self.openapi(move |operation| {
            operation.tag(tag);
        })
    }

    pub fn operation_id(self, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.openapi(move |operation| {
            operation.operation_id(operation_id);
        })
    }

    pub fn openapi(mut self, modifier: impl FnOnce(&mut Operation) + 'static) -> Self {
        self.operation_modifiers.push(Box::new(modifier));
        self
    }

    pub fn middleware<M: Middleware>(mut self, middleware: M) -> Self {
        self.middlewares.push(MiddlewareEntry::new(middleware));
        self
    }

    pub fn without_middleware<M: Middleware>(mut self) -> Self {
        let type_id = TypeId::of::<M>();
        if !self.excluded_middlewares.contains(&type_id) {
            self.excluded_middlewares.push(type_id);
        }
        self
    }
}

macro_rules! route_methods {
    ($($method:ident),+ $(,)?) => {
        #[allow(non_snake_case)]
        pub trait RouteMethods {
            $(
                fn $method<Arguments, Input, H: Handler<Arguments, Input>>(
                    self,
                    handler: H,
                ) -> Route<H, Arguments, Input>;
            )+
        }

        #[allow(non_snake_case)]
        impl RouteMethods for &'static str {
            $(
                fn $method<Arguments, Input, H: Handler<Arguments, Input>>(
                    self,
                    handler: H,
                ) -> Route<H, Arguments, Input> {
                    Route {
                        path: self,
                        method: Method::new(stringify!($method)),
                        handler,
                        operation_modifiers: Vec::new(),
                        middlewares: Vec::new(),
                        excluded_middlewares: Vec::new(),
                        signature: PhantomData,
                    }
                }
            )+
        }
    };
}

route_methods!(GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS);

#[doc(hidden)]
pub trait Routes {
    fn apply(self, router: &mut Router);
}

impl Routes for () {
    fn apply(self, _router: &mut Router) {}
}

impl<Arguments: 'static, Input: 'static, H: Handler<Arguments, Input> + Send + Sync + 'static>
    Routes for Route<H, Arguments, Input>
{
    fn apply(self, router: &mut Router) {
        let mut operation = H::openapi();
        for modifier in self.operation_modifiers {
            modifier(&mut operation);
        }
        router.register(
            self.method,
            self.path,
            self.handler,
            operation,
            self.middlewares,
            self.excluded_middlewares,
        );
    }
}

impl Routes for Router {
    fn apply(self, router: &mut Router) {
        router.register_router(self);
    }
}

macro_rules! impl_route_tuple {
    ($($route:ident),+) => {
        impl<$($route: Routes),+> Routes for ($($route,)+) {
            #[allow(non_snake_case)]
            fn apply(self, router: &mut Router) {
                let ($($route,)+) = self;

                $($route.apply(router);)+
            }
        }
    };
}

serverkit_macros::impl_routes!(16);

#[cfg(test)]
mod tests {
    use crate::{Config, RouteMethods, Router};

    async fn health() -> &'static str {
        "ok"
    }

    async fn version() -> &'static str {
        "0.1.0"
    }

    async fn accepted() -> &'static str {
        "accepted"
    }

    #[test]
    fn routes_can_be_registered_with_an_app() {
        let _application = Router::new(
            Config::new(),
            (
                "/health".GET(health),
                "/version".GET(version),
                "/post".POST(accepted),
                "/put".PUT(accepted),
                "/patch".PATCH(accepted),
                "/delete".DELETE(accepted),
                "/head".HEAD(accepted),
                "/options".OPTIONS(accepted),
            ),
        );
    }

    #[test]
    fn generates_route_tuples_through_the_configured_maximum() {
        let _application = Router::new(
            Config::new(),
            (
                "/1".GET(health),
                "/2".GET(health),
                "/3".GET(health),
                "/4".GET(health),
                "/5".GET(health),
                "/6".GET(health),
                "/7".GET(health),
                "/8".GET(health),
                "/9".GET(health),
                "/10".GET(health),
                "/11".GET(health),
                "/12".GET(health),
                "/13".GET(health),
                "/14".GET(health),
                "/15".GET(health),
                "/16".GET(health),
            ),
        );
    }
}
