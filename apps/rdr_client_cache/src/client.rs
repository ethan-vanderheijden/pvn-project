use crate::utils;

use anyhow::{Result, bail};
use async_trait::async_trait;
use http::HeaderMap;
use http_cache::CacheManager;
use rdr_common::WireProtocol;
use std::{clone::Clone, collections::HashMap, fmt, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{
        Mutex, MutexGuard,
        broadcast::{self, Sender},
    },
};
use tracing::{info, warn};
use url::Url;

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
pub struct PullThroughClient<C> {
    conn: Mutex<Option<OwnedWriteHalf>>,
    pending_requests: Mutex<HashMap<Url, Sender<rdr_common::Response>>>,
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
async fn read_loop<C>(parent_addr: SocketAddr, client: Arc<PullThroughClient<C>>) {
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
            match rdr_common::Response::extract_from_async(reader).await {
                Ok(response) => {
                    // eprintln!("Just read {} bytes from parent cache", n);
                    let mut pending_requests = client.pending_requests.lock().await;
                    if let Some(tx) = pending_requests.remove(&response.original_request.url) {
                        // note: might throw error if all receivers have timed out (but this is fine)
                        let _ = tx.send(response);
                        // don't need to add response to cache since http-cache will
                        // do this automatically
                    } else {
                        // extra resource pushed by parent cache, must add to cache manually
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
            pending_requests: Mutex::new(HashMap::new()),
            cache,
        };
        let client = Arc::new(client);

        let client_2 = client.clone();
        tokio::spawn(async move {
            read_loop(parent_addr, client_2).await;
        });

        client
    }

    /// Wait for read_loop to notify us that the request has been fulfilled.
    /// Atomically waits for notification and drops the lock on `pending_requests`.
    async fn wait_for_request(
        &self,
        url: &Url,
        pending_requests: MutexGuard<'_, HashMap<Url, Sender<rdr_common::Response>>>,
    ) -> Result<rdr_common::Response> {
        let mut rx = pending_requests.get(url).unwrap().subscribe();
        // subscribe to channel before dropping guard to avoid race condition
        // where request finishes before we listen for it
        // must drop pending_requests since read_loop takes out lock before sending data
        drop(pending_requests);
        let Ok(result) = tokio::time::timeout(Duration::from_secs(5), async {
            match rx.recv().await {
                Ok(finished_response) => {
                    info!("Request for {} completed", url);
                    return Ok(finished_response);
                }
                Err(e) => {
                    // Note: we should never get a lagged error since
                    // only one response can be written
                    warn!("Error receiving finished request: {e}");
                    return Err(e);
                }
            }
        })
        .await
        else {
            bail!(PullThroughError::UpstreamTimedOut);
        };
        result.map_err(|e| e.into())
    }
}

#[async_trait]
impl<C: CacheManager> Client for PullThroughClient<C> {
    /// Perform the HTTP GET request by forwarding the request to the upstream cache.
    /// Will wait until a response is received. Returns an error if the upstream cache
    /// is unreachable or times out.
    async fn get(&self, url: &Url, headers: &http::HeaderMap) -> Result<http_cache::HttpResponse> {
        let mut pending_requests = self.pending_requests.lock().await;
        if !pending_requests.contains_key(url) {
            let mut conn = self.conn.lock().await;
            if let Some(writable) = conn.as_mut() {
                let (tx, _) = broadcast::channel(1);
                pending_requests.insert(url.clone(), tx);

                let mut req = rdr_common::Request {
                    url: url.clone(),
                    headers: headers.clone(),
                };
                req.serialize_to_async(writable).await?;
            } else {
                bail!(PullThroughError::NoUpstreamConnection);
            }
        }
        let response = self.wait_for_request(url, pending_requests).await?;
        Ok(response.into())
    }
}
