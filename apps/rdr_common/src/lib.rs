pub mod event_hub;

use anyhow::{Result, bail};
use async_trait::async_trait;
use bincode::{
    config::{self, Configuration, Limit, LittleEndian, Varint},
    serde::{decode_from_slice, encode_to_vec},
};
use chromiumoxide_cdp::cdp;
use http::{HeaderMap, HeaderName, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, fmt::Debug};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;

const CONFIG: Configuration<LittleEndian, Varint, Limit<4294967295>> =
    config::standard().with_limit::<4294967295>();

/// Represents any data type that can be stuffed into the wire and extracted out the other end.
#[async_trait]
pub trait WireProtocol: Sized {
    /// Serialize the data and stuff it into the writable stream. Uses frame length
    /// prefixes to delimit the data.
    async fn serialize_to<W: AsyncWrite + Unpin + Send>(&self, writer: &mut W) -> Result<()>;
    /// Extract the data from the readable stream. Assumes the stream is positioned at
    /// the start of a new data object. Returns error if I/O error occurs or data couldn't
    /// be deserialized. In either case, the data stream is likely unrecoverable.
    async fn extract_from<R: AsyncRead + Unpin + Send>(reader: &mut R) -> Result<Self>;
}

#[async_trait]
impl<T> WireProtocol for T
where
    T: Serialize + DeserializeOwned + Send + Sync,
{
    async fn serialize_to<W: AsyncWrite + Unpin + Send>(&self, writer: &mut W) -> Result<()> {
        let data = encode_to_vec(self, CONFIG)?;
        let size = data.len() as u32;
        let size_bytes = size.to_le_bytes();
        writer.write_all(&size_bytes).await?;
        writer.write_all(&data).await?;
        Ok(())
    }

    async fn extract_from<R: AsyncRead + Unpin + Send>(reader: &mut R) -> Result<Self> {
        let mut length_bytes = [0u8; 4];
        reader.read_exact(&mut length_bytes).await?;
        let length = u32::from_le_bytes(length_bytes);
        let mut data = vec![0u8; length as usize];
        reader.read_exact(&mut data).await?;
        let (item, _) = decode_from_slice(&data, CONFIG)?;
        Ok(item)
    }
}

/// Helper function to convert JSON object to HTTP headers.
fn headers_from_json(json: &serde_json::Value) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    match json {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                headers.insert(k.parse::<HeaderName>()?, v.to_string().parse()?);
            }
        }
        _ => {
            bail!("Expected headers to be a JSON object");
        }
    }
    Ok(headers)
}

/// Information about an HTTP request transfered from client to parent cache.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Request {
    pub url: Url,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
}

impl TryFrom<cdp::browser_protocol::network::Request> for Request {
    type Error = anyhow::Error;

    fn try_from(value: cdp::browser_protocol::network::Request) -> Result<Self> {
        Ok(Self {
            url: Url::parse(&value.url)?,
            headers: headers_from_json(value.headers.inner())?,
        })
    }
}

impl Into<http::Request<()>> for Request {
    fn into(self) -> http::Request<()> {
        let mut request = http::Request::get(self.url.as_str())
            .body(())
            .expect("Failed to build request");
        *request.headers_mut() = self.headers;
        request
    }
}

/// Information about an HTTP resource transfered from parent cache to client.
#[derive(Serialize, Deserialize, Clone)]
pub struct Response {
    pub original_request: Request,
    pub url: Url,
    #[serde(with = "http_serde::status_code")]
    pub status: StatusCode,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    pub data: Vec<u8>,
}

impl Response {
    /// Helper function to split Response object into its HTTP request and response parts.
    pub fn convert(self) -> (http::Request<()>, http_cache::HttpResponse) {
        let mut headers = HashMap::new();
        let mut header_name = None;
        for (key, value) in self.headers.into_iter() {
            if let Some(name) = key {
                header_name = Some(name.to_string());
            }
            headers.insert(
                header_name.clone().unwrap(),
                value.to_str().unwrap().to_owned(),
            );
        }
        (
            self.original_request.into(),
            http_cache::HttpResponse {
                body: self.data,
                headers,
                status: self.status.into(),
                url: self.url,
                version: http_cache::HttpVersion::Http11,
            },
        )
    }
}

impl Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("url", &self.url)
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("data_length", &self.data.len())
            .finish()
    }
}
