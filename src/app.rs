use crate::{Listener, Request, RequestStream, Response, Router, Routes};

pub struct App {
    router: Router,
}

impl App {
    pub fn new(routes: impl Routes) -> Self {
        let mut application = Self {
            router: Router::new(),
        };

        routes.apply(&mut application);

        application
    }

    pub(crate) fn router_mut(&mut self) -> &mut Router {
        &mut self.router
    }

    pub fn run<L: Listener>(self, listener: L) -> L::Output {
        listener.serve(self)
    }

    pub(crate) async fn handle(
        &self,
        request: Request,
        stream: Box<dyn RequestStream>,
    ) -> Response {
        self.router.handle(request, stream).await
    }
}
