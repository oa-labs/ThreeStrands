//! Sender trust signals read from a message's stored headers: the receiving
//! server's authentication verdicts and a Reply-To that leaves the sender's
//! domain. Nothing here contacts the network or re-verifies a signature; the
//! context panel reports what the user's mail server already concluded.

use crate::mime::{MimePart, RawMessage};
use serde::Serialize;

/// Authentication verdicts from one `Authentication-Results` header
/// (RFC 8601). Each verdict is `pass` when any result for that method
/// passed, otherwise the first result reported, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationVerdicts {
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    /// Signing domains of the DKIM signatures that passed, lowercased.
    pub dkim_domains: Vec<String>,
    /// Envelope sender (`smtp.mailfrom`) domains of the SPF checks that
    /// passed, lowercased.
    pub spf_domains: Vec<String>,
}

/// What the context panel's Trust section reads about one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageTrust {
    /// The From address's domain, lowercased; empty when From has no address.
    pub from_domain: String,
    /// Absent when the receiving server reported no authentication results.
    pub authentication: Option<AuthenticationVerdicts>,
    /// The Reply-To address, only when it belongs to another organization
    /// than From (a subdomain of the same domain does not count).
    pub reply_to: Option<String>,
}

/// Reads the trust signals from a stored provider payload. Only the topmost
/// `Authentication-Results` header counts: it is the one the user's own
/// receiving server added last, while lower copies arrived with the message
/// and could have been written by anyone along the way.
pub fn message_trust(message: &RawMessage) -> MessageTrust {
    let payload = &message.payload;
    let from_domain = header(payload, "From")
        .map(address_of)
        .map(|address| domain_of(&address))
        .unwrap_or_default();
    let authentication = header(payload, "Authentication-Results").and_then(authentication_verdicts);
    let reply_to = header(payload, "Reply-To")
        .map(address_of)
        .filter(|address| address.contains('@'))
        .filter(|address| !from_domain.is_empty() && !same_organization(&domain_of(address), &from_domain));
    MessageTrust { from_domain, authentication, reply_to }
}

/// Parses one `Authentication-Results` value. Returns None when it reports
/// no SPF, DKIM, or DMARC result, such as the bare `authserv-id; none`.
pub fn authentication_verdicts(value: &str) -> Option<AuthenticationVerdicts> {
    let mut segments = split_results(value).into_iter();
    // The first segment names the server that did the checking.
    segments.next()?;
    let mut spf = Vec::new();
    let mut dkim = Vec::new();
    let mut dmarc = Vec::new();
    let mut dkim_domains = Vec::new();
    let mut spf_domains = Vec::new();
    for segment in segments {
        let mut tokens = segment.split_whitespace();
        let Some((method, result)) = tokens.next().and_then(|token| token.split_once('=')) else {
            continue;
        };
        let method = method.split('/').next().unwrap_or(method).to_ascii_lowercase();
        let result = result.trim_matches('"').to_ascii_lowercase();
        match method.as_str() {
            "spf" => {
                if result == "pass" {
                    // smtp.mailfrom may be a full address or a bare domain.
                    let domain = tokens
                        .filter_map(|token| token.split_once('='))
                        .find(|(key, _)| key.eq_ignore_ascii_case("smtp.mailfrom"))
                        .map(|(_, value)| value.trim_matches('"').to_ascii_lowercase())
                        .map(|value| value.rsplit_once('@').map(|(_, domain)| domain.to_string()).unwrap_or(value));
                    if let Some(domain) = domain.filter(|domain| !domain.is_empty()) {
                        if !spf_domains.contains(&domain) {
                            spf_domains.push(domain);
                        }
                    }
                }
                spf.push(result);
            }
            "dmarc" => dmarc.push(result),
            "dkim" => {
                if result == "pass" {
                    let properties: Vec<(String, String)> = tokens
                        .filter_map(|token| token.split_once('='))
                        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim_matches('"').to_ascii_lowercase()))
                        .collect();
                    let signer = properties.iter().find(|(key, _)| key == "header.d").map(|(_, value)| value.clone())
                        .or_else(|| properties.iter().find(|(key, _)| key == "header.i").map(|(_, value)| domain_of(value)));
                    if let Some(domain) = signer.filter(|domain| !domain.is_empty()) {
                        if !dkim_domains.contains(&domain) {
                            dkim_domains.push(domain);
                        }
                    }
                }
                dkim.push(result);
            }
            _ => {}
        }
    }
    if spf.is_empty() && dkim.is_empty() && dmarc.is_empty() {
        return None;
    }
    Some(AuthenticationVerdicts { spf: verdict(spf), dkim: verdict(dkim), dmarc: verdict(dmarc), dkim_domains, spf_domains })
}

fn verdict(results: Vec<String>) -> Option<String> {
    if results.iter().any(|result| result == "pass") {
        return Some("pass".into());
    }
    results.into_iter().next()
}

/// Splits a header value on `;`, dropping parenthesized comments (which may
/// nest and may themselves contain `;`) and keeping quoted strings intact.
fn split_results(value: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            if depth == 0 {
                current.push(character);
            }
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted || depth > 0 => {
                escaped = true;
                if depth == 0 {
                    current.push(character);
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                current.push(character);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => {
                depth -= 1;
                // A comment separates tokens the way whitespace does.
                if depth == 0 {
                    current.push(' ');
                }
            }
            ';' if !quoted && depth == 0 => segments.push(std::mem::take(&mut current)),
            _ if depth == 0 => current.push(character),
            _ => {}
        }
    }
    segments.push(current);
    segments.into_iter().map(|segment| segment.trim().to_string()).filter(|segment| !segment.is_empty()).collect()
}

fn header<'a>(part: &'a MimePart, name: &str) -> Option<&'a str> {
    part.headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

/// The first address in an address header, lowercased.
fn address_of(value: &str) -> String {
    let first = value.split(',').next().unwrap_or_default().trim();
    let candidate = match (first.rfind('<'), first.rfind('>')) {
        (Some(open), Some(close)) if close > open => &first[open + 1..close],
        _ => first,
    };
    candidate.trim().trim_matches(|character| character == '"' || character == '\'').to_ascii_lowercase()
}

fn domain_of(address: &str) -> String {
    address.rsplit_once('@').map(|(_, domain)| domain.trim().to_ascii_lowercase()).unwrap_or_default()
}

/// Whether two domains belong to the same organization: equal, or one a
/// subdomain of the other (`mail.example.com` and `example.com`).
fn same_organization(left: &str, right: &str) -> bool {
    left == right || left.ends_with(&format!(".{right}")) || right.ends_with(&format!(".{left}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::MimeHeader;

    fn raw(headers: &[(&str, &str)]) -> RawMessage {
        RawMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: String::new(),
            payload: MimePart {
                headers: headers.iter().map(|(name, value)| MimeHeader { name: (*name).into(), value: (*value).into() }).collect(),
                ..MimePart::default()
            },
        }
    }

    #[test]
    fn reads_every_method_from_a_typical_receiving_server_header() {
        let verdicts = authentication_verdicts(
            "mx.example.net; dkim=pass header.i=@news.example.com header.s=s1 header.b=abc; \
             spf=pass (example.net: domain of bounce@news.example.com designates 192.0.2.1 as permitted sender) smtp.mailfrom=bounce@news.example.com; \
             dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=example.com",
        )
        .unwrap();
        assert_eq!(verdicts.spf.as_deref(), Some("pass"));
        assert_eq!(verdicts.dkim.as_deref(), Some("pass"));
        assert_eq!(verdicts.dmarc.as_deref(), Some("pass"));
        assert_eq!(verdicts.dkim_domains, vec!["news.example.com".to_string()]);
        assert_eq!(verdicts.spf_domains, vec!["news.example.com".to_string()]);
    }

    #[test]
    fn lists_the_envelope_domain_of_each_passing_spf_check_once() {
        // A relay that reports no DMARC result, as for forwarded or filtered mail.
        let verdicts = authentication_verdicts(
            "mx.example.net; dkim=pass header.i=@tenant.mailhost.example header.s=s2; \
             arc=pass (i=1 spf=pass spfdomain=other.example dmarc=pass fromdomain=other.example); \
             spf=pass (mx.example.net: domain of Pat@Partner.example designates 192.0.2.7 as permitted sender) smtp.mailfrom=Pat@Partner.example",
        )
        .unwrap();
        assert_eq!(verdicts.dmarc, None);
        assert_eq!(verdicts.spf_domains, vec!["partner.example".to_string()]);
        assert_eq!(verdicts.dkim_domains, vec!["tenant.mailhost.example".to_string()]);

        let bare = authentication_verdicts("mx.example; spf=pass smtp.mailfrom=bounces.shop.example; spf=pass smtp.mailfrom=x@bounces.shop.example").unwrap();
        assert_eq!(bare.spf_domains, vec!["bounces.shop.example".to_string()]);

        let failed = authentication_verdicts("mx.example; spf=softfail smtp.mailfrom=a@partner.example; spf=pass").unwrap();
        assert_eq!(failed.spf.as_deref(), Some("pass"));
        assert!(failed.spf_domains.is_empty());
    }

    #[test]
    fn any_passing_signature_passes_and_lists_each_signer_once() {
        let verdicts = authentication_verdicts(
            "relay.example; dkim=fail header.d=example.org; dkim=pass header.d=ESP.example; dkim=pass header.d=esp.example; spf=softfail smtp.mailfrom=example.org",
        )
        .unwrap();
        assert_eq!(verdicts.dkim.as_deref(), Some("pass"));
        assert_eq!(verdicts.dkim_domains, vec!["esp.example".to_string()]);
        assert_eq!(verdicts.spf.as_deref(), Some("softfail"));
        assert_eq!(verdicts.dmarc, None);
    }

    #[test]
    fn failures_and_versioned_methods_keep_the_reported_result() {
        let verdicts = authentication_verdicts("mx.example; dmarc/1=FAIL header.from=example.com; spf=none").unwrap();
        assert_eq!(verdicts.dmarc.as_deref(), Some("fail"));
        assert_eq!(verdicts.spf.as_deref(), Some("none"));
        assert_eq!(verdicts.dkim, None);
        assert!(verdicts.dkim_domains.is_empty());
    }

    #[test]
    fn a_semicolon_inside_a_comment_or_quotes_does_not_start_a_result() {
        let verdicts = authentication_verdicts(
            "mx.example; spf=fail (sender; not permitted (see \"policy; v2\")) smtp.mailfrom=a@example.com; dkim=pass header.d=\"example.com\"",
        )
        .unwrap();
        assert_eq!(verdicts.spf.as_deref(), Some("fail"));
        assert_eq!(verdicts.dkim.as_deref(), Some("pass"));
        assert_eq!(verdicts.dkim_domains, vec!["example.com".to_string()]);
    }

    #[test]
    fn a_server_that_checked_nothing_reports_no_verdicts() {
        assert_eq!(authentication_verdicts("mx.example; none"), None);
        assert_eq!(authentication_verdicts(""), None);
        assert_eq!(authentication_verdicts("mx.example"), None);
    }

    #[test]
    fn only_the_topmost_results_header_counts() {
        // A lower copy travelled with the message and is not trusted.
        let trust = message_trust(&raw(&[
            ("Authentication-Results", "mx.receiver.example; dmarc=fail header.from=bank.example"),
            ("Authentication-Results", "forged.example; dmarc=pass header.from=bank.example"),
            ("From", "Bank <alerts@bank.example>"),
        ]));
        assert_eq!(trust.authentication.unwrap().dmarc.as_deref(), Some("fail"));
        assert_eq!(trust.from_domain, "bank.example");
    }

    #[test]
    fn reply_to_is_reported_only_when_it_leaves_the_senders_organization() {
        let elsewhere = message_trust(&raw(&[
            ("From", "Support <help@shop.example>"),
            ("Reply-To", "\"Support\" <Claims@Other.example>"),
        ]));
        assert_eq!(elsewhere.reply_to.as_deref(), Some("claims@other.example"));
        assert_eq!(elsewhere.authentication, None);

        let subdomain = message_trust(&raw(&[
            ("From", "News <news@example.com>"),
            ("Reply-To", "reply@mail.example.com"),
        ]));
        assert_eq!(subdomain.reply_to, None);

        let parent = message_trust(&raw(&[("From", "a@mail.example.com"), ("Reply-To", "b@example.com")]));
        assert_eq!(parent.reply_to, None);

        let lookalike = message_trust(&raw(&[("From", "a@example.com"), ("Reply-To", "b@notexample.com")]));
        assert_eq!(lookalike.reply_to.as_deref(), Some("b@notexample.com"));
    }

    #[test]
    fn a_message_without_headers_reports_nothing() {
        let trust = message_trust(&raw(&[]));
        assert_eq!(trust, MessageTrust { from_domain: String::new(), authentication: None, reply_to: None });
    }
}
