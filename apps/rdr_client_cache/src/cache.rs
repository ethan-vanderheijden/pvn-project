use crate::{client::Client, utils};

use async_trait::async_trait;
use http::{HeaderValue, Method, Request, Response, header::CACHE_CONTROL, request};
use http_cache::{
    CacheManager, CacheMode, CacheOptions, HttpCache, HttpCacheOptions, HttpResponse, Middleware,
};
use http_cache_semantics::{CachePolicy, RequestLike};
use hudsucker::{HttpContext, HttpHandler, RequestOrResponse};
use std::{sync::Arc, time::SystemTime};
use tracing::error;
use url::Url;

type CacheResult<T> = http_cache::Result<T>;

/// Middleware that performs requests with the provided client.
struct UpstreamRequest<C> {
    req: Request<hudsucker::Body>,
    parts: request::Parts,
    client: Arc<C>,
}

impl<C> UpstreamRequest<C> {
    /// Create a new `UpstreamRequest` perform the given request using `client`.
    fn new(req: Request<hudsucker::Body>, client: Arc<C>) -> Self {
        let (parts, body) = req.into_parts();
        UpstreamRequest {
            req: Request::from_parts(parts.clone(), body),
            parts,
            client,
        }
    }
}

#[async_trait]
impl<C: Client> Middleware for &mut UpstreamRequest<C> {
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
        let url = Url::parse(&self.req.uri().to_string())?;
        Ok(self.client.get(&url, self.req.headers()).await?)
    }
}

impl<C> UpstreamRequest<C> {
    /// Convert the `UpstreamRequest` back into the original request.
    /// Useful if you want to fall back to performing the upstream
    /// request via another method.
    fn recover_request(self) -> Request<hudsucker::Body> {
        let (parts, body) = self.req.into_parts();
        Request::from_parts(parts, hudsucker::Body::from(body))
    }
}

/// HTTP proxy implementation that caches resources according to HTTP standards.
pub struct HttpChildCache<T: CacheManager, C> {
    cache: HttpCache<T>,
    client: Arc<C>,
}

impl<T: CacheManager + Clone, C> Clone for HttpChildCache<T, C> {
    fn clone(&self) -> Self {
        // Cloning is trivial but derive macro gets confused
        // since type C isn't cloneable
        HttpChildCache {
            cache: self.cache.clone(),
            client: Arc::clone(&self.client),
        }
    }
}

impl<T: CacheManager, C: Client> HttpChildCache<T, C> {
    /// Create a new HTTP caching proxy that uses `client` to perform all GET requests.
    pub fn new(cache_manager: T, client: Arc<C>) -> HttpChildCache<T, C> {
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
                    cache_key: Some(Arc::new(|parts| {
                        utils::create_cache_key(parts.method(), &parts.uri())
                    })),
                    ..HttpCacheOptions::default()
                },
            },
            client,
        }
    }
}

impl<T: CacheManager + Clone, C: Client + 'static> HttpHandler for HttpChildCache<T, C> {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<hudsucker::Body>,
    ) -> RequestOrResponse {
        // processed requests overshadows Hudsucker's built-in functionality
        // only trap GET requests
        if req.method() != Method::GET {
            return RequestOrResponse::Request(req);
        }

        let mut middleware = UpstreamRequest::new(req, self.client.clone());

        // if encountering an error, return the request to Hudsucker
        // which will execute them directly and bypass the upstream cache
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
