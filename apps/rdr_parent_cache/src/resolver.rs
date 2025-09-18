use anyhow::{Result, bail};
use async_stream::stream;
use base64::{Engine, prelude::BASE64_STANDARD};
use chromiumoxide::{
    Browser, BrowserConfig, Page,
    cdp::browser_protocol::{
        fetch::{
            self, ContinueRequestParams, EventRequestPaused, FailRequestParams,
            GetResponseBodyParams, RequestPattern, RequestStage,
        },
        network::{CookieParam, ErrorReason, SetUserAgentOverrideParams},
        performance_timeline::{self, EventTimelineEventAdded},
        target::{CreateBrowserContextParams, CreateTargetParams},
    },
};
use cookie::Cookie;
use flate2::{Compression, write::GzEncoder};
use futures::{Stream, StreamExt};
use http::{
    HeaderMap, HeaderName, HeaderValue, Method,
    header::{ACCEPT_ENCODING, ACCEPT_LANGUAGE, CONTENT_ENCODING, COOKIE, USER_AGENT},
};
use rdr_common::{DownstreamMessage, PageLoadNotification, WireProtocol};
use reqwest::Client;
use std::{io::Write, pin::pin, sync::Arc, time::{Duration, SystemTime}};
use tokio::{net::tcp::OwnedWriteHalf, sync::Mutex};
use tracing::{info, warn};

/// Extracts the request and response from the provided network event. The network event
/// must be in the response stage (i.e. `response_status_code` is set). `use_gzip` says
/// whether to re-compress the response body with gzip (since Chrome auto-decompresses it).
async fn extract_resource(
    page: Page,
    event: Arc<EventRequestPaused>,
    original_accept_encoding: Option<&HeaderValue>,
) -> Result<rdr_common::Response> {
    let status = event.response_status_code.unwrap();
    let mut converted_request: rdr_common::Request = event.request.clone().try_into()?;
    converted_request.headers.remove(ACCEPT_ENCODING);

    let mut response_body = if status < 300 && status > 399 {
        // GetResponseBody doesn't work for redirected requests
        Vec::new()
    } else {
        let response_params = GetResponseBodyParams::new(event.request_id.clone());
        let response = page.execute(response_params).await?;
        if response.result.base64_encoded {
            BASE64_STANDARD.decode(response.result.body)?
        } else {
            response.result.body.into_bytes()
        }
    };

    let mut headers = HeaderMap::new();

    if let Some(encoding) = original_accept_encoding {
        // accept-encoding of actual request is irrelevant since we decode and re-encode
        // the data. Pretend that it was using the client's intended encoding.
        converted_request
            .headers
            .insert(ACCEPT_ENCODING, encoding.clone());

        // unfortunately, the response body given by Chrome is already uncompressed
        // so we need to re-compress it with gzip
        if encoding.to_str()?.contains("gzip") {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&response_body)?;
            response_body = encoder.finish()?;
            headers.insert(CONTENT_ENCODING, "gzip".parse()?);
        }
    }

    for entry in event.response_headers.as_ref().unwrap() {
        if entry.name.to_lowercase() != "content-encoding" {
            headers.insert(entry.name.parse::<HeaderName>()?, entry.value.parse()?);
        }
    }

    Ok(rdr_common::Response {
        url: converted_request.url.clone(),
        status: (status as u16).try_into()?,
        headers: headers,
        data: response_body,
        original_request: converted_request,
    })
}

/// Add network middleware to the page that intercepts all requests/responses and
/// yields those resources. Non-GET requests are blocked since the
/// simulated page load should be transparent.
fn sniff_resources(
    page: Page,
    original_accept_encoding: Option<HeaderValue>,
) -> impl Stream<Item = rdr_common::Response> {
    stream! {
        let intercept_requests = RequestPattern::builder()
            .url_pattern("*")
            .request_stage(RequestStage::Request)
            .build();
        let intercept_responses = RequestPattern::builder()
            .url_pattern("*")
            .request_stage(RequestStage::Response)
            .build();
        if let Err(error) = page.execute(
            fetch::EnableParams::builder()
                .patterns([intercept_requests, intercept_responses].into_iter())
                .build(),
        )
        .await {
            warn!("Failed to enable request interception: {error}");
            return;
        }

        let Ok(mut request_paused) = page.event_listener::<EventRequestPaused>().await else {
            warn!("Failed to set up request paused listener for request events");
            return;
        };

        while let Some(event) = request_paused.next().await {
            let mut new_resource = None;

            if event.request.method.to_lowercase() != "get" {
                info!("Blocking non-GET request: {}", event.request.url);
                let fail_config = FailRequestParams::builder()
                    .request_id(event.request_id.clone())
                    .error_reason(ErrorReason::Aborted)
                    .build()
                    .unwrap();
                if let Err(error) = page.execute(fail_config).await {
                    warn!("Failed to abort request: {error}");
                }
            } else if event.response_status_code.is_some() {
                match extract_resource(page.clone(), event.clone(), original_accept_encoding.as_ref()).await {
                    Ok(resource) => {
                        new_resource = Some(resource);
                    }
                    Err(error) => {
                        warn!("Failed to extract resource from response: {error}");
                    }
                }
            }

            let fulfill_config = ContinueRequestParams::new(event.request_id.clone());
            if let Err(error) = page.execute(fulfill_config).await {
                warn!("Failed to continue request: {error}");
            }

            if let Some(new_resource) = new_resource {
                yield new_resource;
            }
        }
    }
}

/// Returns the page's Largest Contentful Paint (LCP).
async fn record_lcp(page: Page) -> Result<f64> {
    let timeline_enable = performance_timeline::EnableParams::builder()
        .event_type("largest-contentful-paint")
        .build()
        .map_err(anyhow::Error::msg)?;
    page.execute(timeline_enable).await?;

    let mut timeline_events = page.event_listener::<EventTimelineEventAdded>().await?;
    while let Some(event) = timeline_events.next().await {
        let event = &event.event;
        if let Some(lcp) = &event.lcp_details {
            return Ok(*lcp.render_time.inner());
        }
    }
    bail!("Failed to record LCP");
}

/// The Resolver is capable of performing HTTP GET requests by simulating a full page load
/// or making a single, direct request.
pub struct Resolver {
    browser: Browser,
    client: Client,
    page_settle_time: u32,
}

impl Resolver {
    /// Create a headless Chrome instance and HTTP client for future request resolution.
    pub async fn new(page_settle_time: u32) -> Result<Self> {
        let config = BrowserConfig::builder()
            .new_headless_mode()
            .enable_request_intercept()
            .build()
            .map_err(anyhow::Error::msg)?;
        let (browser, mut handler) = Browser::launch(config).await?;

        tokio::spawn(async move {
            loop {
                let _event = handler.next().await.unwrap();
            }
        });

        Ok(Self {
            browser,
            client: Client::new(),
            page_settle_time,
        })
    }

    /// Perform an HTTP GET request by simulating a full page load. Discovered resources are
    /// sent to the client immediately via the `writable_stream`. In the simulated page load,
    /// the cookies, user agent, and accepted language will reflect the original request.
    pub async fn resolve_recursive(
        &self,
        writable_stream: Arc<Mutex<OwnedWriteHalf>>,
        req: rdr_common::Request,
    ) -> Result<()> {
        let start = SystemTime::now();
        let time_since_epoch = start.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

        let context_params = CreateBrowserContextParams::builder()
            .dispose_on_detach(true)
            .build();
        let context = self.browser.create_browser_context(context_params).await?;

        let page_config = CreateTargetParams::builder()
            .url("about:blank") // load the requested URL after configuring page
            .browser_context_id(context)
            .build()
            .map_err(anyhow::Error::msg)?;
        let page = self.browser.new_page(page_config).await?;

        if let Some(user_agent) = req.headers.get(USER_AGENT) {
            let mut user_agent_config =
                SetUserAgentOverrideParams::builder().user_agent(user_agent.to_str()?);
            if let Some(language) = req.headers.get(ACCEPT_LANGUAGE) {
                user_agent_config = user_agent_config.accept_language(language.to_str()?);
            }
            let user_agent_config = user_agent_config.build().map_err(anyhow::Error::msg)?;
            page.set_user_agent(user_agent_config).await?;
        }

        for cookie_string in req.headers.get_all(COOKIE) {
            for cookie in Cookie::split_parse(cookie_string.to_str()?) {
                let Ok(cookie) = cookie else {
                    continue;
                };
                let cookie_config = CookieParam::builder()
                    .name(cookie.name())
                    .value(cookie.value())
                    .url(req.url.as_str())
                    .build()
                    .map_err(anyhow::Error::msg)?;
                page.set_cookie(cookie_config).await?;
            }
        }

        let page_2 = page.clone();
        let writable_stream_2 = writable_stream.clone();
        let original_accept_encoding = req.headers.get(ACCEPT_ENCODING).cloned();
        tokio::spawn(async move {
            let mut stream = pin!(sniff_resources(page_2, original_accept_encoding));
            while let Some(resource) = stream.next().await {
                let mut writable = writable_stream_2.lock().await;
                let msg = DownstreamMessage::Response(resource);
                match msg.serialize_to(&mut *writable).await {
                    Ok(_) => {
                        info!("Pushed resource for '{}'", msg.url());
                    }
                    Err(error) => {
                        warn!("Failed to write resource: {error}");
                    }
                }
            }
        });

        let page_3 = page.clone();
        let writable_stream_3 = writable_stream.clone();
        let url_2 = req.url.clone();
        tokio::spawn(async move {
            match record_lcp(page_3).await {
                Ok(lcp) => {
                    let time_to_load = lcp - time_since_epoch;
                    let perf = PageLoadNotification {
                        url: url_2,
                        lcp_secs: time_to_load,
                    };
                    let mut writable = writable_stream_3.lock().await;
                    let msg = DownstreamMessage::PageLoaded(perf);
                    match msg.serialize_to(&mut *writable).await {
                        Ok(_) => {
                            info!("Recorded LCP of {}s for '{}'", time_to_load, msg.url());
                        }
                        Err(error) => {
                            warn!("Failed to write LCP notification: {error}");
                        }
                    }
                }
                Err(error) => {
                    warn!("Failed to record LCP: {error}");
                }
            }
        });

        info!("Performing navigation to: {}", req.url.clone());
        page.goto(req.url.clone()).await?;

        // Give the page time to load and fetch new resources
        tokio::time::sleep(Duration::from_secs(self.page_settle_time as u64)).await;

        page.close().await?;

        Ok(())
    }

    pub async fn resolve_direct(
        &self,
        writable_stream: Arc<Mutex<OwnedWriteHalf>>,
        req: rdr_common::Request,
    ) -> Result<()> {
        info!("Performing direct request to: {}", req.url);
        let response = self
            .client
            .request(Method::GET, req.url.clone())
            .headers(req.headers.clone())
            .send()
            .await?;
        let resource = rdr_common::Response {
            original_request: req,
            url: response.url().to_owned(),
            status: response.status(),
            headers: response.headers().clone(),
            data: response.bytes().await?.to_vec(),
        };
        let mut writerable_stream = writable_stream.lock().await;
        let msg = DownstreamMessage::Response(resource);
        msg.serialize_to(&mut *writerable_stream).await?;
        Ok(())
    }
}
