//! Shared SSRF defenses for the two places this app makes outbound requests
//! to URLs found inside message content rather than URLs we control
//! ourselves: one-click unsubscribe ([`crate::unsubscribe`]) and the remote
//! image proxy ([`crate::image_proxy`]).
//!
//! A URL string can look like it points at a public host and still resolve
//! (immediately, or on a later request via DNS rebinding) to a private or
//! loopback address reachable only from this machine. Rejecting the literal
//! hostname string is a fast first check but not sufficient on its own, so
//! [`dns_resolver`] also filters every address a hostname resolves to at
//! connect time — including on redirects, since it's attached to the
//! `reqwest::Client` itself rather than checked once up front.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

fn is_private_ipv4(address: Ipv4Addr) -> bool {
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
}

pub(crate) fn is_private_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_private_ipv4(address),
        IpAddr::V6(address) => address.to_ipv4_mapped().map_or_else(
            || {
                address.is_loopback()
                    || address.is_unspecified()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
            },
            is_private_ipv4,
        ),
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
    fn classifies_ipv4_mapped_ipv6_by_its_embedded_ipv4_address() {
        for address in [
            "::ffff:127.0.0.1",   // loopback
            "::ffff:10.0.0.1",    // private
            "::ffff:192.168.1.1", // private
            "::ffff:169.254.1.1", // link-local
        ] {
            assert!(
                is_private_ip(address.parse().unwrap()),
                "{address} must not be treated as public"
            );
        }

        assert!(!is_private_ip("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn flags_ipv4_mapped_ipv6_literal_hosts() {
        assert!(is_disallowed_host("::ffff:127.0.0.1"));
        assert!(!is_disallowed_host("::ffff:8.8.8.8"));
    }
}
