use crate::utils;

use anyhow::{Result, bail};
use async_trait::async_trait;
use http::HeaderMap;
use http_cache::CacheManager;
use http_cache_semantics::CachePolicy;
use rdr_common::{WireProtocol, event_hub::EventHub};
use std::{clone::Clone, fmt, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex,
};
use tracing::{info, warn};
use url::Url;

const UPSTREAM_TIMEOUT: u32 = 5;

/// Trait representing a client that can perform HTTP GET requests.
#[async_trait]
pub trait Client: Send + Sync {
    /// Perform the HTTP GET request with the given headers. Will wait until a response is received.
    async fn get(&self, url: &Url, headers: &HeaderMap) -> Result<http_cache::HttpResponse>;
}

/// Possible errors when forwarding requests to the upstream cache.
#[derive(Debug)]
pub enum PullThroughError {
    UpstreamTimedOut,
    NoUpstreamConnection,
}

impl fmt::Display for PullThroughError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PullThroughError::UpstreamTimedOut => {
                write!(f, "Upstream request timed out for URL")
            }
            PullThroughError::NoUpstreamConnection => {
                write!(f, "No upstream connection for URL")
            }
        }
    }
}

impl std::error::Error for PullThroughError {}

/// Client implementation that connects to an upstream cache and forwards
/// requests to it. Also listens for resources pushed by the upstream cache
/// and silently adds them to the local cache.
pub struct PullThroughClient<C: CacheManager> {
    conn: Mutex<Option<OwnedWriteHalf>>,
    pending_requests: Mutex<EventHub<Url, rdr_common::Response>>,
    cache: C,
}

/// Create a TCP connection and split it.
async fn reconnect(addr: SocketAddr) -> Option<(OwnedReadHalf, OwnedWriteHalf)> {
    match TcpStream::connect(addr).await {
        Ok(stream) => Some(stream.into_split()),
        Err(e) => {
            warn!("Failed to connect to upstream cache at {}: {}", addr, e);
            None
        }
    }
}

/// Inject the request/response pair into the HTTP cache if it is deemed cacheable.
async fn inject_to_cache(resource: rdr_common::Response, cache: &impl CacheManager) -> Result<()> {
    let (request, response) = resource.convert();
    let policy = CachePolicy::new(
        &request,
        &response.parts().map_err(|e| anyhow::Error::from_boxed(e))?,
    );
    if policy.is_storable() {
        cache
            .put(
                utils::create_cache_key(request.method(), request.uri()),
                response,
                policy,
            )
            .await
            .map_err(|e| anyhow::Error::from_boxed(e))?;
    }
    Ok(())
}

/// This function has three responsibilities:
/// 1. Continually reads from the TCP connection to the parent cache and
///    notifies waiting request tasks when their request has been fulfilled.
/// 2. If the response given by the parent cache does not correspond to a pending
///    request, it is a pushed resource and silently added to the local cache.
/// 3. If the connection to the parent cache is lost or corrupted, it will
///    transparently reconnect.
///
/// This function indefinitely holds a reference to the client instance. We detect
/// when all other references are gone and drop our reference too.
async fn read_loop<C: CacheManager>(parent_addr: SocketAddr, client: Arc<PullThroughClient<C>>) {
    // Invariant: if read_half is None, then client.conn should also be None.
    let mut read_half: Option<OwnedReadHalf> = None;
    loop {
        // also checking weak_count ensures race conditions are impossible
        // since no one can create a new Arc between checking and exiting
        if Arc::weak_count(&client) == 0 && Arc::strong_count(&client) == 1 {
            info!("PullThroughClient has no other references, exiting read loop");
            return;
        }

        if let Some(reader) = &mut read_half {
            match rdr_common::Response::extract_from(reader).await {
                Ok(response) => {
                    // if the notification goes through, we don't need to add response to cache
                    // since http-cache will do this automatically
                    let mut pending_requests = client.pending_requests.lock().await;
                    if let Err(response) =
                        pending_requests.notify(&response.original_request.url.clone(), response)
                    {
                        drop(pending_requests);
                        // extra resource pushed by parent cache, must add to cache manually
                        info!(
                            "Extra resource pushed by parent cache: {}",
                            response.original_request.url
                        );
                        if let Err(error) = inject_to_cache(response, &client.cache).await {
                            warn!("Failed to inject pushed resource into cache: {error}");
                        }
                    }
                    continue;
                }
                Err(e) => {
                    // Connection either errored out, was closed, or data was corrupted
                    // unrecoverable - must reconnect
                    warn!("Connection to parent cache died: {e}");

                    read_half = None;
                    client.conn.lock().await.take();
                }
            }
        } else {
            if let Some((read, write)) = reconnect(parent_addr).await {
                read_half = Some(read);
                client.pending_requests.lock().await.clear();
                client.conn.lock().await.replace(write);
                info!("Reconnected to parent cache at {}", parent_addr);
            } else {
                // wait before retrying connection
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

impl<C: CacheManager> PullThroughClient<C> {
    /// Create a new `PullThroughClient` that connects to the parent cache at `parent_addr`.
    pub async fn new(parent_addr: SocketAddr, cache: C) -> Arc<Self> {
        let client = PullThroughClient {
            conn: Mutex::new(None),
            pending_requests: Mutex::new(EventHub::new(Duration::from_secs(
                UPSTREAM_TIMEOUT as u64,
            ))),
            cache,
        };
        let client = Arc::new(client);

        let client_2 = client.clone();
        tokio::spawn(async move {
            read_loop(parent_addr, client_2).await;
        });

        client
    }
}

#[async_trait]
impl<C: CacheManager> Client for PullThroughClient<C> {
    /// Perform the HTTP GET request by forwarding the request to the upstream cache.
    /// Will wait until a response is received. Returns an error if the upstream cache
    /// is unreachable or times out.
    async fn get(&self, url: &Url, headers: &http::HeaderMap) -> Result<http_cache::HttpResponse> {
        let mut pending_requests = self.pending_requests.lock().await;
        if !pending_requests.has_event(url) {
            let mut conn = self.conn.lock().await;
            if let Some(writable) = conn.as_mut() {
                let req = rdr_common::Request {
                    url: url.clone(),
                    headers: headers.clone(),
                };
                info!("Forwarding request to upstream cache: {}", url);
                req.serialize_to(writable).await?;
            } else {
                bail!(PullThroughError::NoUpstreamConnection);
            }
        }

        let response = pending_requests.get_or_create_event(url.clone());
        drop(pending_requests);
        let response = response.listen().await;

        if let Some(response) = response {
            Ok(response.convert().1)
        } else {
            bail!(PullThroughError::UpstreamTimedOut);
        }
    }
}
