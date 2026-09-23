//! Validation shared by every user-configured HTTP sync endpoint (an IPFS
//! RPC endpoint, an S3-compatible storage endpoint). These are URLs the
//! user typed, not URLs found in message content, so private hosts are
//! allowed — but credentials must never travel over plaintext HTTP to
//! anything other than this machine, and no part of the URL may smuggle
//! user-info, a query string, or a fragment into the requests an adapter
//! builds from it.

/// A validated, normalized endpoint origin: scheme + host + optional port +
/// optional fixed path prefix. Never carries user-info, a query string, or
/// a fragment — every real request path is appended structurally by the
/// adapter, never by string-splicing a caller-supplied URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EndpointOrigin {
    pub scheme: &'static str,
    pub host: String,
    pub port: Option<u16>,
    pub path_prefix: String,
}

impl EndpointOrigin {
    /// Parses `input`, naming it `label` (e.g. "RPC URL") in every error
    /// message so the user can tell which field was rejected.
    pub fn parse(input: &str, label: &str) -> Result<Self, String> {
        let url = url::Url::parse(input.trim()).map_err(|error| format!("Invalid {label}: {error}"))?;
        let scheme = match url.scheme() {
            "https" => "https",
            "http" => "http",
            other => return Err(format!("{label} must use http or https, not {other}")),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!("{label} must not contain user-info"));
        }
        if url.query().is_some() {
            return Err(format!("{label} must not contain a query string"));
        }
        if url.fragment().is_some() {
            return Err(format!("{label} must not contain a fragment"));
        }
        let host = url.host_str().ok_or_else(|| format!("{label} must have a host"))?.to_string();
        if scheme == "http" && !is_loopback_host(&host) {
            return Err(format!("Non-loopback endpoints must use HTTPS ({label})"));
        }
        let mut path_prefix = url.path().trim_end_matches('/').to_string();
        if path_prefix == "/" {
            path_prefix.clear();
        }
        Ok(Self {
            scheme,
            host,
            port: url.port(),
            path_prefix,
        })
    }

    /// `scheme://host[:port][/prefix]`, with no trailing slash.
    pub fn base_url(&self) -> String {
        let port = self.port.map(|port| format!(":{port}")).unwrap_or_default();
        format!("{}://{}{}{}", self.scheme, self.host, port, self.path_prefix)
    }
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `Url::host_str` returns a bracketed literal for IPv6 (`"[::1]"`),
    // since that's the form a URL authority requires; strip the brackets
    // before parsing it as an address.
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    unbracketed
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_field_in_errors() {
        let error = EndpointOrigin::parse("http://storage.example.com", "Endpoint URL").unwrap_err();
        assert!(error.contains("Endpoint URL"), "{error}");
    }

    #[test]
    fn base_url_keeps_port_and_prefix_without_a_trailing_slash() {
        let origin = EndpointOrigin::parse("http://127.0.0.1:9000/minio/", "Endpoint URL").unwrap();
        assert_eq!(origin.base_url(), "http://127.0.0.1:9000/minio");
        let origin = EndpointOrigin::parse("https://s3.us-east-1.amazonaws.com", "Endpoint URL").unwrap();
        assert_eq!(origin.base_url(), "https://s3.us-east-1.amazonaws.com");
    }

    #[test]
    fn allows_https_to_a_private_host() {
        // A self-hosted MinIO on the LAN is a legitimate user choice; only
        // plaintext to a non-loopback host is refused.
        assert!(EndpointOrigin::parse("https://192.168.1.5:9000", "Endpoint URL").is_ok());
    }
}
