use std::marker::PhantomData;

use crate::{App, Handler, Method};

pub struct Route<H, Arguments, Modes> {
    path: &'static str,
    method: Method,
    handler: H,
    signature: PhantomData<fn() -> (Arguments, Modes)>,
}

pub trait RouteMethods {
    #[allow(non_snake_case)]
    fn GET<Arguments, Modes, H: Handler<Arguments, Modes>>(
        self,
        handler: H,
    ) -> Route<H, Arguments, Modes>;
}

impl RouteMethods for &'static str {
    #[allow(non_snake_case)]
    fn GET<Arguments, Modes, H: Handler<Arguments, Modes>>(
        self,
        handler: H,
    ) -> Route<H, Arguments, Modes> {
        Route {
            path: self,
            method: Method::new("GET"),
            handler,
            signature: PhantomData,
        }
    }
}

#[doc(hidden)]
pub trait Routes {
    fn apply(self, application: &mut App);
}

impl Routes for () {
    fn apply(self, _application: &mut App) {}
}

impl<Arguments: 'static, Modes: 'static, H: Handler<Arguments, Modes> + 'static> Routes
    for Route<H, Arguments, Modes>
{
    fn apply(self, application: &mut App) {
        application
            .router_mut()
            .register(self.method, self.path, self.handler);
    }
}

macro_rules! routes {
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

routes!(R1);
routes!(R1, R2);
routes!(R1, R2, R3);
routes!(R1, R2, R3, R4);
routes!(R1, R2, R3, R4, R5);
routes!(R1, R2, R3, R4, R5, R6);
routes!(R1, R2, R3, R4, R5, R6, R7);
routes!(R1, R2, R3, R4, R5, R6, R7, R8);
routes!(R1, R2, R3, R4, R5, R6, R7, R8, R9);
routes!(R1, R2, R3, R4, R5, R6, R7, R8, R9, R10);
routes!(R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11);
routes!(R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11, R12);

#[cfg(test)]
mod tests {
    use crate::{App, RouteMethods};

    async fn health() -> &'static str {
        "ok"
    }

    async fn version() -> &'static str {
        "0.1.0"
    }

    #[test]
    fn routes_can_be_registered_with_an_app() {
        let _application = App::new(("/health".GET(health), "/version".GET(version)));
    }
}
