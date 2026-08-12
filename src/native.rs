use std::{
    convert::Infallible,
    io,
    net::TcpListener,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::{Body as HyperBody, Bytes, Frame, Incoming, SizeHint},
    header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;

use crate::{App, Headers, Listener, Method, Request, RequestStream, Response, StreamError};

impl Listener for TcpListener {
    type Output = io::Result<()>;

    fn serve(self, application: App) -> Self::Output {
        serve(application, self)
    }
}

fn serve(application: App, listener: TcpListener) -> io::Result<()> {
    listener.set_nonblocking(true)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let tasks = tokio::task::LocalSet::new();

    tasks.block_on(&runtime, serve_connections(application, listener))
}

async fn serve_connections(application: App, listener: TcpListener) -> io::Result<()> {
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let application = Rc::new(application);

    loop {
        let (connection, _) = listener.accept().await?;
        let application = Rc::clone(&application);

        tokio::task::spawn_local(async move {
            serve_connection(application, connection).await;
        });
    }
}

async fn serve_connection(application: Rc<App>, connection: tokio::net::TcpStream) {
    let service = service_fn(move |request| {
        let application = Rc::clone(&application);

        async move { Ok::<_, Infallible>(handle_request(application, request).await) }
    });

    let _result = http1::Builder::new()
        .serve_connection(TokioIo::new(connection), service)
        .await;
}

async fn handle_request(
    application: Rc<App>,
    request: HyperRequest<Incoming>,
) -> HyperResponse<NativeResponseBody> {
    let (parts, body) = request.into_parts();
    let mut headers = Headers::new();

    for (name, value) in &parts.headers {
        headers.append(name.as_str(), value.as_bytes());
    }

    let request = Request::new(
        Method::new(parts.method.as_str()),
        parts.uri.path(),
        parts.uri.query().map(str::to_owned),
        headers,
    );
    let stream = Box::new(NativeRequestStream::new(body));
    let response = application.handle(request, stream).await;

    into_native_response(response)
}

struct NativeRequestStream {
    body: Pin<Box<Incoming>>,
}

impl NativeRequestStream {
    fn new(body: Incoming) -> Self {
        Self {
            body: Box::pin(body),
        }
    }
}

impl RequestStream for NativeRequestStream {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        loop {
            match self.body.as_mut().poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => return Poll::Ready(Some(Ok(data.to_vec()))),
                    Err(_) => continue,
                },
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(StreamError::new(error.to_string()))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct NativeResponseBody {
    data: Option<Bytes>,
}

impl NativeResponseBody {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data: Some(Bytes::from(data)),
        }
    }
}

impl HyperBody for NativeResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.get_mut().data.take().map(|data| Ok(Frame::data(data))))
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(self.data.as_ref().map_or(0, Bytes::len) as u64);
        hint
    }
}

fn into_native_response(response: Response) -> HyperResponse<NativeResponseBody> {
    let (status, content_type, body) = response.into_parts();
    let length = body.len();
    let mut response = HyperResponse::new(NativeResponseBody::new(body));

    match hyper::StatusCode::from_u16(status) {
        Ok(status) => *response.status_mut() = status,
        Err(_) => *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR,
    }

    response.headers_mut().insert(CONTENT_LENGTH, length.into());

    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    }

    response
}
