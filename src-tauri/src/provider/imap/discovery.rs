//! IMAP/SMTP autodiscovery from an email domain.
//!
//! `docs/imap-design.md` ("Account setup", step 2): given an email address,
//! try to discover the server settings so the user confirms rather than types
//! them. The order and the HTTPS-only rule come straight from the design:
//!
//! 1. `autoconfig.<domain>` (Thunderbird-style ISP autoconfig);
//! 2. `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`;
//! 3. the Mozilla ISP database (`autoconfig.thunderbird.net`);
//! 4. RFC 6186/8314 SRV records — see the deferral note below;
//! 5. guarded guesses (`imap.<domain>` / `mail.<domain>`), accepted only when
//!    their TLS certificate matches the host.
//!
//! ## SSRF: discovery fetches ARE filtered; the user's mail host is NOT
//!
//! Autodiscovery fetches are driven by the EMAIL DOMAIN, which comes from the
//! address the user typed but points this client at a URL derived from it, so
//! those fetches go through the normal [`net_safety`](crate::net_safety) SSRF
//! path: the resolver refuses private, loopback and link-local addresses, and
//! every fetch is HTTPS-only. This stops a hostile `foo@127.0.0.1`-style
//! domain from turning setup into a request against the user's own network.
//!
//! The FINAL, user-chosen mail host is the opposite case and is handled in the
//! setup command, NOT here: a LAN server, a self-hosted box, or Proton Bridge
//! on `127.0.0.1` MUST be allowed, because the user entered it directly rather
//! than it being derived from untrusted content. The SSRF filter deliberately
//! does not apply there. See `docs/imap-design.md` ("Account setup", final
//! paragraph) and the comment on the setup command in `crate::lib`.
//!
//! ## SRV (step 4) deferral
//!
//! RFC 6186 SRV lookup needs a DNS SRV resolver (`hickory-resolver`), a new
//! dependency this slice does not add. Manual setup is a first-class path and
//! fully covers the primary account (Proton Bridge on a non-standard loopback
//! port, which autodiscovery from the email domain can never find anyway) and
//! every self-hosted server. The HTTPS sources above cover the common ISP
//! case. SRV is a documented follow-up within Phase 2; its absence narrows
//! autodiscovery, it does not block setup.

use std::time::Duration;

use serde::Serialize;

use crate::net_safety;
use crate::provider::imap::settings::SecurityMode;

/// How long any single discovery fetch may take, and the total connect
/// budget. Kept short: discovery is best-effort and must never stall setup.
const DISCOVERY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DISCOVERY_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// The Mozilla ISP database endpoint (HTTPS). Step 3 of the design's order.
const MOZILLA_ISPDB_BASE: &str = "https://autoconfig.thunderbird.net/v1.1/";

/// One server endpoint discovered for an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredServer {
    pub host: String,
    pub port: u16,
    pub security: SecurityMode,
    pub username: String,
}

/// The settings autodiscovery proposes, for the user to confirm or edit before
/// the password goes anywhere (`docs/imap-design.md` step 3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub imap: DiscoveredServer,
    pub smtp: DiscoveredServer,
    /// Which source answered, for display and diagnostics.
    pub source: String,
}

/// The domain part of an email address, lowercased, or an error for an address
/// with no `@`.
pub fn email_domain(email: &str) -> Result<String, String> {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim().to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| "That does not look like an email address.".to_string())
}

/// Attempt autodiscovery for `email` across the HTTPS sources, in the design's
/// order. Returns the first source that yields a usable config, or `None` when
/// none do (the UI then shows the manual form, which is a first-class path).
///
/// Every fetch here is SSRF-filtered and HTTPS-only (see the module docs).
pub async fn discover(email: &str) -> Result<Option<DiscoveryResult>, String> {
    let domain = email_domain(email)?;
    let client = ssrf_safe_client()?;

    // Source 1 + 2: ISP autoconfig at `autoconfig.<domain>` and the domain's
    // `.well-known` path. Both return the Thunderbird autoconfig XML schema.
    for url in [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
    ] {
        if let Some(result) = fetch_autoconfig(&client, &url, email, "isp-autoconfig").await {
            return Ok(Some(result));
        }
    }

    // Source 3: the Mozilla ISP database, keyed by the domain.
    let ispdb = format!("{MOZILLA_ISPDB_BASE}{domain}");
    if let Some(result) = fetch_autoconfig(&client, &ispdb, email, "mozilla-ispdb").await {
        return Ok(Some(result));
    }

    // Sources 4 (SRV) and 5 (guarded guesses) are deferred — see module docs.
    // Manual setup is the fallback and is a first-class path.
    Ok(None)
}

/// Build the SSRF-safe, HTTPS-only client discovery uses. Identical discipline
/// to the image proxy: the private-address-refusing resolver is attached to
/// the client so redirects are checked too, and `https_only` closes off a
/// plaintext downgrade.
fn ssrf_safe_client() -> Result<reqwest::Client, String> {
    crate::http_client::builder()
        .dns_resolver(net_safety::dns_resolver())
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(DISCOVERY_CONNECT_TIMEOUT)
        .timeout(DISCOVERY_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Unable to prepare autodiscovery requests: {error}"))
}

/// Fetch and parse one autoconfig XML document. Any failure (network, non-2xx,
/// unparsable, or missing the servers we need) yields `None` so the caller
/// moves on to the next source rather than failing setup.
async fn fetch_autoconfig(
    client: &reqwest::Client,
    url: &str,
    email: &str,
    source: &str,
) -> Option<DiscoveryResult> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    parse_autoconfig(&body, email, source)
}

/// Parse the Thunderbird autoconfig XML schema into a [`DiscoveryResult`].
///
/// The schema's shape (abridged):
/// ```xml
/// <clientConfig><emailProvider>
///   <incomingServer type="imap">
///     <hostname>…</hostname><port>…</port>
///     <socketType>SSL|STARTTLS</socketType>
///     <username>%EMAILADDRESS%</username>
///   </incomingServer>
///   <outgoingServer type="smtp">…</outgoingServer>
/// </emailProvider></clientConfig>
/// ```
/// We take the first usable IMAP incoming server and the first SMTP outgoing
/// server. `%EMAILADDRESS%` / `%EMAILLOCALPART%` placeholders are expanded.
fn parse_autoconfig(xml: &str, email: &str, source: &str) -> Option<DiscoveryResult> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut imap: Option<DiscoveredServer> = None;
    let mut smtp: Option<DiscoveredServer> = None;
    // Which element we're inside: "incoming"/"outgoing" plus the pending text
    // tag name, so character data lands in the right field.
    let mut server: Option<(&'static str, ParsedServer)> = None;
    let mut field: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                let name = local_name(tag.name().as_ref());
                match name.as_str() {
                    "incomingServer" => {
                        server = Some(("incoming", ParsedServer {
                            kind: attr(&tag, "type"),
                            ..ParsedServer::default()
                        }));
                    }
                    "outgoingServer" => {
                        server = Some(("outgoing", ParsedServer {
                            kind: attr(&tag, "type"),
                            ..ParsedServer::default()
                        }));
                    }
                    "hostname" | "port" | "socketType" | "username" => {
                        if server.is_some() {
                            field = Some(name);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                if let (Some((_, partial)), Some(field_name)) = (server.as_mut(), field.as_ref()) {
                    let value = text.into_inner().trim().to_string();
                    match field_name.as_str() {
                        "hostname" => partial.hostname = Some(value),
                        "port" => partial.port = value.parse().ok(),
                        "socketType" => partial.socket = Some(value),
                        "username" => partial.username = Some(expand_placeholders(&value, email)),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(tag)) => {
                let name = local_name(tag.name().as_ref());
                if name == "incomingServer" || name == "outgoingServer" {
                    if let Some((role, partial)) = server.take() {
                        if let Some(resolved) = resolve_server(partial, email) {
                            match role {
                                "incoming" if imap.is_none() => imap = Some(resolved),
                                "outgoing" if smtp.is_none() => smtp = Some(resolved),
                                _ => {}
                            }
                        }
                    }
                    field = None;
                } else if Some(name) == field {
                    field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }

    Some(DiscoveryResult {
        imap: imap?,
        smtp: smtp?,
        source: source.to_string(),
    })
}

/// Turn a parsed server element into a [`DiscoveredServer`], keeping only IMAP
/// incoming servers and SMTP outgoing servers over a TLS socket type. A
/// plaintext (`socketType=plain`) server is dropped — the design forbids a
/// plaintext fallback.
fn resolve_server(partial: ParsedServer, email: &str) -> Option<DiscoveredServer> {
    // `kind` is "imap"/"smtp"; a POP incoming server is ignored.
    let kind = partial.kind?.to_ascii_lowercase();
    if kind != "imap" && kind != "smtp" {
        return None;
    }
    let security = match partial.socket?.to_ascii_uppercase().as_str() {
        "SSL" | "TLS" => SecurityMode::ImplicitTls,
        "STARTTLS" => SecurityMode::StartTls,
        // "plain" or anything else: refuse — no plaintext fallback.
        _ => return None,
    };
    Some(DiscoveredServer {
        host: partial.hostname?,
        port: partial.port?,
        security,
        username: partial.username.unwrap_or_else(|| email.to_string()),
    })
}

/// A parsed server element from the autoconfig XML, before validation.
#[derive(Default)]
struct ParsedServer {
    kind: Option<String>,
    hostname: Option<String>,
    port: Option<u16>,
    socket: Option<String>,
    username: Option<String>,
}

fn expand_placeholders(value: &str, email: &str) -> String {
    let local = email.rsplit_once('@').map(|(l, _)| l).unwrap_or(email);
    value
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local)
}

/// The element's local name without any namespace prefix.
fn local_name(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or(raw).to_string()
}

fn attr(tag: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    tag.attributes().flatten().find_map(|a| {
        if local_name(a.key.as_ref()) == key {
            Some(a.value.as_ref().to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_domain_is_extracted_and_lowercased() {
        assert_eq!(email_domain("Me@Example.COM").unwrap(), "example.com");
        assert!(email_domain("not-an-email").is_err());
        assert!(email_domain("trailing@").is_err());
    }

    const SAMPLE: &str = r#"
      <clientConfig version="1.1">
        <emailProvider id="example.com">
          <incomingServer type="pop3">
            <hostname>pop.example.com</hostname><port>995</port>
            <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
          </incomingServer>
          <incomingServer type="imap">
            <hostname>imap.example.com</hostname><port>993</port>
            <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
          </incomingServer>
          <outgoingServer type="smtp">
            <hostname>smtp.example.com</hostname><port>587</port>
            <socketType>STARTTLS</socketType><username>%EMAILLOCALPART%</username>
          </outgoingServer>
        </emailProvider>
      </clientConfig>"#;

    #[test]
    fn parses_imap_and_smtp_skipping_pop_and_expanding_placeholders() {
        let result = parse_autoconfig(SAMPLE, "grace@example.com", "test").unwrap();
        assert_eq!(
            result.imap,
            DiscoveredServer {
                host: "imap.example.com".into(),
                port: 993,
                security: SecurityMode::ImplicitTls,
                username: "grace@example.com".into(),
            }
        );
        assert_eq!(
            result.smtp,
            DiscoveredServer {
                host: "smtp.example.com".into(),
                port: 587,
                security: SecurityMode::StartTls,
                // %EMAILLOCALPART% expands to the local part only.
                username: "grace".into(),
            }
        );
        assert_eq!(result.source, "test");
    }

    #[test]
    fn a_plaintext_only_server_is_refused() {
        let xml = r#"<clientConfig><emailProvider>
          <incomingServer type="imap"><hostname>imap.example.com</hostname>
          <port>143</port><socketType>plain</socketType></incomingServer>
          <outgoingServer type="smtp"><hostname>smtp.example.com</hostname>
          <port>25</port><socketType>plain</socketType></outgoingServer>
        </emailProvider></clientConfig>"#;
        // No TLS server survives, so there is nothing to propose.
        assert!(parse_autoconfig(xml, "x@example.com", "test").is_none());
    }

    #[test]
    fn missing_outgoing_server_yields_no_result() {
        let xml = r#"<clientConfig><emailProvider>
          <incomingServer type="imap"><hostname>imap.example.com</hostname>
          <port>993</port><socketType>SSL</socketType></incomingServer>
        </emailProvider></clientConfig>"#;
        assert!(parse_autoconfig(xml, "x@example.com", "test").is_none());
    }
}
