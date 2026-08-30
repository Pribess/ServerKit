use std::future::Future;

use crate::{
    FromRequest, IntoResponse, Request, Response,
    openapi::Operation,
    stream::{BufferedRequestStream, collect_stream},
};

pub trait Handler<Arguments, Input> {
    async fn call(&self, request: Request) -> Response;

    #[doc(hidden)]
    fn openapi() -> Operation;
}

impl<Output: IntoResponse, Fut: Future<Output = Output>, F: Fn() -> Fut> Handler<(), ()> for F {
    async fn call(&self, request: Request) -> Response {
        drop(request);
        self().await.into_response()
    }

    fn openapi() -> Operation {
        let mut operation = Operation::default();
        Output::openapi(&mut operation);
        operation.ensure_response();
        operation
    }
}

macro_rules! impl_handler {
    ([$(($argument:ident, $value:ident)),*]; ($last_argument:ident, $last_value:ident)) => {
        impl<
                $(
                    $argument: for<'request> FromRequest<(
                        &'request Request,
                        &'request [u8],
                    )>,
                )*
                $last_argument: for<'request> FromRequest<(
                    &'request Request,
                    &'request [u8],
                )>,
                Output: IntoResponse,
                Fut: Future<Output = Output>,
                F: Fn($($argument,)* $last_argument) -> Fut,
            > Handler<($($argument,)* $last_argument,), ()> for F
        {
            async fn call(&self, mut request: Request) -> Response {
                let has_buffered = false
                    $(|| <$argument as FromRequest<(&Request, &[u8])>>::BUFFERED)*
                    || <$last_argument as FromRequest<(&Request, &[u8])>>::BUFFERED;

                let buffered = if has_buffered {
                    let body_limit = request.body_limit();

                    match collect_stream(request.body.as_mut(), body_limit).await {
                        Ok(buffered) => buffered,
                        Err(error) => return error.into_response(),
                    }
                } else {
                    Vec::new()
                };

                $(
                    let $value = match <$argument as FromRequest<(
                        &Request,
                        &[u8],
                    )>>::from_request((&request, buffered.as_slice()))
                    .await
                    {
                        Ok(value) => value,
                        Err(error) => return error.into_response(),
                    };
                )*

                let $last_value = match <$last_argument as FromRequest<(
                    &Request,
                    &[u8],
                )>>::from_request((&request, buffered.as_slice()))
                .await
                {
                    Ok(value) => value,
                    Err(error) => return error.into_response(),
                };

                self($($value,)* $last_value).await.into_response()
            }

            fn openapi() -> Operation {
                let mut operation = Operation::default();
                $(
                    <$argument as FromRequest<(
                        &Request,
                        &[u8],
                    )>>::openapi(&mut operation);
                )*
                <$last_argument as FromRequest<(
                    &Request,
                    &[u8],
                )>>::openapi(&mut operation);
                Output::openapi(&mut operation);
                operation.ensure_response();
                operation
            }
        }

        impl<
                $(
                    $argument: for<'request> FromRequest<(
                        &'request Request,
                        &'request [u8],
                    )>,
                )*
                $last_argument: FromRequest<Request>,
                Output: IntoResponse,
                Fut: Future<Output = Output>,
                F: Fn($($argument,)* $last_argument) -> Fut,
            > Handler<($($argument,)* $last_argument,), Request> for F
        {
            async fn call(&self, mut request: Request) -> Response {
                let has_buffered = false
                    $(|| <$argument as FromRequest<(&Request, &[u8])>>::BUFFERED)*;

                let buffered = if has_buffered {
                    let body_limit = request.body_limit();

                    match collect_stream(request.body.as_mut(), body_limit).await {
                        Ok(buffered) => buffered,
                        Err(error) => return error.into_response(),
                    }
                } else {
                    Vec::new()
                };

                $(
                    let $value = match <$argument as FromRequest<(
                        &Request,
                        &[u8],
                    )>>::from_request((&request, buffered.as_slice()))
                    .await
                    {
                        Ok(value) => value,
                        Err(error) => return error.into_response(),
                    };
                )*

                if has_buffered {
                    request.body = Box::new(BufferedRequestStream::new(buffered));
                }

                let $last_value = match <$last_argument as FromRequest<Request>>::from_request(
                    request,
                )
                .await
                {
                    Ok(value) => value,
                    Err(error) => return error.into_response(),
                };

                self($($value,)* $last_value).await.into_response()
            }

            fn openapi() -> Operation {
                let mut operation = Operation::default();
                $(
                    <$argument as FromRequest<(
                        &Request,
                        &[u8],
                    )>>::openapi(&mut operation);
                )*
                <$last_argument as FromRequest<Request>>::openapi(&mut operation);
                Output::openapi(&mut operation);
                operation.ensure_response();
                operation
            }
        }
    };
}

serverkit_macros::impl_handlers!(16);

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        convert::Infallible,
        future::Future,
        rc::Rc,
        task::{Context, Poll, Waker},
    };

    use crate::{
        Body, Bytes, Config, Error, Extension, FromRequest, Handler, Headers, Method, Request,
        RequestStream, RouteMethods, Router, State, StreamError,
    };

    struct ProbeStream {
        body: Vec<u8>,
        sent: bool,
        polls: Rc<Cell<usize>>,
    }

    impl RequestStream for ProbeStream {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<(), StreamError>>> {
            self.polls.set(self.polls.get() + 1);

            if self.sent {
                Poll::Ready(None)
            } else {
                self.sent = true;
                Poll::Ready(Some(Ok(())))
            }
        }

        fn chunk(&self) -> &[u8] {
            &self.body
        }
    }

    struct BufferedBytes(Vec<u8>);

    struct StateBacked(String);

    impl<'request> FromRequest<(&'request Request, &'request [u8])> for StateBacked {
        type Error = Error;

        async fn from_request(
            input: (&'request Request, &'request [u8]),
        ) -> Result<Self, Self::Error> {
            let State(value) = State::<String>::from_request(input).await?;
            Ok(Self(value.as_str().to_owned()))
        }
    }

    impl<'request> FromRequest<(&'request Request, &'request [u8])> for BufferedBytes {
        type Error = Infallible;

        const BUFFERED: bool = true;

        async fn from_request(
            input: (&'request Request, &'request [u8]),
        ) -> Result<Self, Self::Error> {
            Ok(Self(input.1.to_vec()))
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

    fn request(body: &[u8], polls: Rc<Cell<usize>>) -> Request {
        Request::from_parts(
            Method::GET,
            "/",
            None,
            Headers::new(),
            Box::new(ProbeStream {
                body: body.to_vec(),
                sent: false,
                polls,
            }),
        )
    }

    async fn one(_a0: Method) {}

    async fn two(_a0: Method, _a1: Method) {}

    async fn stream_last(_a0: Method, _a1: Body) {}

    async fn leave_stream_unread(_body: Body) -> &'static str {
        "unread"
    }

    async fn buffered_then_stream(
        first: BufferedBytes,
        second: BufferedBytes,
        mut body: Body,
    ) -> Vec<u8> {
        assert_eq!(first.0, second.0);
        body.next().await.unwrap().unwrap().to_vec()
    }

    async fn buffered_body(Bytes(bytes): Bytes) -> Vec<u8> {
        bytes
    }

    async fn streaming_body(mut body: Body) -> Result<Vec<u8>, StreamError> {
        let mut bytes = Vec::new();

        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(chunk?);
        }

        Ok(bytes)
    }

    async fn application_state(State(value): State<String>) -> String {
        value.as_str().to_owned()
    }

    async fn request_extension(Extension(value): Extension<u64>) -> String {
        value.to_string()
    }

    async fn state_backed(StateBacked(value): StateBacked) -> String {
        value
    }

    #[allow(clippy::too_many_arguments)]
    async fn sixteen(
        _a0: Method,
        _a1: Method,
        _a2: Method,
        _a3: Method,
        _a4: Method,
        _a5: Method,
        _a6: Method,
        _a7: Method,
        _a8: Method,
        _a9: Method,
        _a10: Method,
        _a11: Method,
        _a12: Method,
        _a13: Method,
        _a14: Method,
        _a15: Method,
    ) {
    }

    fn assert_handler<Arguments, Input, H: Handler<Arguments, Input>>(_handler: H) {}

    #[test]
    fn implements_supported_arities() {
        assert_handler::<(Method,), (), _>(one);
        assert_handler::<(Method, Method), (), _>(two);
        assert_handler::<(Method, Body), Request, _>(stream_last);
        assert_handler::<
            (
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
                Method,
            ),
            (),
            _,
        >(sixteen);
    }

    #[test]
    fn streaming_only_does_not_preconsume_the_body() {
        let polls = Rc::new(Cell::new(0));
        let application = Router::new(Config::new(), ("/".GET(leave_stream_unread),));
        let response = block_on(application.handle(request(b"stream", Rc::clone(&polls))));

        assert_eq!(response.body(), b"unread");
        assert_eq!(polls.get(), 0);
    }

    #[test]
    fn buffered_extractors_share_one_collection_before_streaming() {
        let polls = Rc::new(Cell::new(0));
        let application = Router::new(Config::new(), ("/".GET(buffered_then_stream),));
        let response = block_on(application.handle(request(b"replayed", Rc::clone(&polls))));

        assert_eq!(response.body(), b"replayed");
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn body_limit_applies_to_buffered_and_streaming_extractors() {
        let buffered = Router::new(Config::new(), ("/".GET(buffered_body),)).body_limit(3);
        let response = block_on(buffered.handle(request(b"four", Rc::new(Cell::new(0)))));
        assert_eq!(response.status(), 413);

        let streaming = Router::new(Config::new(), ("/".GET(streaming_body),)).body_limit(3);
        let response = block_on(streaming.handle(request(b"four", Rc::new(Cell::new(0)))));
        assert_eq!(response.status(), 413);
    }

    #[test]
    fn extracts_application_state_and_request_extensions() {
        let application =
            Router::new(Config::new(), ("/".GET(application_state),)).state("ready".to_owned());
        let response = block_on(application.handle(request(b"", Rc::new(Cell::new(0)))));
        assert_eq!(response.body(), b"ready");

        let application = Router::new(Config::new(), ("/".GET(request_extension),));
        let mut request = request(b"", Rc::new(Cell::new(0)));
        request.insert_extension(42_u64);
        let response = block_on(application.handle(request));
        assert_eq!(response.body(), b"42");
    }

    #[test]
    fn nested_extractors_can_propagate_missing_values_into_error() {
        let application =
            Router::new(Config::new(), ("/".GET(state_backed),)).state("ready".to_owned());
        let response = block_on(application.handle(request(b"", Rc::new(Cell::new(0)))));
        assert_eq!(response.body(), b"ready");

        let application = Router::new(Config::new(), ("/".GET(state_backed),));
        let response = block_on(application.handle(request(b"", Rc::new(Cell::new(0)))));
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.body(),
            br#"{"error":{"code":"application.state.unavailable","message":"application state is unavailable","fields":[]}}"#,
        );
    }
}
