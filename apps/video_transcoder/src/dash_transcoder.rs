use crate::{full, mp4_utils};
use anyhow::{Result, bail};
use bytes::Bytes;
use flate2::read::GzDecoder;
use http::{
    HeaderValue, Response, Uri,
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING},
    response,
};
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::body::Incoming;
use mp4_atom::{Atom, Moov, ReadAtom, WriteTo};
use regex::Regex;
use std::io::Read;
use tokio::{fs, sync::Mutex};
use tracing::{warn};
use xmltree::{Element, XMLNode};

const TARGET_CODEC: &str = "vp09.00.10.08";

/// Checks if the element contains a <BaseUrl> element. If so, extracts the URL from it, and
/// if it is a relative URL, prepends the root URI. The returned URL is gaurenteed to have
/// both a scheme and authority.
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

/// Returns an iterator over all child elements with the specified tag name.
fn filter_children<'a>(ele: &'a mut Element, name: &str) -> impl Iterator<Item = &'a mut Element> {
    ele.children.iter_mut().filter_map(move |child| {
        if let XMLNode::Element(e) = child {
            if e.name == name { Some(e) } else { None }
        } else {
            None
        }
    })
}

/// URLs inside <SegmentTemplate> can contain variables. This helper function generates a regex
/// pattern that matches those URLs, e.g. a numeric variable is represented as `\d+`.
/// $Number$ is converted into a capture group.
fn resolve_template(template: &str, representation_id: &str, bandwidth: &str) -> Option<Regex> {
    // TODO: naive resolution doesn't support escaped characters (i.e. "$$")
    // or formatted width (i.e. "$...%[width]d$")
    let pattern = regex::escape(
        &template
            .replace("$RepresentationID$", representation_id)
            .replace("$Bandwidth$", bandwidth),
    );
    let pattern = pattern
        .replace("\\$Number\\$", "(\\d+)")
        .replace("\\$Time\\$", "\\d+")
        .replace("\\$SubNumber\\$", "\\d+");
    Regex::new(&format!(r"^{pattern}$")).ok()
}

/// Scans through MPD file and finds all representations described with SegmentTemplate.
/// Extracts the URL of the initialization/media segments of the representation based on
/// the appropriate <BaseUrl> element, if present. Will modify any found representation
/// and change the codec to VP9.
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
                let Some(seg_template) = representation.get_child("SegmentTemplate") else {
                    continue;
                };
                let Some(timescale) = seg_template.attributes.get("timescale") else {
                    continue;
                };
                let Ok(timescale) = timescale.parse::<u32>() else {
                    continue;
                };
                let Some(duration) = seg_template.attributes.get("duration") else {
                    continue;
                };
                let Ok(duration) = duration.parse::<u32>() else {
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
                    base_uri: representation_base,
                    timescale,
                    duration,
                });
                representation
                    .attributes
                    .insert("codecs".to_owned(), TARGET_CODEC.to_owned());
            }
        }
    }
    targets
}

/// Checks if `prefix` is a prefix of `test`. If so, returns the suffix of `test`
/// not found in `prefix`.
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
    let (mut parts, body) = response.into_parts();
    parts.headers.remove(TRANSFER_ENCODING);
    let body_collected = body.collect().await?;
    let body_collected: Vec<u8> = body_collected.to_bytes().into_iter().collect();

    if let Some(encoding) = parts.headers.get(CONTENT_ENCODING) {
        if encoding == HeaderValue::from_static("gzip") {
            let mut decompressed = Vec::new();
            GzDecoder::new(&body_collected[..]).read_to_end(&mut decompressed)?;
            parts.headers.remove(CONTENT_ENCODING);
            parts
                .headers
                .insert(CONTENT_LENGTH, HeaderValue::from(decompressed.len()));
            Ok((parts, decompressed))
        } else {
            // this should never happen because we set Accept-Encoding of upstream requests to gzip
            bail!(
                "Trying to read response with unsupported content encoding: {:?}",
                encoding
            );
        }
    } else {
        Ok((parts, body_collected))
    }
}

/// Represents DASH stream representation. A single MPD file can hold multiple TranscodeTargets.
/// A stream representation is broken into two parts:
/// 1. Initialization segment, which contains the Moov atom
/// 2. Media segments, which contain the Mdat atoms
#[derive(Debug, Clone)]
struct TranscodeTarget {
    init_pattern: Regex,
    media_pattern: Regex,
    original_init_segment: Option<Vec<u8>>,
    base_uri: Uri,
    timescale: u32,
    duration: u32,
}

/// Transcoder that tracks active MPEG-DASH streams and silently transcodes them to VP9.
pub struct Transcoder {
    vp9_stbl: Vec<u8>,
    active_targets: Mutex<Vec<TranscodeTarget>>,
    gst_transcode_exe: String,
}

impl Transcoder {
    /// Creates a new Transcoder instance. DASH streams start with an initialization segment,
    /// containing a Moov atom that describes the video codec. We inject the VP9 codec described in
    /// the Moov atom inside `vp9_template` into the initialization segment. The actual transcoding
    /// is performed by the GStreamer helper executable `gst_transcoder_exe`.
    pub async fn new(vp9_template: &str, gst_transcode_exe: String) -> Result<Self> {
        let template_data = fs::read(vp9_template).await?;
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
            gst_transcode_exe,
        })
    }

    /// Reads the MPD file and prepares to sniff out the MP4 segments described in it.
    /// Only support On-Demand profiles describe using SegmentTemplate.
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
        let mut nodes = match Element::parse_all(&body[..]) {
            Ok(nodes) => nodes,
            Err(err) => {
                warn!("Failed to parse MPD file: {:?}", err);
                return Ok(Response::from_parts(parts, full(body)));
            }
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
            warn!("Could not find <MPD> element in the MPD file");
            Ok(Response::from_parts(parts, full(body)))
        }
    }

    /// Checks if the MP4 segment matches any known TranscodeTarget, and if so, transcodes it to VP9.
    pub async fn transcode_segments(
        &self,
        request_uri: Uri,
        response: Response<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let mut active_targets = self.active_targets.lock().await;
        for target in active_targets.iter_mut() {
            let resource = match uri_prefixes(&target.base_uri, &request_uri) {
                Some(resource) => resource,
                None => continue,
            };

            if target.init_pattern.is_match(&resource) {
                let (mut parts, body) = read_response(response).await?;
                let new_body =
                    match mp4_utils::replace_stbl_atom(&body, &self.vp9_stbl, target.timescale) {
                        Ok(new_body) => new_body,
                        Err(err) => {
                            warn!("Failed to replace Stbl atom in initialization segment: {err}");
                            return Ok(Response::from_parts(parts, full(body)));
                        }
                    };
                target.original_init_segment = Some(body);

                parts
                    .headers
                    .insert(CONTENT_LENGTH, HeaderValue::from(new_body.len()));
                return Ok(Response::from_parts(parts, full(new_body)));
            } else if let Some(captures) = target.media_pattern.captures(&resource) {
                if target.original_init_segment.is_none() {
                    warn!("Found media segment before initialization segment");
                    continue;
                }
                let Some(segment_number) = captures.get(1) else {
                    warn!("Found media segment but don't know the segment number. Can't proceed.");
                    continue;
                };
                let segment_number = segment_number.as_str().parse::<u32>().unwrap_or(1);

                let timescale = target.timescale;
                let segment_durations = target.duration;
                // MUST drop active_targets before transcoding because it takes a long time and
                // will block other actions
                // that said, we could probably refactor to remove the cloning
                let original_init_segment = target.original_init_segment.as_ref().unwrap().clone();
                drop(active_targets);

                let (mut parts, body) = read_response(response).await?;
                let new_body = match mp4_utils::transcode_segment(
                    &original_init_segment,
                    &body,
                    timescale,
                    segment_number,
                    segment_durations,
                    &self.gst_transcode_exe,
                )
                .await
                {
                    Ok(new_body) => new_body,
                    Err(err) => {
                        warn!("Failed to transcode segment: {err}");
                        return Ok(Response::from_parts(parts, full(body)));
                    }
                };

                parts
                    .headers
                    .insert(CONTENT_LENGTH, HeaderValue::from(new_body.len()));
                return Ok(Response::from_parts(parts, full(new_body)));
            }
        }
        Ok(response.map(|b| b.boxed()))
    }

    /// Sniffs the HTTP response and checks if it corresponds to an MPEG-DASH stream.
    /// Will process MPD files (i.e. `application/dash+xml`) and MP4 segments corresponding
    /// to known MPD files. Either returns the original response or a response transcoded
    /// to VP9.
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
