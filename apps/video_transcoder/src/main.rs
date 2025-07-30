mod dash_transcoder;
mod mp4_utils;

use anyhow::Result;
use bytes::Bytes;
use clap::Parser;
use http::StatusCode;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{Method, Request, Response, service::service_fn, upgrade::Upgraded};
use hyper_util::{
    client::legacy::{Client, connect::Connect},
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser)]
struct Args {
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let listener = TcpListener::bind(format!("127.0.0.1:{}", args.port)).await?;

    let client = Client::builder(TokioExecutor::new()).build_http::<hyper::body::Incoming>();
    let dash_transcoder = Arc::new(dash_transcoder::Transcoder::new().await?);

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
                eprintln!("Failed to serve connection: {:?}", err);
            }
        });
    }
}

async fn proxy<C>(
    client: Client<C, hyper::body::Incoming>,
    req: Request<hyper::body::Incoming>,
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
                            eprintln!("IO error while tunneling: {}", e);
                        };
                    }
                    Err(e) => eprintln!("Failed to upgrade on CONNECT: {}", e),
                }
            });
            Ok(Response::new(empty()))
        } else {
            eprintln!("CONNECT host is not socket addr: {:?}", req.uri());
            let mut resp = Response::new(full("CONNECT must be to a socket address"));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            Ok(resp)
        }
    } else {
        let uri_clone = req.uri().clone();
        match client.request(req).await {
            Ok(response) => match transcoder.process_response(uri_clone, response).await {
                Ok(response) => {
                    return Ok(response);
                }
                Err(err) => {
                    eprintln!("Failed to parse upstream response: {err}");
                }
            },
            Err(err) => {
                eprintln!("Failed to make upstream request: {err}");
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

pub fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

pub fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}
