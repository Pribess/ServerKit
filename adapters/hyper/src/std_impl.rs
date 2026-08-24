use std::{
    convert::Infallible,
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    thread,
};

use async_io::Async;
use hyper::{
    body::Incoming,
    rt::{Read as HyperRead, ReadBufCursor, Write as HyperWrite},
    server::conn::http1,
    service::service_fn,
};
use serverkit::Router;

use crate::{
    Run,
    bridge::{handle_request, into_hyper_response},
};

impl Run<TcpListener> for Router {
    type Output = io::Result<()>;

    fn run(self, listener: TcpListener) -> Self::Output {
        serve(self, listener)
    }
}

fn serve(router: Router, listener: TcpListener) -> io::Result<()> {
    listener.set_nonblocking(false)?;
    let router = Arc::new(router);

    loop {
        let (connection, address) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let router = Arc::clone(&router);

        thread::spawn(move || {
            let _result = serve_connection(router, connection, address);
        });
    }
}

fn serve_connection(
    router: Arc<Router>,
    connection: TcpStream,
    address: std::net::SocketAddr,
) -> io::Result<()> {
    let service = service_fn(move |request: hyper::Request<Incoming>| {
        let router = Arc::clone(&router);

        async move {
            let handled = handle_request(router.as_ref(), request, address).await;
            Ok::<_, Infallible>(into_hyper_response(handled.response))
        }
    });
    let connection = http1::Builder::new().serve_connection(StdIo::new(connection)?, service);

    async_io::block_on(connection).map_err(io::Error::other)
}

struct StdIo {
    stream: Async<TcpStream>,
}

impl StdIo {
    fn new(stream: TcpStream) -> io::Result<Self> {
        Async::new(stream).map(|stream| Self { stream })
    }
}

impl HyperRead for StdIo {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        mut output: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        let mut buffer = [0_u8; 16 * 1024];
        let length = output.remaining().min(buffer.len());

        if length == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            let mut stream = self.stream.get_ref();

            match stream.read(&mut buffer[..length]) {
                Ok(read) => {
                    output.put_slice(&buffer[..read]);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready!(self.stream.poll_readable(context))?;
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl HyperWrite for StdIo {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut stream = self.stream.get_ref();

            match stream.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready!(self.stream.poll_writable(context))?;
                }
                result => return Poll::Ready(result),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut stream = self.stream.get_ref();
        Poll::Ready(stream.flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.stream.get_ref().shutdown(Shutdown::Write))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::Arc,
        task::{Context, Poll},
        thread,
    };

    use serverkit::{Chunk, Config, Response, ResponseStream, RouteMethods, Router, StreamError};

    use super::serve_connection;

    struct LargeStream {
        sent: bool,
    }

    impl ResponseStream for LargeStream {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Chunk, StreamError>>> {
            if self.sent {
                Poll::Ready(None)
            } else {
                self.sent = true;
                Poll::Ready(Some(Ok(Chunk::from(vec![7; 1024 * 1024]))))
            }
        }
    }

    fn router() -> Router {
        Router::new(
            Config::new(),
            (
                "/health".GET(|| async { "ok" }),
                "/stream".GET(|| async { Response::stream(200, LargeStream { sent: false }) }),
            ),
        )
    }

    fn request(path: &str, version: &str) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (connection, peer) = listener.accept().unwrap();
            serve_connection(Arc::new(router()), connection, peer).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        write!(
            client,
            "GET {path} {version}\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        server.join().unwrap();

        response
    }

    #[test]
    fn serves_http_1_0_and_http_1_1() {
        assert!(request("/health", "HTTP/1.0").starts_with(b"HTTP/1.0 200 OK"));
        assert!(request("/health", "HTTP/1.1").starts_with(b"HTTP/1.1 200 OK"));
    }

    #[test]
    fn streams_large_responses() {
        let response = request("/stream", "HTTP/1.1");
        let body = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .map(|index| &response[index + 4..])
            .unwrap();

        assert!(body.len() >= 1024 * 1024);
        assert!(body.iter().filter(|byte| **byte == 7).count() >= 1024 * 1024);
    }
}
