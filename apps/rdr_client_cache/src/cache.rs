use async_trait::async_trait;
use http::{HeaderValue, Method, Request, Response, header::CACHE_CONTROL, request};
use http_body_util::BodyExt;
use http_cache::{
    CacheManager, CacheMode, CacheOptions, HttpCache, HttpCacheOptions, HttpResponse, Middleware,
};
use http_cache_semantics::CachePolicy;
use hudsucker::{HttpContext, HttpHandler, RequestOrResponse};
use hyper_util::client::legacy::{Client, connect::Connect};
use std::{collections::HashMap, mem, time::SystemTime};
use tracing::{error, info};
use url::Url;

type CacheResult<T> = http_cache::Result<T>;

struct PullThroughRequest<C> {
    req: Request<hudsucker::Body>,
    parts: request::Parts,
    client: Client<C, hudsucker::Body>,
}

async fn clone_req(
    req: Request<hudsucker::Body>,
) -> (Request<hudsucker::Body>, Request<hudsucker::Body>) {
    let (parts, body) = req.into_parts();

    let parts1 = parts.clone();
    let parts2 = parts.clone();

    let body_collected = body.collect().await.unwrap().to_bytes();
    let body1 = http_body_util::Full::new(body_collected.clone());
    let body2 = http_body_util::Full::new(body_collected);

    (
        Request::from_parts(parts1, hudsucker::Body::from(body1)),
        Request::from_parts(parts2, hudsucker::Body::from(body2)),
    )
}

#[async_trait]
impl<C: Send + Sync + Clone + 'static + Connect> Middleware for &mut PullThroughRequest<C> {
    fn is_method_get_head(&self) -> bool {
        match self.req.method() {
            &Method::GET | &Method::HEAD => true,
            _ => false,
        }
    }

    fn policy(&self, response: &HttpResponse) -> CacheResult<CachePolicy> {
        Ok(CachePolicy::new(&self.parts()?, &response.parts()?))
    }

    fn policy_with_options(
        &self,
        response: &HttpResponse,
        options: CacheOptions,
    ) -> CacheResult<CachePolicy> {
        Ok(CachePolicy::new_options(
            &self.parts()?,
            &response.parts()?,
            SystemTime::now(),
            options,
        ))
    }

    fn update_headers(&mut self, parts: &request::Parts) -> CacheResult<()> {
        for header in parts.headers.iter() {
            self.req
                .headers_mut()
                .insert(header.0.clone(), header.1.clone());
        }
        Ok(())
    }

    fn force_no_cache(&mut self) -> CacheResult<()> {
        self.req
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_str("no-cache")?);
        Ok(())
    }

    fn parts(&self) -> CacheResult<request::Parts> {
        Ok(self.parts.clone())
    }

    fn url(&self) -> CacheResult<Url> {
        Ok(self.req.uri().to_string().parse()?)
    }

    fn method(&self) -> CacheResult<String> {
        Ok(self.req.method().to_string())
    }

    async fn remote_fetch(&mut self) -> CacheResult<HttpResponse> {
        let req = mem::replace(
            &mut self.req,
            Request::new(http_body_util::Empty::new().into()),
        );
        let (req1, req2) = clone_req(req).await;
        self.req = req1;

        info!("Remote fetch: {}", self.req.uri());
        let (parts, body) = self.client.request(req2).await?.into_parts();
        let mut headers = HashMap::new();
        for (name, value) in &parts.headers {
            let name = name.to_string();
            let value = value.to_str().unwrap().to_owned();
            headers.insert(name, value);
        }
        let status: u16 = parts.status.into();
        let version = http_cache::HttpVersion::try_from(parts.version)?;
        Ok(HttpResponse {
            body: body.collect().await?.to_bytes().into(),
            headers,
            status,
            url: Url::parse(&self.req.uri().to_string())?,
            version,
        })
    }
}

impl<C> PullThroughRequest<C> {
    fn recover_request(self) -> Request<hudsucker::Body> {
        let (parts, body) = self.req.into_parts();
        Request::from_parts(parts, hudsucker::Body::from(body))
    }
}

#[derive(Clone)]
pub struct HttpChildCache<T: CacheManager + Clone, C> {
    cache: HttpCache<T>,
    client: Client<C, hudsucker::Body>,
}

impl<T: CacheManager + Clone, C> HttpChildCache<T, C> {
    pub fn new(cache_manager: T, client: Client<C, hudsucker::Body>) -> HttpChildCache<T, C> {
        let cache_opts = CacheOptions {
            shared: false,
            ..CacheOptions::default()
        };
        HttpChildCache {
            cache: HttpCache {
                mode: CacheMode::Default,
                manager: cache_manager,
                options: HttpCacheOptions {
                    cache_options: Some(cache_opts),
                    ..HttpCacheOptions::default()
                },
            },
            client,
        }
    }
}

impl<T: CacheManager + Clone, C: Send + Sync + Clone + 'static + Connect> HttpHandler
    for HttpChildCache<T, C>
{
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<hudsucker::Body>,
    ) -> RequestOrResponse {
        // processed requests overshadows Hudsucker's built-in functionality
        // only trap necessary requests
        if req.method() != Method::GET {
            return RequestOrResponse::Request(req);
        }

        let (parts, body) = req.into_parts();
        let parts_2 = parts.clone();
        let mut middleware = PullThroughRequest {
            req: Request::from_parts(parts, body),
            parts: parts_2,
            client: self.client.clone(),
        };

        match self.cache.run(&mut middleware).await {
            Ok(response) => match response.parts() {
                Ok(parts) => {
                    let body = http_body_util::Full::new(bytes::Bytes::from(response.body));
                    let converted = Response::from_parts(parts, hudsucker::Body::from(body));
                    RequestOrResponse::Response(converted)
                }
                Err(e) => {
                    error!("Error reading response: {e}");
                    RequestOrResponse::Request(middleware.recover_request())
                }
            },
            Err(e) => {
                error!("Error pulling request through parent cache: {e}");
                RequestOrResponse::Request(middleware.recover_request())
            }
        }
    }
}
