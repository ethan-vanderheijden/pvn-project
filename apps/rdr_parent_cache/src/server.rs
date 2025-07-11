use anyhow::Result;
use http::{HeaderName, HeaderValue};
use rdr_common::WireProtocol;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tracing::error;

use crate::resolver::Resolver;

/// Continually HTTP GET requests from the connected client.
async fn read_requests(stream: TcpStream, resolver: Arc<Resolver>, always_recursive: bool) {
    let (mut read_half, write_half) = stream.into_split();
    // write access to TcpStream must be protected by Mutex to ensure that
    // entire data object is written atomically
    let write_half = Arc::new(Mutex::new(write_half));
    loop {
        match rdr_common::Request::extract_from(&mut read_half).await {
            Ok(req) => {
                let writable_2 = write_half.clone();
                let resolver_2 = resolver.clone();
                tokio::spawn(async move {
                    let url_2 = req.url.clone();
                    let mut is_navigation = true;
                    if let Some(mode) = req.headers.get(HeaderName::from_static("sec-fetch-mode")) {
                        is_navigation = mode == HeaderValue::from_static("navigate");
                    }
                    let resolution_result = if always_recursive || is_navigation {
                        resolver_2.resolve_recursive(writable_2, req).await
                    } else {
                        resolver_2.resolve_direct(writable_2, req).await
                    };
                    if let Err(error) = resolution_result {
                        error!("Failed to process request URL '{url_2}': {error}");
                    }
                });
            }
            Err(e) => {
                error!("Failed to read request from peer: {e}");
                break;
            }
        }
    }
}

/// Start listening for client cache TCP connections on the specified port.
pub async fn serve(port: u16, always_recursive: bool) -> Result<()> {
    let resolver = Arc::new(Resolver::new().await?);
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let resolver_2 = resolver.clone();
        tokio::spawn(async move {
            read_requests(stream, resolver_2, always_recursive).await;
        });
    }
}
