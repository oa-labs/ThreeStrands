//! Safe conferencing join-URL extraction for calendar events shown in the
//! menu bar.
//!
//! Only strictly validated `https://` URLs without embedded credentials or
//! control characters are ever accepted as join links.

/// A validated meeting join link extracted from a calendar event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinLink {
    pub url: String,
    pub provider: Option<&'static str>,
}

const KNOWN_CONFERENCING_HOSTS: &[(&str, &str)] = &[
    ("meet.google.com", "Google Meet"),
    ("zoom.us", "Zoom"),
    ("zoom.com", "Zoom"),
    ("zoomgov.com", "Zoom"),
    ("teams.microsoft.com", "Teams"),
    ("teams.live.com", "Teams"),
    ("webex.com", "Webex"),
    ("discord.com", "Discord"),
    ("discord.gg", "Discord"),
    ("chime.aws", "Chime"),
    ("meet.goto.com", "GoTo"),
    ("gotomeeting.com", "GoTo"),
    ("meet.jit.si", "Jitsi"),
    ("whereby.com", "Whereby"),
    ("around.co", "Around"),
    ("tuple.app", "Tuple"),
    ("loom.com", "Loom"),
];

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// Validates that `raw` is a safe `https://` URL with a host and no credentials
/// or control characters.
pub fn validate_join_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.to_string())
}

/// Returns the user-facing conferencing service name for a validated URL, if recognized.
pub fn provider_label(url: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "app.slack.com" && parsed.path().starts_with("/huddle") {
        return Some("Slack");
    }
    KNOWN_CONFERENCING_HOSTS
        .iter()
        .find_map(|(domain, label)| host_matches(&host, domain).then_some(*label))
}

fn strip_trailing_url_punctuation(token: &str) -> &str {
    token.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\''])
}

fn extract_https_tokens(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for piece in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'')) {
        if piece.to_ascii_lowercase().starts_with("https://") {
            let cleaned = strip_trailing_url_punctuation(piece);
            if let Some(valid) = validate_join_url(cleaned) {
                urls.push(valid);
            }
        }
    }
    urls
}

fn extract_href(tag: &str) -> Option<&str> {
    let bytes = tag.as_bytes();
    let (idx, _) = bytes.windows(4).enumerate().find(|(i, w)| {
        w.eq_ignore_ascii_case(b"href") && (*i == 0 || bytes[*i - 1].is_ascii_whitespace())
    })?;
    let after = tag[idx + 4..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    if let Some(rest) = after.strip_prefix('"') {
        rest.split('"').next()
    } else if let Some(rest) = after.strip_prefix('\'') {
        rest.split('\'').next()
    } else {
        after.split_whitespace().next()
    }
}

fn flatten_description(description: &str) -> String {
    let unescaped = description.replace("&amp;", "&").replace("&AMP;", "&");
    let mut out = String::with_capacity(unescaped.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for ch in unescaped.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                if let Some(href) = extract_href(&tag_buf) {
                    out.push(' ');
                    out.push_str(href);
                    out.push(' ');
                }
                let lower = tag_buf.trim().to_ascii_lowercase();
                if lower.starts_with("br")
                    || lower.starts_with("/p")
                    || lower.starts_with("/div")
                    || lower.starts_with("/li")
                {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                tag_buf.clear();
            } else {
                tag_buf.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else {
            out.push(ch);
        }
    }
    out
}

fn labeled_join_url(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let (prefix, rest) = trimmed.split_once(':')?;
    let lower_prefix = prefix.trim().to_ascii_lowercase();
    let is_join_label = lower_prefix == "join"
        || lower_prefix.starts_with("join with ")
        || lower_prefix == "meeting link"
        || lower_prefix == "video call";
    if !is_join_label {
        return None;
    }
    let token = rest.trim().split_whitespace().next()?;
    let cleaned = strip_trailing_url_punctuation(token);
    validate_join_url(cleaned)
}

/// Extracts a safe `https://` meeting join link from calendar event fields in
/// priority order: explicit `conference_url`, `location`, then `description`.
pub fn extract_join_link(
    conference_url: Option<&str>,
    location: Option<&str>,
    description: Option<&str>,
) -> Option<JoinLink> {
    if let Some(valid) = conference_url.and_then(validate_join_url) {
        return Some(JoinLink {
            provider: provider_label(&valid),
            url: valid,
        });
    }
    if let Some(loc) = location {
        let candidates = extract_https_tokens(loc);
        if let Some(known) = candidates.iter().find(|u| provider_label(u).is_some()) {
            return Some(JoinLink {
                provider: provider_label(known),
                url: known.clone(),
            });
        }
        if let Some(first) = candidates.into_iter().next() {
            return Some(JoinLink {
                provider: provider_label(&first),
                url: first,
            });
        }
    }
    if let Some(desc) = description {
        let plain = flatten_description(desc);
        for line in plain.lines() {
            if let Some(valid) = labeled_join_url(line) {
                return Some(JoinLink {
                    provider: provider_label(&valid),
                    url: valid,
                });
            }
        }
        for candidate in extract_https_tokens(&plain) {
            if let Some(provider) = provider_label(&candidate) {
                return Some(JoinLink {
                    provider: Some(provider),
                    url: candidate,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_conference_url_then_location_then_description() {
        let link = extract_join_link(
            Some("https://meet.google.com/abc-defg-hij"),
            Some("https://zoom.us/j/123456"),
            Some("Join: https://teams.microsoft.com/l/meetup-join/xyz"),
        )
        .unwrap();
        assert_eq!(link.url, "https://meet.google.com/abc-defg-hij");
        assert_eq!(link.provider, Some("Google Meet"));

        let from_location = extract_join_link(
            None,
            Some("Room 4B or https://us02web.zoom.us/j/987654321."),
            Some("Agenda: https://docs.example.com/notes"),
        )
        .unwrap();
        assert_eq!(from_location.url, "https://us02web.zoom.us/j/987654321");
        assert_eq!(from_location.provider, Some("Zoom"));

        let from_desc = extract_join_link(
            None,
            Some("Conference Room A"),
            Some("Agenda: https://example.com/doc\nJoin with Zoom: https://zoom.us/j/111222\n"),
        )
        .unwrap();
        assert_eq!(from_desc.url, "https://zoom.us/j/111222");
        assert_eq!(from_desc.provider, Some("Zoom"));
    }

    #[test]
    fn extracts_conferencing_providers_and_html_descriptions() {
        let teams = extract_join_link(
            None,
            None,
            Some("<p>Daily standup<br/><a href=\"https://teams.microsoft.com/l/meetup-join/abc\">Join</a></p>"),
        )
        .unwrap();
        assert_eq!(teams.provider, Some("Teams"));

        let webex = extract_join_link(
            None,
            None,
            Some("Meeting link: https://acme.webex.com/meet/jdoe"),
        )
        .unwrap();
        assert_eq!(webex.provider, Some("Webex"));

        let slack = extract_join_link(
            None,
            None,
            Some("Hop on https://app.slack.com/huddle/T123/C456"),
        )
        .unwrap();
        assert_eq!(slack.provider, Some("Slack"));

        let chime = extract_join_link(Some("https://chime.aws/1234567890"), None, None).unwrap();
        assert_eq!(chime.provider, Some("Chime"));

        // Non-meeting URLs inside description text are ignored unless explicitly labeled.
        let doc_only = extract_join_link(
            None,
            Some("HQ 2nd floor"),
            Some("Read the spec at https://docs.example.com/spec before we meet."),
        );
        assert_eq!(doc_only, None);
    }

    #[test]
    fn rejects_unsafe_or_malformed_join_urls() {
        for bad in [
            "",
            "   ",
            "http://meet.google.com/abc-defg-hij",
            "javascript:alert(1)",
            "file:///Applications/Calculator.app",
            "data:text/html,<script>alert(1)</script>",
            "zoommtg://zoom.us/join?confno=123",
            "https://user:pass@meet.google.com/abc",
            "https://user@meet.google.com/abc",
            "https://meet.google.com/abc\nopen",
            "https://meet.google.com/abc\x00def",
        ] {
            assert_eq!(
                validate_join_url(bad),
                None,
                "expected rejection for {bad:?}"
            );
            assert_eq!(extract_join_link(Some(bad), None, None), None);
        }
    }
}
