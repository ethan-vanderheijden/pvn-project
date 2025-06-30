use async_walkdir::WalkDir;
use axum::{
    Extension, Json, Router,
    extract::{Multipart, Path},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_lite::StreamExt;
use librqbit::{AddTorrent, AddTorrentOptions, Api, ApiError, api::TorrentIdOrHash};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Cursor, Write},
    sync::Arc,
};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    sync::Mutex,
};
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub struct TorrentState {
    pub api: Api,
    pub torrent_file_folder: String,
    torrent_files: Mutex<HashSet<Uuid>>,
}

impl TorrentState {
    pub fn new(api: Api, torrent_file_folder: String) -> TorrentState {
        TorrentState {
            api,
            torrent_file_folder,
            torrent_files: Mutex::new(HashSet::new()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum NewTorrent {
    Url(String),
    File(Uuid),
}

const TORRENT_FILE_FIELD: &'static str = "torrent_file";

struct AnyhowError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AnyhowError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AnyhowError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

fn convert_error(error: ApiError) -> Response {
    let mut response = axum::Json(&error).into_response();
    *response.status_mut() = error.status();
    response.into_response()
}

fn map_result<T: Serialize>(result: Result<T, ApiError>) -> Result<Response, Response> {
    result
        .map(|ele| axum::Json(ele).into_response())
        .map_err(convert_error)
}

async fn get_torrent(
    session: Extension<Arc<TorrentState>>,
    Path(id): Path<usize>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let details = session.api.api_torrent_details(TorrentIdOrHash::Id(id));
    match details {
        Ok(mut details) => {
            let stats = session.api.api_stats_v1(TorrentIdOrHash::Id(id));
            match stats {
                Ok(stats) => {
                    details.stats = Some(stats);
                    Ok(axum::Json(details))
                }
                Err(api_error) => Err(convert_error(api_error)),
            }
        }
        Err(api_error) => Err(convert_error(api_error)),
    }
}

async fn delete_torrent(
    session: Extension<Arc<TorrentState>>,
    Path(id): Path<usize>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    map_result(
        session
            .api
            .api_torrent_action_delete(TorrentIdOrHash::Id(id))
            .await,
    )
}

async fn add_torrent(
    session: Extension<Arc<TorrentState>>,
    Json(torrent): Json<NewTorrent>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let options = AddTorrentOptions {
        sub_folder: Some(Uuid::new_v4().to_string()),
        ..Default::default()
    };
    match torrent {
        NewTorrent::Url(url) => map_result(
            session
                .api
                .api_add_torrent(AddTorrent::Url(url.into()), Some(options))
                .await,
        ),
        NewTorrent::File(data) => {
            let torrent_files = session.torrent_files.lock().await;
            if torrent_files.contains(&data) {
                let filename = format!("{}/{}", session.torrent_file_folder, data);
                let file = AddTorrent::from_local_filename(&filename).unwrap();
                map_result(session.api.api_add_torrent(file, Some(options)).await)
            } else {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "text/plain")],
                    "Torrent file could not be found.",
                )
                    .into_response())
            }
        }
    }
}

async fn build_zip(folder_path: &str) -> anyhow::Result<Vec<u8>> {
    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut entries = WalkDir::new(folder_path);
    while let Some(entry) = entries.next().await {
        let entry = entry?;
        if entry.file_type().await?.is_file() {
            let path = format!("files/{}", entry.file_name().to_str().unwrap());
            zip.start_file(path, options)?;
            let file_data = fs::read(entry.path()).await?;
            zip.write_all(&file_data)?;
        }
    }

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

async fn direct_download(session: Extension<Arc<TorrentState>>, Path(id): Path<usize>) -> Response {
    match session.api.api_torrent_details(TorrentIdOrHash::Id(id)) {
        Ok(details) => match session.api.api_stats_v1(TorrentIdOrHash::Id(id)) {
            Ok(stats) => {
                let mut result = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "text/plain")],
                    "Torrent is not finished downloading".as_bytes().to_vec(),
                );
                if stats.finished {
                    let zip = build_zip(&details.output_folder).await;
                    result = match zip {
                        Ok(zip_file) => (
                            StatusCode::OK,
                            [(
                                header::CONTENT_TYPE,
                                "application/zip, application/octet-stream",
                            )],
                            zip_file,
                        ),
                        Err(error) => (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            [(header::CONTENT_TYPE, "text/plain")],
                            format!("Failed to build zip file: {}", error).into_bytes(),
                        ),
                    }
                }

                result.into_response()
            }
            Err(error) => convert_error(error).into_response(),
        },
        Err(error) => convert_error(error).into_response(),
    }
}

async fn add_torrent_file(
    session: Extension<Arc<TorrentState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AnyhowError> {
    while let Some(field) = multipart.next_field().await? {
        let Some(name) = field.name() else {
            continue;
        };
        if name == TORRENT_FILE_FIELD {
            let id = Uuid::new_v4();
            session.torrent_files.lock().await.insert(id);
            let mut file = File::create(format!("{}/{}", session.torrent_file_folder, id)).await?;
            file.write_all(&field.bytes().await?).await?;
            return Ok((StatusCode::OK, id.to_string()));
        }
    }
    Ok((
        StatusCode::BAD_REQUEST,
        format!("'{}' field not found in multipart data", TORRENT_FILE_FIELD),
    ))
}

pub fn register_routes(router: Router) -> Router {
    router
        .route("/torrents/{id}", get(get_torrent).delete(delete_torrent))
        .route("/torrents", post(add_torrent))
        .route("/torrents/{id}/download", get(direct_download))
        .route("/torrent_files", post(add_torrent_file))
}
