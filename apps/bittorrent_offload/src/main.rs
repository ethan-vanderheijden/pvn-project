mod routes;

use axum::{Extension, Router};
use clap::Parser;
use librqbit::{Api, Session};
use std::{fs, io, sync::Arc};

use crate::routes::TorrentState;

#[derive(Parser)]
struct Args {
    #[clap(short, long, default_value = "3000")]
    port: u16,
    #[clap(long, default_value = "./bittorrent_offload")]
    output_folder: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if let Err(error) = fs::remove_dir_all(&args.output_folder) {
        if !matches!(error.kind(), io::ErrorKind::NotFound) {
            panic!("Failed to clear output directory: {}", error);
        }
    }

    let torrent_file_folder = format!("{}/torrent_files", args.output_folder);
    fs::create_dir_all(&torrent_file_folder).expect("Couldn't create torrent file folder.");

    let torrent_session = Session::new(args.output_folder.clone().into()).await.unwrap();
    let torrent_api = Api::new(torrent_session, None);
    let app = Router::new();

    let state = Arc::new(TorrentState::new(torrent_api, torrent_file_folder));
    let app = routes::register_routes(app).layer(Extension(state));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port)).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
