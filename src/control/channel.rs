//! Tonic transport over WASI TCP sockets. DNS resolution and HTTP/2 run in the guest.
use crate::{Error, Result};
use http::{Request, Response, Uri, uri::Authority};
use hyper::{body::Incoming, client::conn::http2};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use std::{
    future::Future,
    net::ToSocketAddrs,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::net::TcpStream;

#[derive(Clone)]
pub(super) struct Channel(Authority);

impl Channel {
    pub(super) fn new(target: &str) -> Result<Self> {
        target.parse().map(Self).map_err(|_| Error::storage())
    }
}

impl tower_service::Service<Request<tonic::body::Body>> for Channel {
    type Response = Response<Incoming>;
    type Error = tonic::Status;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, tonic::Status>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), tonic::Status>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request<tonic::body::Body>) -> Self::Future {
        let authority = self.0.clone();
        Box::pin(async move {
            let send = async {
                let addresses = authority.as_str().to_socket_addrs()?.collect::<Vec<_>>();
                let socket = tokio::time::timeout(
                    Duration::from_secs(10),
                    TcpStream::connect(addresses.as_slice()),
                )
                .await??;
                let (mut sender, driver) = http2::Builder::new(TokioExecutor::new())
                    .timer(TokioTimer::new())
                    .handshake(TokioIo::new(socket))
                    .await
                    .map_err(std::io::Error::other)?;
                tokio::spawn(driver);
                let mut parts = req.uri().clone().into_parts();
                parts.scheme = Some(http::uri::Scheme::HTTP);
                parts.authority = Some(authority);
                *req.uri_mut() = Uri::from_parts(parts).map_err(std::io::Error::other)?;
                sender
                    .send_request(req)
                    .await
                    .map_err(std::io::Error::other)
            };
            send.await
                .map_err(|_| tonic::Status::unavailable("runtime connection failed"))
        })
    }
}
