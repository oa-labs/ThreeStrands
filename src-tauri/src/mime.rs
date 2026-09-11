use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub internal_date: String,
    pub payload: MimePart,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MimePart {
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub headers: Vec<MimeHeader>,
    #[serde(default)]
    pub body: MimeBody,
    #[serde(default)]
    pub parts: Vec<MimePart>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MimeHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MimeBody {
    pub data: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormalizedMessage {
    pub id: String,
    pub thread_id: String,
    pub subject: String,
    pub from: String,
    pub to: Vec<String>,
    pub date: String,
    pub body_html: String,
    pub body_text: String,
    pub snippet: String,
    pub labels: Vec<String>,
}

pub fn normalize(message: &GmailMessage) -> Result<NormalizedMessage, String> {
    let mut html = None;
    let mut text = None;
    select_bodies(&message.payload, &mut html, &mut text)?;
    Ok(NormalizedMessage {
        id: message.id.clone(),
        thread_id: message.thread_id.clone(),
        subject: header(&message.payload, "Subject")
            .unwrap_or("(no subject)")
            .to_string(),
        from: header(&message.payload, "From")
            .unwrap_or_default()
            .to_string(),
        to: split_addresses(header(&message.payload, "To").unwrap_or_default()),
        date: header(&message.payload, "Date")
            .and_then(normalize_date)
            .unwrap_or_else(|| millis_to_rfc3339(&message.internal_date)),
        body_html: html.unwrap_or_default(),
        body_text: text.unwrap_or_default(),
        snippet: message.snippet.clone(),
        labels: message.label_ids.clone(),
    })
}

fn select_bodies(
    part: &MimePart,
    html: &mut Option<String>,
    text: &mut Option<String>,
) -> Result<(), String> {
    // A named part is an attachment even when its content type is text.
    if part.filename.is_empty() {
        if let Some(data) = part.body.data.as_deref() {
            let decoded = decode(data)?;
            match part.mime_type.as_str() {
                "text/html" if html.is_none() => *html = Some(decoded),
                "text/plain" if text.is_none() => *text = Some(decoded),
                _ => {}
            }
        }
    }
    for child in &part.parts {
        select_bodies(child, html, text)?;
    }
    Ok(())
}

fn decode(value: &str) -> Result<String, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .map_err(|error| format!("Invalid Gmail base64url body: {error}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn header<'a>(part: &'a MimePart, name: &str) -> Option<&'a str> {
    part.headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn split_addresses(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn millis_to_rfc3339(value: &str) -> String {
    value
        .parse::<i64>()
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

fn normalize_date(value: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc2822(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|date| date.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    fn part(kind: &str, body: &str) -> MimePart {
        MimePart {
            mime_type: kind.into(),
            body: MimeBody {
                data: Some(URL_SAFE_NO_PAD.encode(body)),
            },
            ..Default::default()
        }
    }

    #[test]
    fn selects_plain_and_html_from_nested_multipart_and_ignores_attachments() {
        let message = GmailMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec!["INBOX".into()],
            snippet: "hello".into(),
            internal_date: "0".into(),
            payload: MimePart {
                headers: vec![
                    MimeHeader {
                        name: "subject".into(),
                        value: "A subject".into(),
                    },
                    MimeHeader {
                        name: "From".into(),
                        value: "a@example.com".into(),
                    },
                ],
                parts: vec![
                    part("text/plain", "plain"),
                    MimePart {
                        parts: vec![part("text/html", "<b>html</b>")],
                        ..Default::default()
                    },
                    MimePart {
                        filename: "notes.txt".into(),
                        ..part("text/plain", "attachment")
                    },
                ],
                ..Default::default()
            },
        };
        let normalized = normalize(&message).unwrap();
        assert_eq!(normalized.subject, "A subject");
        assert_eq!(normalized.body_text, "plain");
        assert_eq!(normalized.body_html, "<b>html</b>");
    }

    #[test]
    fn rejects_invalid_base64_at_provider_boundary() {
        let mut message = GmailMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: String::new(),
            payload: part("text/plain", "ok"),
        };
        message.payload.body.data = Some("%%%".into());
        assert!(normalize(&message).is_err());
    }
}
