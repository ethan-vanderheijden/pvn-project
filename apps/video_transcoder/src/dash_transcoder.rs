use crate::{full, mp4_utils};
use anyhow::{Result, bail};
use bytes::Bytes;
use http::{
    HeaderValue, Response, Uri,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
    response,
};
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::body::Incoming;
use mp4_atom::{Atom, Moov, ReadAtom, WriteTo};
use regex::Regex;
use tokio::{fs, sync::Mutex};
use xmltree::{Element, XMLNode};

const TARGET_CODEC: &str = "vp9";
const VP9_MOOV_TEMPLATE: &str = "vp9_moov.mp4";

fn extract_base_uri(ele: &Element, root_uri: &Uri) -> Option<Uri> {
    if let Some(base_url_ele) = ele.get_child("BaseURL") {
        if let Some(text) = base_url_ele.get_text() {
            if let Ok(uri) = text.parse::<Uri>() {
                if uri.authority().is_some() {
                    // probably absolute URL
                    if uri.scheme().is_none() {
                        let mut parts = uri.into_parts();
                        parts.scheme = root_uri.scheme().cloned();
                        // make sure every URL has a scheme and authority so we can match
                        // it against request_uri in the future
                        return Uri::from_parts(parts).ok();
                    } else {
                        return Some(uri);
                    }
                } else {
                    // relative URL but absolute path
                    // e.g. "/path/to/"
                    let mut parts = uri.into_parts();
                    parts.scheme = root_uri.scheme().cloned();
                    parts.authority = root_uri.authority().cloned();
                    return Uri::from_parts(parts).ok();
                }
            } else {
                // if parsing failed, it is probably a relative URL
                // e.g. "path/to/"
                return format!("{}/{}", root_uri, text).parse().ok();
            }
        }
    }
    None
}

fn filter_children<'a>(ele: &'a mut Element, name: &str) -> impl Iterator<Item = &'a mut Element> {
    ele.children.iter_mut().filter_map(move |child| {
        if let XMLNode::Element(e) = child {
            if e.name == name { Some(e) } else { None }
        } else {
            None
        }
    })
}

fn resolve_template(template: &str, representation_id: &str, bandwidth: &str) -> Option<Regex> {
    // TODO: naive resolution doesn't support escaped characters (i.e. "$$")
    // or formatted width (i.e. "$...%[width]d$")
    let pattern = regex::escape(
        &template
            .replace("$RepresentationID$", representation_id)
            .replace("$Bandwidth$", bandwidth),
    );
    let pattern = pattern
        .replace("\\$Number\\$", "\\d+")
        .replace("\\$Time\\$", "\\d+")
        .replace("\\$SubNumber\\$", "\\d+");
    Regex::new(&format!(r"^{pattern}$")).ok()
}

fn prepare_dash_representations(
    mpd: &mut Element,
    root_uri: Uri,
    mut initial_base_uri: Uri,
) -> Vec<TranscodeTarget> {
    if let Some(new_base_uri) = extract_base_uri(mpd, &root_uri) {
        initial_base_uri = new_base_uri;
    }

    let mut targets = Vec::new();
    for period in filter_children(mpd, "Period") {
        let mut period_base = initial_base_uri.clone();
        if let Some(new_base_uri) = extract_base_uri(period, &root_uri) {
            period_base = new_base_uri;
        }

        for adaptation_set in filter_children(period, "AdaptationSet") {
            let Some(content_type) = adaptation_set.attributes.get("contentType") else {
                continue;
            };
            let Some(mime_type) = adaptation_set.attributes.get("mimeType") else {
                continue;
            };
            if content_type != "video" || mime_type != "video/mp4" {
                continue;
            }

            let mut adaptation_set_base = period_base.clone();
            if let Some(new_base_uri) = extract_base_uri(adaptation_set, &root_uri) {
                adaptation_set_base = new_base_uri;
            }

            for representation in filter_children(adaptation_set, "Representation") {
                let Some(id) = representation.attributes.get("id") else {
                    continue;
                };
                let Some(bandwidth) = representation.attributes.get("bandwidth") else {
                    continue;
                };
                let Some(codecs) = representation.attributes.get("codecs") else {
                    continue;
                };
                let Some(seg_template) = representation.get_child("SegmentTemplate") else {
                    continue;
                };
                let Some(init_template) = seg_template.attributes.get("initialization") else {
                    continue;
                };
                let Some(init_pattern) = resolve_template(&init_template, &id, &bandwidth) else {
                    continue;
                };
                let Some(media_template) = seg_template.attributes.get("media") else {
                    continue;
                };
                let Some(media_pattern) = resolve_template(&media_template, &id, &bandwidth) else {
                    continue;
                };

                let mut representation_base = adaptation_set_base.clone();
                if let Some(new_base_uri) = extract_base_uri(representation, &root_uri) {
                    representation_base = new_base_uri;
                }

                targets.push(TranscodeTarget {
                    init_pattern,
                    media_pattern,
                    original_init_segment: None,
                    codecs: codecs.clone(),
                    base_uri: representation_base,
                });
                representation
                    .attributes
                    .insert("codecs".to_owned(), TARGET_CODEC.to_owned());
            }
        }
    }
    targets
}

fn uri_prefixes(prefix: &Uri, test: &Uri) -> Option<String> {
    let scheme_matches = test.scheme().is_none()
        || prefix.scheme().is_none()
        || test.scheme().unwrap() == prefix.scheme().unwrap();
    let authority_matches = test.authority().is_none()
        || prefix.authority().is_none()
        || test.authority().unwrap() == prefix.authority().unwrap();
    let path_prefixes = test.path().starts_with(prefix.path());
    if scheme_matches && authority_matches && path_prefixes {
        Some(
            test.path()[prefix.path().len()..]
                .trim_matches('/')
                .to_string(),
        )
    } else {
        None
    }
}

async fn read_response(response: Response<Incoming>) -> Result<(response::Parts, Vec<u8>)> {
    let (parts, body) = response.into_parts();
    let body_collected = body.collect().await?;
    let body_collected: Vec<u8> = body_collected.to_bytes().into_iter().collect();
    Ok((parts, body_collected))
}

#[derive(Debug, Clone)]
struct TranscodeTarget {
    init_pattern: Regex,
    media_pattern: Regex,
    original_init_segment: Option<Vec<u8>>,
    codecs: String,
    base_uri: Uri,
}

pub struct Transcoder {
    vp9_stbl: Vec<u8>,
    active_targets: Mutex<Vec<TranscodeTarget>>,
}

impl Transcoder {
    pub async fn new() -> Result<Self> {
        let template_data = fs::read(VP9_MOOV_TEMPLATE).await?;
        let Some(moov) = mp4_utils::find_atom(&template_data, &Moov::KIND) else {
            bail!("Can't find moov atom in template file");
        };
        let moov = Moov::read_atom(
            &moov.header,
            &mut moov.extract_from_unchecked(&template_data),
        )?;
        let Some(track) = moov.trak.first() else {
            bail!("No tracks found in moov atom");
        };
        let stbl = &track.mdia.minf.stbl;
        let mut stbl_data = Vec::new();
        stbl.write_to(&mut stbl_data)?;

        Ok(Transcoder {
            vp9_stbl: stbl_data,
            active_targets: Mutex::new(Vec::new()),
        })
    }

    async fn sniff_mpd(
        &self,
        request_uri: Uri,
        response: Response<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let (mut parts, body) = read_response(response).await?;

        let root_uri = request_uri.to_string();
        let lash_slash = root_uri.rfind("/").unwrap_or(root_uri.len());
        let root_uri = root_uri[..lash_slash].parse::<Uri>().unwrap();
        let mut base_uri = root_uri.clone();

        let mut mpd = None;
        let Ok(mut nodes) = Element::parse_all(&body[..]) else {
            return Ok(Response::from_parts(parts, full(body)));
        };
        for node in &mut nodes {
            let XMLNode::Element(ele) = node else {
                continue;
            };
            if ele.name == "MPD" {
                mpd = Some(ele);
            } else if ele.name == "BaseURL" {
                if let Some(new_base_uri) = extract_base_uri(&ele, &root_uri) {
                    base_uri = new_base_uri;
                }
            }
        }

        if let Some(mut mpd) = mpd {
            let targets = prepare_dash_representations(&mut mpd, root_uri, base_uri);
            self.active_targets.lock().await.extend(targets);

            let mut new_body = Vec::new();
            for node in nodes {
                let XMLNode::Element(ele) = node else {
                    continue;
                };
                ele.write(&mut new_body)?;
            }

            parts
                .headers
                .insert(CONTENT_LENGTH, HeaderValue::from(new_body.len()));
            Ok(Response::from_parts(parts, full(new_body)))
        } else {
            Ok(Response::from_parts(parts, full(body)))
        }
    }

    pub async fn transcode_segments(
        &self,
        request_uri: Uri,
        response: Response<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let mut active_targets = self.active_targets.lock().await;
        for target in active_targets.iter_mut() {
            if let Some(resource) = uri_prefixes(&target.base_uri, &request_uri) {
                if target.init_pattern.is_match(&resource) {
                    eprintln!("init_pattern match: {resource}");
                    let (mut parts, body) = read_response(response).await?;
                    let new_body = mp4_utils::replace_stbl_atom(&body, &self.vp9_stbl).unwrap();
                    target.original_init_segment = Some(body);
                    parts
                        .headers
                        .insert(CONTENT_LENGTH, HeaderValue::from(new_body.len()));
                    return Ok(Response::from_parts(parts, full(new_body)));
                } else if target.media_pattern.is_match(&resource) {
                    eprintln!("media_pattern match: {resource}");
                }
            }
        }
        Ok(response.map(|b| b.boxed()))
    }

    pub async fn process_response(
        &self,
        request_uri: Uri,
        response: Response<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        // request_uri should be full, absolute URI (i.e. has both scheme and authority)
        // since requests to HTTP proxies are supposed to have complete URIs

        if let Some(content_type) = response.headers().get(CONTENT_TYPE) {
            if content_type == HeaderValue::from_str("application/dash+xml").unwrap() {
                return self.sniff_mpd(request_uri, response).await;
            } else if content_type == HeaderValue::from_str("video/mp4").unwrap() {
                return self.transcode_segments(request_uri, response).await;
            }
        }
        Ok(response.map(|b| b.boxed()))
    }
}
