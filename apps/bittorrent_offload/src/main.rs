mod routes;

use axum::{Extension, Router};
use librqbit::{Api, Session};
use std::{env, fs, io, sync::Arc};

use crate::routes::TorrentState;

#[tokio::main]
async fn main() {
    let home_dir = env::home_dir().unwrap();
    let home_dir = home_dir.to_string_lossy();
    let output_folder = format!("{}/bittorrent_offload", home_dir);

    let torrent_session = Session::new(output_folder.clone().into()).await.unwrap();
    let torrent_api = Api::new(torrent_session, None);
    let app = Router::new();

    let torrent_file_folder = format!("{}/torrent_files", output_folder);
    if let Err(error) = fs::remove_dir_all(&output_folder) {
        if !matches!(error.kind(), io::ErrorKind::NotFound) {
            panic!("Failed to clear output directory: {}", error);
        }
    }
    fs::create_dir_all(&torrent_file_folder).expect("Couldn't create torrent file folder.");
    let state = Arc::new(TorrentState::new(torrent_api, torrent_file_folder));
    let app = routes::register_routes(app).layer(Extension(state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
