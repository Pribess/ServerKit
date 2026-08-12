use std::{future::Future, sync::Arc};

use crate::{
    IntoResponse, Request, RequestStream, Response,
    extract::{Buffered, FromRequest, Input, Mode, Streaming, Unused},
    stream::{BufferedRequestStream, collect_stream},
};

pub(crate) trait ResolveRequest<M: Mode, State>: FromRequest<M> {
    type Next;

    async fn resolve(
        request: &Request,
        buffered: &[u8],
        state: State,
    ) -> Result<(Self, Self::Next), Response>;
}

impl<A: FromRequest<Unused>, State> ResolveRequest<Unused, State> for A {
    type Next = State;

    async fn resolve(
        request: &Request,
        _buffered: &[u8],
        state: State,
    ) -> Result<(Self, Self::Next), Response> {
        let value = <A as FromRequest<Unused>>::from_request(Input::<Unused>::new(request, ()))
            .await
            .map_err(IntoResponse::into_response)?;

        Ok((value, state))
    }
}

impl<A: FromRequest<Buffered>, State> ResolveRequest<Buffered, State> for A {
    type Next = State;

    async fn resolve(
        request: &Request,
        buffered: &[u8],
        state: State,
    ) -> Result<(Self, Self::Next), Response> {
        let value =
            <A as FromRequest<Buffered>>::from_request(Input::<Buffered>::new(request, buffered))
                .await
                .map_err(IntoResponse::into_response)?;

        Ok((value, state))
    }
}

impl<A: FromRequest<Streaming>> ResolveRequest<Streaming, Box<dyn RequestStream>> for A {
    type Next = ();

    async fn resolve(
        request: &Request,
        _buffered: &[u8],
        stream: Box<dyn RequestStream>,
    ) -> Result<(Self, Self::Next), Response> {
        let value =
            <A as FromRequest<Streaming>>::from_request(Input::<Streaming>::new(request, stream))
                .await
                .map_err(IntoResponse::into_response)?;

        Ok((value, ()))
    }
}

pub trait Handler<Arguments, Modes> {
    async fn call(&self, request: Request, stream: Box<dyn RequestStream>) -> Response;
}

impl<Output: IntoResponse, Fut: Future<Output = Output>, F: Fn() -> Fut> Handler<(), ()> for F {
    async fn call(&self, request: Request, stream: Box<dyn RequestStream>) -> Response {
        drop(request);
        drop(stream);

        self().await.into_response()
    }
}

macro_rules! impl_handler {
    (
        $stream:ident,
        $next:ident;
        $(($argument:ident, $mode:ident, $value:ident, $state:ty, $input:ident)),+
        $(,)?
    ) => {
        impl<
                $(
                    $mode: Mode,
                    $argument: ResolveRequest<$mode, $state>,
                )+
                Output: IntoResponse,
                Fut: Future<Output = Output>,
                F: Fn($($argument),+) -> Fut,
            > Handler<($($argument,)+), ($($mode,)+)> for F
        {
            async fn call(
                &self,
                request: Request,
                mut $stream: Box<dyn RequestStream>,
            ) -> Response {
                let has_buffered = false
                    $(|| <$argument as FromRequest<$mode>>::BUFFERED)+;

                let buffered = if has_buffered {
                    let bytes = match collect_stream($stream.as_mut()).await {
                        Ok(bytes) => bytes,
                        Err(error) => return error.into_response(),
                    };

                    Arc::new(bytes)
                } else {
                    Arc::new(Vec::new())
                };

                if has_buffered {
                    $stream = Box::new(BufferedRequestStream::new(Arc::clone(&buffered)));
                }

                $(
                    let ($value, $next) = match <$argument as ResolveRequest<
                        $mode,
                        $state,
                    >>::resolve(
                        &request,
                        buffered.as_slice(),
                        $input,
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(response) => return response,
                    };
                )+

                drop($next);
                drop(buffered);

                self($($value),+).await.into_response()
            }
        }
    };
}

serverkit_macros::impl_handlers!(12);

#[cfg(test)]
mod tests {
    use crate::{Handler, Method, Unused};

    async fn one(_a0: Method) {}

    async fn two(_a0: Method, _a1: Method) {}

    #[allow(clippy::too_many_arguments)]
    async fn twelve(
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
    ) {
    }

    fn assert_handler<Arguments, Modes, H: Handler<Arguments, Modes>>(_handler: H) {}

    #[test]
    fn implements_supported_arities() {
        assert_handler::<(Method,), (Unused,), _>(one);
        assert_handler::<(Method, Method), (Unused, Unused), _>(two);
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
            ),
            (
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
                Unused,
            ),
            _,
        >(twelve);
    }
}
