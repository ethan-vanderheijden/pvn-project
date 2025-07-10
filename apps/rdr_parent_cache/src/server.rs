use anyhow::Result;
use base64::{Engine, prelude::BASE64_STANDARD};
use chromiumoxide::{
    Browser, BrowserConfig, Page,
    cdp::browser_protocol::{
        fetch::{
            self, ContinueRequestParams, EventRequestPaused, FailRequestParams,
            GetResponseBodyParams, RequestPattern, RequestStage,
        },
        network::{CookieParam, ErrorReason, SetUserAgentOverrideParams},
        target::{CreateBrowserContextParams, CreateTargetParams},
    },
};
use cookie::Cookie;
use flate2::{Compression, write::GzEncoder};
use futures::StreamExt;
use http::{
    HeaderMap, HeaderName,
    header::{ACCEPT_LANGUAGE, CONTENT_ENCODING, COOKIE, USER_AGENT},
};
use rdr_common::WireProtocol;
use std::{io::Write, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream, tcp::OwnedWriteHalf},
    sync::Mutex,
};
use tracing::{error, info, warn};

const PAGE_SETTLE_TIME: u32 = 10;

/// Extracts the request and response from the provided network event. The network event
/// must be in the response stage (i.e. `response_status_code` is set). The response body
/// is re-compressed with gzip (since it is automatically uncompressed by Chrome).
async fn extract_resource(
    page: Arc<Page>,
    event: Arc<EventRequestPaused>,
) -> Result<rdr_common::Response> {
    let status = event.response_status_code.unwrap();
    let converted_request: rdr_common::Request = event.request.clone().try_into()?;
    let response_body = if status < 300 && status > 399 {
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
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&response_body)?;
    let response_body = encoder.finish()?;

    let mut headers = HeaderMap::new();
    // unfortunately, the response body given by Chrome is already uncompressed
    // so we need to re-compress it with gzip
    headers.insert(CONTENT_ENCODING, "gzip".parse()?);
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
/// pushes those resources to the client. Non-GET requests are blocked since the
/// simulated page load should be transparent.
async fn sniff_resources(
    writable_stream: Arc<Mutex<OwnedWriteHalf>>,
    page: Arc<Page>,
) -> Result<()> {
    let intercept_requests = RequestPattern::builder()
        .url_pattern("*")
        .request_stage(RequestStage::Request)
        .build();
    let intercept_responses = RequestPattern::builder()
        .url_pattern("*")
        .request_stage(RequestStage::Response)
        .build();
    page.execute(
        fetch::EnableParams::builder()
            .patterns([intercept_requests, intercept_responses].into_iter())
            .build(),
    )
    .await?;

    let mut request_paused = page.event_listener::<EventRequestPaused>().await?;
    tokio::spawn(async move {
        while let Some(event) = request_paused.next().await {
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
                match extract_resource(page.clone(), event.clone()).await {
                    Ok(mut resource) => {
                        let mut writable = writable_stream.lock().await;
                        match resource.serialize_to(&mut *writable).await {
                            Ok(_) => {
                                info!("Pushed resource for '{}'", resource.url);
                            }
                            Err(error) => {
                                warn!("Failed to write resource: {error}");
                            }
                        }
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
        }
    });
    Ok(())
}

/// Perform an HTTP GET request by simulating a full page load. Discovered resources are
/// sent to the client immediately. In the simulated page load, the cookies and user agent
/// will reflect the original request.
async fn process_request(
    writable_stream: Arc<Mutex<OwnedWriteHalf>>,
    req: rdr_common::Request,
    browser: Arc<Browser>,
) -> Result<()> {
    let context_params = CreateBrowserContextParams::builder()
        .dispose_on_detach(true)
        .build();
    let context = browser.create_browser_context(context_params).await?;

    let page_config = CreateTargetParams::builder()
        .url("about:blank") // load the requested URL after configuring page
        .browser_context_id(context)
        .build()
        .map_err(anyhow::Error::msg)?;
    let page = browser.new_page(page_config).await?;

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

    let page = Arc::new(page);
    sniff_resources(writable_stream, page.clone()).await?;

    info!("Performing navigation to: {}", req.url.clone());
    page.goto(req.url.clone()).await?;

    // Give the page time to load and fetch new resources
    tokio::time::sleep(Duration::from_secs(PAGE_SETTLE_TIME as u64)).await;

    Ok(())
}

/// Continually HTTP GET requests from the connected client.
async fn read_requests(stream: TcpStream, browser: Arc<Browser>) {
    let (mut read_half, write_half) = stream.into_split();
    // write access to TcpStream must be protected by Mutex to ensure that
    // entire data object is written atomically
    let write_half = Arc::new(Mutex::new(write_half));
    loop {
        match rdr_common::Request::extract_from(&mut read_half).await {
            Ok(req) => {
                let writable_2 = write_half.clone();
                let browser_2 = browser.clone();
                tokio::spawn(async move {
                    let url_2 = req.url.clone();
                    if let Err(error) = process_request(writable_2, req, browser_2).await {
                        error!("Failed to process request URL '{url_2}': {error}");
                    }
                });
            }
            Err(e) => {
                error!("Failed to read request from peer: {e}");
                break;
            }
        }
    }
}

/// Start listening for client cache TCP connections on the specified port.
pub async fn serve(port: u16) -> Result<()> {
    let config = BrowserConfig::builder()
        .new_headless_mode()
        .enable_request_intercept()
        .build()
        .map_err(anyhow::Error::msg)?;
    let (browser, mut handler) = Browser::launch(config).await?;
    let browser = Arc::new(browser);

    tokio::spawn(async move {
        loop {
            let _event = handler.next().await.unwrap();
        }
    });

    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let browser_2 = browser.clone();
        tokio::spawn(async move {
            read_requests(stream, browser_2).await;
        });
    }
}
