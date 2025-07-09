use http::{Method, Uri};

/// Standard representation of a cache key derived from an HTTP request.
pub fn create_cache_key(method: &Method, uri: &Uri) -> String {
    format!("{}:{}", method, uri)
}
