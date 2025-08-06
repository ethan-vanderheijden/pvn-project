mod dash_transcoder;
mod mp4_utils;

use anyhow::Result;
use bytes::Bytes;
use clap::Parser;
use http::{header::{ACCEPT_ENCODING, RANGE}, HeaderValue, StatusCode};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{Method, Request, Response, service::service_fn, upgrade::Upgraded};
use hyper_util::{
    client::legacy::{Client, connect::Connect},
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tracing::{warn, Level};

#[derive(Parser)]
struct Args {
    #[clap(long, default_value = "4000", help = "Port to run the HTTP proxy on")]
    port: u16,
    #[clap(help = "Path to the GStreamer transcoding helper executable")]
    gstreamer_transcoder_path: String,
    #[clap(
        help = "Path to an example VP9 mp4 file produced by GStreamer that contains a Moov atom"
    )]
    vp9_template_path: String,
}

/// Starts a standards compliant HTTP proxy. MPEG-DASH streams are sniffed out and
/// transparently transcoded to VP9.
#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args = Args::parse();
    let listener = TcpListener::bind(format!("0.0.0.0:{}", args.port)).await?;

    let client = Client::builder(TokioExecutor::new()).build_http::<hyper::body::Incoming>();
    let dash_transcoder = Arc::new(
        dash_transcoder::Transcoder::new(&args.vp9_template_path, args.gstreamer_transcoder_path)
            .await?,
    );

    loop {
        let (stream, _) = listener.accept().await?;
        let client_2 = client.clone();
        let transcoder_2 = dash_transcoder.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(|req| {
                let client_temp = client_2.clone();
                let transcoder_temp = transcoder_2.clone();
                async move { proxy(client_temp.clone(), req, transcoder_temp).await }
            });

            if let Err(err) = auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, service)
                .await
            {
                warn!("Failed to serve connection: {:?}", err);
            }
        });
    }
}

/// Handle HTTP Proxy requests received over a single downstream connection.
/// HTTPS CONNECT requests are tunneled as usual while HTTP requests are examined
/// to see if it is part of an MPEG-DASH streams.
async fn proxy<C>(
    client: Client<C, hyper::body::Incoming>,
    mut req: Request<hyper::body::Incoming>,
    transcoder: Arc<dash_transcoder::Transcoder>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    if req.method() == Method::CONNECT {
        if let Some(addr) = req.uri().authority() {
            let addr = addr.to_string();
            tokio::spawn(async move {
                match hyper::upgrade::on(req).await {
                    Ok(upgraded) => {
                        if let Err(e) = tunnel(upgraded, addr).await {
                            warn!("IO error during CONNECT tunnel: {}", e);
                        };
                    }
                    Err(e) => warn!("Failed to upgrade on CONNECT: {}", e),
                }
            });
            Ok(Response::new(empty()))
        } else {
            warn!("CONNECT host is not socket addr: {:?}", req.uri());
            let mut resp = Response::new(full("CONNECT must be to a socket address"));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            Ok(resp)
        }
    } else {
        let uri_clone = req.uri().clone();
        // VLC media player will always add a Range header, but byte ranges change after
        // transcoding the video segments
        // Range requests aren't necessary for the MPEG-DASH streams we are supporting
        // so we can remove it
        req.headers_mut().remove(RANGE);
        req.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        match client.request(req).await {
            Ok(response) => match transcoder.process_response(uri_clone, response).await {
                Ok(response) => {
                    return Ok(response);
                }
                Err(err) => {
                    warn!("Failed to parse upstream response: {err}");
                }
            },
            Err(err) => {
                warn!("Failed to make upstream request: {err}");
            }
        }

        let mut resp = Response::new(full(format!("Upstream request failed")));
        *resp.status_mut() = StatusCode::GATEWAY_TIMEOUT;
        Ok(resp)
    }
}

async fn tunnel(upgraded: Upgraded, destination: String) -> Result<()> {
    let mut io = TokioIo::new(upgraded);
    let mut upstream = TcpStream::connect(destination).await?;
    tokio::io::copy_bidirectional(&mut io, &mut upstream).await?;
    Ok(())
}

/// Helper function to create an empty response body.
pub fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// Helper function to create a response body from data in memory.
pub fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}
