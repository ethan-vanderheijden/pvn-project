mod server;

use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    port: u16,
}

/// The RDR parent cache reads HTTP GET requests from the downstream client cache
/// and simulates them inside a headless Chrome instance. The response, and any other
/// resource fetched as the page loads, is sent back to the client cache.
fn main() {
    let args = Args::parse();
    server::serve(args.port);
}
