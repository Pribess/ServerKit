use std::net::SocketAddr;

use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::Incoming,
    header::{CONTENT_LENGTH, HeaderName, HeaderValue},
};
use serverkit::{Headers, Method, Request, Response, ResponseBody, Router};

use crate::body::{HyperRequestStream, HyperResponseBody};

pub(crate) struct HandledRequest {
    pub(crate) response: Response,
    #[cfg(feature = "websocket")]
    pub(crate) on_upgrade: hyper::upgrade::OnUpgrade,
}

pub(crate) async fn handle_request(
    router: &Router,
    #[cfg_attr(not(feature = "websocket"), allow(unused_mut))] mut request: HyperRequest<Incoming>,
    address: SocketAddr,
) -> HandledRequest {
    #[cfg(feature = "websocket")]
    let on_upgrade = hyper::upgrade::on(&mut request);
    let (parts, body) = request.into_parts();
    let mut headers = Headers::new();

    for (name, value) in &parts.headers {
        headers
            .append(name.as_str(), value.as_bytes())
            .expect("Hyper supplied an invalid request header");
    }

    let mut request = Request::from_parts(
        Method::try_from(parts.method.as_str()).expect("Hyper supplied an invalid request method"),
        parts.uri.path(),
        parts.uri.query().map(str::to_owned),
        headers,
        Box::new(HyperRequestStream::new(body)),
    );
    request.extensions.insert(address);

    HandledRequest {
        response: router.handle(request).await,
        #[cfg(feature = "websocket")]
        on_upgrade,
    }
}

pub(crate) fn into_hyper_response(response: Response) -> HyperResponse<HyperResponseBody> {
    let (status, headers, body) = response.into_parts();

    #[cfg(feature = "websocket")]
    let body = match body {
        ResponseBody::WebSocket(_) => {
            let response = Response::text(501, "WebSocket requires the Tokio Hyper driver");
            let (status, headers, body) = response.into_parts();
            return into_hyper_response_parts(status, headers, body);
        }
        body => body,
    };

    into_hyper_response_parts(status, headers, body)
}

pub(crate) fn into_hyper_response_parts(
    status: u16,
    headers: Headers,
    body: ResponseBody,
) -> HyperResponse<HyperResponseBody> {
    let length = body.buffered().map(<[u8]>::len);
    let has_content_length = headers.contains("content-length");
    let mut response = HyperResponse::new(HyperResponseBody::new(body));

    match hyper::StatusCode::from_u16(status) {
        Ok(status) => *response.status_mut() = status,
        Err(_) => *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR,
    }

    append_headers(&mut response, headers);

    if !has_content_length && let Some(length) = length {
        response.headers_mut().insert(CONTENT_LENGTH, length.into());
    }

    response
}

pub(crate) fn append_headers(response: &mut HyperResponse<HyperResponseBody>, headers: Headers) {
    for (name, value) in headers.iter() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .expect("ServerKit generated an invalid response header name");
        let value = HeaderValue::from_bytes(value)
            .expect("ServerKit generated an invalid response header value");

        response.headers_mut().append(name, value);
    }
}
