mod cache;
mod client;
mod utils;

use crate::{cache::HttpChildCache, client::PullThroughClient};

use anyhow::Result;
use clap::Parser;
use http_cache::CACacheManager;
use hudsucker::{
    Proxy,
    certificate_authority::RcgenAuthority,
    rcgen::{CertificateParams, KeyPair},
    rustls::crypto::aws_lc_rs,
};
use std::net::ToSocketAddrs;
use tokio::fs;
use tracing::{Level, error};

#[derive(Parser, Debug)]
struct Args {
    #[clap(help = "Path to the Root CA's PEM-encoded private key file")]
    key: String,
    #[clap(help = "Path to the Root CA's DER-encoded certificate file")]
    der_cert: String,
    #[clap(help = "Path to persistent cache directory")]
    cache_file: String,
    #[clap(help = "Address of the parent RDR cache")]
    upstream_cache_address: String,
    #[clap(long, default_value = "4000", help = "Port to run the RDR client cache on")]
    port: u16,
}

/// The RDR client cache uses Hudsucker act as a man-in-the-middle HTTP proxy and http-cache
/// to behave like a standards-compliant HTTP proxy that caches appropriate GET requests.
/// It performs all non-GET requests by directly contacting the target server, but GET requests
/// are forwarded to the RDR parent cache, if reachable. The parent cache can push additional
/// resources to the client beyond the forwarded GET requests.
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

    let parent_addr = args
        .upstream_cache_address
        .to_socket_addrs()
        .expect("Failed to parse upstream cache address")
        .next()
        .expect("Could not resolve upstream cache adress");
    let cacache = CACacheManager::new(args.cache_file.into(), false);
    let client = PullThroughClient::new(parent_addr, cacache.clone()).await;
    let child_cache = HttpChildCache::new(cacache, client);

    let proxy = Proxy::builder()
        .with_addr(
            format!("127.0.0.1:{}", args.port)
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap(),
        )
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
