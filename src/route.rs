use std::marker::PhantomData;

use crate::{App, Handler, Method, openapi::Operation};

type OperationModifier = Box<dyn FnOnce(&mut Operation)>;

pub struct Route<H, Arguments, Input> {
    path: &'static str,
    method: Method,
    handler: H,
    operation_modifiers: Vec<OperationModifier>,
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
    fn apply(self, application: &mut App);
}

impl Routes for () {
    fn apply(self, _application: &mut App) {}
}

impl<Arguments: 'static, Input: 'static, H: Handler<Arguments, Input> + Send + Sync + 'static>
    Routes for Route<H, Arguments, Input>
{
    fn apply(self, application: &mut App) {
        let mut operation = H::openapi();
        for modifier in self.operation_modifiers {
            modifier(&mut operation);
        }
        application
            .router_mut()
            .register(self.method, self.path, self.handler, operation);
    }
}

macro_rules! impl_route_tuple {
    ($($route:ident),+) => {
        impl<$($route: Routes),+> Routes for ($($route,)+) {
            #[allow(non_snake_case)]
            fn apply(self, application: &mut App) {
                let ($($route,)+) = self;

                $($route.apply(application);)+
            }
        }
    };
}

serverkit_macros::impl_routes!(16);

#[cfg(test)]
mod tests {
    use crate::{App, RouteMethods};

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
        let _application = App::new((
            "/health".GET(health),
            "/version".GET(version),
            "/post".POST(accepted),
            "/put".PUT(accepted),
            "/patch".PATCH(accepted),
            "/delete".DELETE(accepted),
            "/head".HEAD(accepted),
            "/options".OPTIONS(accepted),
        ));
    }

    #[test]
    fn generates_route_tuples_through_the_configured_maximum() {
        let _application = App::new((
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
        ));
    }
}
