//! Safe execution of unsubscribe actions extracted from message headers.
//!
//! The webview passes a cached message ID to the native layer. This module
//! never accepts a caller-supplied URL for an external side effect.

use std::time::Duration;

use reqwest::{header::CONTENT_TYPE, redirect::Policy};
use url::Url;

use crate::models::{UnsubscribeMethod, UnsubscribeResult, UnsubscribeTarget};
use crate::net_safety::{self, is_disallowed_url_host};

const ONE_CLICK_BODY: &str = "List-Unsubscribe=One-Click";

/// `open_mailto` receives a validated mailto fallback so it can start a draft
/// in ThreeStrands instead of leaving for the OS mail handler.
pub async fn execute(
    target: &UnsubscribeTarget,
    open_mailto: impl FnOnce(&str),
) -> Result<UnsubscribeResult, String> {
    match target.method {
        UnsubscribeMethod::OneClick => execute_one_click(target).await,
        UnsubscribeMethod::Mailto | UnsubscribeMethod::Web => open_fallback(target, open_mailto),
    }
}

async fn execute_one_click(target: &UnsubscribeTarget) -> Result<UnsubscribeResult, String> {
    let url = validate_https_url(&target.url)?;
    let client = crate::http_client::builder()
        .dns_resolver(net_safety::dns_resolver())
        .redirect(Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Unable to prepare unsubscribe request: {error}"))?;
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(ONE_CLICK_BODY)
        .send()
        .await
        .map_err(|error| format!("Unsubscribe request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        if status.is_redirection() {
            return Err(
                "Unsubscribe endpoint returned a redirect; open the preferences page instead"
                    .into(),
            );
        }
        return Err(format!("Unsubscribe endpoint returned HTTP {status}"));
    }
    Ok(UnsubscribeResult {
        method: UnsubscribeMethod::OneClick,
        outcome: "requested".into(),
        http_status: Some(status.as_u16()),
    })
}

fn open_fallback(
    target: &UnsubscribeTarget,
    open_mailto: impl FnOnce(&str),
) -> Result<UnsubscribeResult, String> {
    let url = Url::parse(&target.url).map_err(|_| "Invalid unsubscribe URL".to_string())?;
    match target.method {
        UnsubscribeMethod::Mailto if url.scheme().eq_ignore_ascii_case("mailto") => {
            open_mailto(&target.url);
        }
        UnsubscribeMethod::Web => {
            validate_https_url(&target.url)?;
            open::that(&target.url)
                .map_err(|error| format!("Unable to open unsubscribe option: {error}"))?;
        }
        _ => return Err("Invalid unsubscribe fallback".to_string()),
    }
    Ok(UnsubscribeResult {
        method: target.method.clone(),
        outcome: "opened".into(),
        http_status: None,
    })
}

fn validate_https_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Invalid HTTPS unsubscribe URL".to_string())?;
    if !url.scheme().eq_ignore_ascii_case("https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Unsubscribe requires a safe HTTPS URL".to_string());
    }
    if is_disallowed_url_host(&url) {
        return Err("Unsubscribe URL points to a local or private host".to_string());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_public_https_endpoints() {
        assert!(validate_https_url("https://lists.example/unsubscribe").is_ok());
        assert!(validate_https_url("http://lists.example/unsubscribe").is_err());
        assert!(validate_https_url("https://localhost/unsubscribe").is_err());
        assert!(validate_https_url("https://127.0.0.1/unsubscribe").is_err());
        assert!(validate_https_url("https://lists.example:8443/unsubscribe").is_err());
    }

    #[test]
    fn rejects_private_ipv6_literal_hosts() {
        for url in [
            "https://[::1]/unsubscribe",
            "https://[fd00::1]/unsubscribe",
            "https://[::ffff:127.0.0.1]/unsubscribe",
        ] {
            assert_eq!(
                validate_https_url(url).unwrap_err(),
                "Unsubscribe URL points to a local or private host",
                "{url} must be rejected"
            );
        }
        assert!(validate_https_url("https://[2606:4700:4700::1111]/unsubscribe").is_ok());
    }

    #[test]
    fn rejects_credentials_and_fragments() {
        assert!(validate_https_url("https://user:pass@lists.example/unsubscribe").is_err());
        assert!(validate_https_url("https://lists.example/unsubscribe#confirm").is_err());
    }

    fn target(method: UnsubscribeMethod, url: &str) -> UnsubscribeTarget {
        UnsubscribeTarget { request_id: "request".into(), method, url: url.into() }
    }

    #[test]
    fn mailto_fallback_starts_an_in_app_draft() {
        let mut opened = None;
        let result = open_fallback(
            &target(UnsubscribeMethod::Mailto, "mailto:leave@lists.example?subject=unsubscribe"),
            |url| opened = Some(url.to_string()),
        )
        .unwrap();
        assert_eq!(opened.as_deref(), Some("mailto:leave@lists.example?subject=unsubscribe"));
        assert_eq!(result.outcome, "opened");
    }

    #[test]
    fn rejects_a_fallback_whose_url_does_not_match_its_method() {
        let mut opened = false;
        assert!(open_fallback(
            &target(UnsubscribeMethod::Mailto, "https://lists.example/unsubscribe"),
            |_| opened = true,
        )
        .is_err());
        assert!(open_fallback(
            &target(UnsubscribeMethod::Web, "mailto:leave@lists.example"),
            |_| opened = true,
        )
        .is_err());
        assert!(!opened);
    }

    #[test]
    fn one_click_body_is_the_rfc_value() {
        assert_eq!(ONE_CLICK_BODY, "List-Unsubscribe=One-Click");
        assert_eq!(reqwest::StatusCode::OK.as_u16(), 200);
    }
}
