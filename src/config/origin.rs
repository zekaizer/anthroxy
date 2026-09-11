//! The origin a URL connects to: what connections are pooled by and what a
//! proxy is chosen for (ADR-0012).

/// `scheme://host:port`, with the scheme's default port written out; `None`
/// for a URL without a host or a known port.
pub fn origin(url: &reqwest::Url) -> Option<String> {
    Some(format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str()?,
        url.port_or_known_default()?
    ))
}
