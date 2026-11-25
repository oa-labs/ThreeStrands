//! Shared SSRF defenses for outbound requests to URLs derived from
//! untrusted content rather than URLs we control ourselves: one-click
//! unsubscribe ([`crate::unsubscribe`]), the remote image proxy
//! ([`crate::image_proxy`]), IMAP autodiscovery
//! ([`crate::provider::imap::discovery`]), and agent research transport
//! ([`crate::agents::transport`]).
//!
//! A URL string can look like it points at a public host and still resolve
//! (immediately, or on a later request via DNS rebinding) to a private or
//! loopback address reachable only from this machine. Rejecting the literal
//! hostname string is a fast first check but not sufficient on its own, so
//! [`dns_resolver`] also filters every address a hostname resolves to at
//! connect time — including on redirects, since it's attached to the
//! `reqwest::Client` itself rather than checked once up front.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

/// Returns `true` only for globally routable public IP addresses.
///
/// IPv4 blocks:
/// - `0.0.0.0/8` (this network / unspecified)
/// - `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` (RFC 1918 private)
/// - `100.64.0.0/10` (CGNAT / Tailscale)
/// - `127.0.0.0/8` (loopback)
/// - `169.254.0.0/16` (link-local / cloud metadata)
/// - `192.0.0.0/24` (IETF protocol assignments)
/// - `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24` (TEST-NET-1/2/3)
/// - `192.88.99.0/24` (deprecated 6to4 relay anycast)
/// - `198.18.0.0/15` (benchmarking)
/// - `224.0.0.0/3` (multicast, reserved, limited broadcast)
///
/// IPv6 allows only `2000::/3` global unicast minus special-purpose subranges
/// (`2001::/23`, `2001:db8::/32`, `2002::/16` 6to4, `3fff::/20`), which also
/// rejects loopback, unspecified, IPv4-compatible (`::/96`), IPv4-mapped
/// (`::ffff:0:0/96`), NAT64 (`64:ff9b::/96`, `64:ff9b:1::/48`), ULA, and
/// link-local addresses.
pub(crate) fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v) => {
            let n = u32::from(v);
            ![
                (0, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 3),
            ]
            .iter()
            .any(|(base, bits)| n >> (32 - bits) == base >> (32 - bits))
        }
        IpAddr::V6(v) => {
            let n = u128::from(v);
            n >> 125 == 1
                && ![
                    (0x2001u128 << 112, 23),
                    (0x20010db8u128 << 96, 32),
                    (0x2002u128 << 112, 16),
                    (0x3fffu128 << 112, 20),
                ]
                .iter()
                .any(|(base, bits)| n >> (128 - bits) == base >> (128 - bits))
        }
    }
}

pub(crate) fn is_private_ip(address: IpAddr) -> bool {
    !is_public_ip(address)
}

/// Checks a parsed URL's host. Prefer this over passing `host_str()` to
/// [`is_disallowed_host`]: `host_str()` keeps IPv6 literals bracketed
/// (`"[::1]"`), which don't parse as an address, and `reqwest` connects to
/// IP literals without consulting [`dns_resolver`] — so a string check would
/// let every private IPv6 literal through.
pub(crate) fn is_disallowed_url_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => is_disallowed_host(domain),
        Some(Host::Ipv4(address)) => is_private_ip(IpAddr::V4(address)),
        Some(Host::Ipv6(address)) => is_private_ip(IpAddr::V6(address)),
        None => true,
    }
}

pub(crate) fn is_disallowed_host(host: &str) -> bool {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    normalized == "localhost"
        || normalized == "local"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.parse::<IpAddr>().is_ok_and(is_private_ip)
}

/// A `reqwest` DNS resolver that refuses to hand back private, loopback, or
/// link-local addresses. Attach it via `ClientBuilder::dns_resolver` so
/// every connection this client makes — including ones a server redirects
/// it to — is checked, not just the URL the caller started with.
#[derive(Clone, Default)]
pub(crate) struct SsrfSafeResolver;

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
                .filter(|addr| !is_private_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::other(format!(
                    "{host} has no public address"
                )))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

pub(crate) fn dns_resolver() -> Arc<SsrfSafeResolver> {
    Arc::new(SsrfSafeResolver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_localhost_and_private_hostnames() {
        assert!(is_disallowed_host("localhost"));
        assert!(is_disallowed_host("LOCALHOST."));
        assert!(is_disallowed_host("printer.local"));
        assert!(is_disallowed_host("127.0.0.1"));
        assert!(is_disallowed_host("169.254.169.254")); // cloud metadata endpoint
        assert!(is_disallowed_host("192.168.1.1"));
        assert!(!is_disallowed_host("example.com"));
        assert!(!is_disallowed_host("8.8.8.8"));
    }

    #[test]
    fn classifies_ipv4_mapped_ipv6_and_special_ranges_as_non_public() {
        for address in [
            "::ffff:127.0.0.1",   // loopback
            "::ffff:10.0.0.1",    // private
            "::ffff:192.168.1.1", // private
            "::ffff:169.254.1.1", // link-local
            "::ffff:8.8.8.8",     // IPv4-mapped IPv6 is always rejected outside 2000::/3
            "::127.0.0.1",        // IPv4-compatible IPv6
            "::169.254.169.254",
            "::8.8.8.8",
            "64:ff9b::127.0.0.1", // NAT64
            "64:ff9b::8.8.8.8",
            "64:ff9b:1::1",
            "2002:7f00:1::1", // 6to4
            "2002::1",
            "2001::1",     // IETF protocol assignments
            "2001:db8::1", // documentation
            "3fff::1",     // documentation
            "0.1.2.3",     // this network
            "100.64.0.1",  // CGNAT / Tailscale
            "100.127.255.254",
            "192.0.2.1",       // TEST-NET-1
            "198.18.0.1",      // benchmarking
            "198.19.255.254",  // benchmarking
            "198.51.100.1",    // TEST-NET-2
            "203.0.113.1",     // TEST-NET-3
            "224.0.0.1",       // multicast
            "255.255.255.255", // broadcast
        ] {
            assert!(
                is_private_ip(address.parse().unwrap()),
                "{address} must not be treated as public"
            );
        }

        for public in [
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "198.17.255.255",
            "198.20.0.0",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(
                !is_private_ip(public.parse().unwrap()),
                "{public} should be treated as public"
            );
        }
    }

    #[test]
    fn flags_ipv4_mapped_and_special_literal_hosts() {
        assert!(is_disallowed_host("::ffff:127.0.0.1"));
        assert!(is_disallowed_host("::ffff:8.8.8.8"));
        for url in [
            "http://100.64.0.1/x.png",
            "http://198.18.0.1/x.png",
            "http://[64:ff9b::127.0.0.1]/x.png",
            "http://[2002:7f00:1::1]/x.png",
            "http://[::127.0.0.1]/x.png",
        ] {
            assert!(
                is_disallowed_url_host(&Url::parse(url).unwrap()),
                "{url} must be disallowed"
            );
        }
    }
}
