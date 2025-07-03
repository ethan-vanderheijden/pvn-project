mod cache;

use anyhow::Result;
use clap::Parser;
use http_cache::CACacheManager;
use hudsucker::{
    Proxy,
    certificate_authority::RcgenAuthority,
    rcgen::{CertificateParams, KeyPair},
    rustls::crypto::aws_lc_rs,
};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use std::net::ToSocketAddrs;
use tokio::fs;
use tracing::{Level, error};

use crate::cache::HttpChildCache;

#[derive(Parser, Debug)]
struct Args {
    #[clap(help = "Path to the PEM-encoded private key file")]
    key: String,
    #[clap(help = "Path to the DER-encoded CA certificate file")]
    der_cert: String,
    #[clap(help = "Path to cache file")]
    cache: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args = Args::parse();
    let key = fs::read_to_string(args.key).await?;
    let cert = fs::read(args.der_cert).await?;

    let key = KeyPair::from_pem(&key).expect("Failed to parse private key");
    let cert = CertificateParams::from_ca_cert_der(&cert[..].into())
        .expect("Failed to parse CA certificate")
        .self_signed(&key)
        .expect("Failed to sign CA certificate");

    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: Client<HttpsConnector<HttpConnector>, hudsucker::Body> =
        Client::builder(TokioExecutor::new())
            .http1_title_case_headers(true)
            .http1_preserve_header_case(true)
            .build(https_connector);

    let cacache = CACacheManager::new(args.cache.into(), true);
    let child_cache = HttpChildCache::new(cacache, client);

    let proxy = Proxy::builder()
        .with_addr("127.0.0.1:4000".to_socket_addrs().unwrap().next().unwrap())
        .with_ca(RcgenAuthority::new(
            key,
            cert,
            1_000,
            aws_lc_rs::default_provider(),
        ))
        .with_rustls_client(aws_lc_rs::default_provider())
        .with_http_handler(child_cache)
        .build()
        .expect("Failed to build proxy");

    if let Err(e) = proxy.start().await {
        error!("Proxy error: {e}");
    };
    Ok(())
}
