mod resolver;
mod server;

use anyhow::Result;
use clap::Parser;
use tracing::Level;

#[derive(Parser, Debug)]
struct Args {
    port: u16,
    #[clap(
        long,
        default_value = "false",
        help = "If off, the resolver will only recursively resolve top level \
                navigation (based on Sec-Fetch-Mode)."
    )]
    recursive_resolve_everything: bool,
}

/// The RDR parent cache reads HTTP GET requests from the downstream client cache
/// and simulates them inside a headless Chrome instance. The response, and any other
/// resource fetched as the page loads, is sent back to the client cache.
#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args = Args::parse();
    server::serve(args.port, args.recursive_resolve_everything).await?;
    Ok(())
}
