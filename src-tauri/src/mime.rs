use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MimeHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MimeBody {
    pub data: Option<String>,
    #[serde(default, alias = "attachment_id")]
    pub attachment_id: Option<String>,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeMetadata {
    pub one_click_url: Option<String>,
    pub mailto_url: Option<String>,
    pub web_url: Option<String>,
    pub list_id: Option<String>,
}

impl UnsubscribeMetadata {
    pub fn info(&self) -> crate::models::UnsubscribeInfo {
        let mut methods = Vec::new();
        if self.one_click_url.is_some() {
            methods.push(crate::models::UnsubscribeMethod::OneClick);
        }
        if self.mailto_url.is_some() {
            methods.push(crate::models::UnsubscribeMethod::Mailto);
        }
        if self.web_url.is_some() {
            methods.push(crate::models::UnsubscribeMethod::Web);
        }
        crate::models::UnsubscribeInfo {
            methods,
            list_id: self.list_id.clone(),
        }
    }
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
    pub metadata_json: String,
    pub unsubscribe: Option<UnsubscribeMetadata>,
    pub attachments: Vec<crate::models::MessageAttachment>,
}

pub fn normalize(message: &GmailMessage) -> Result<NormalizedMessage, String> {
    let mut html = None;
    let mut text = None;
    select_bodies(&message.payload, &mut html, &mut text)?;
    let unsubscribe = unsubscribe_metadata(&message.payload);
    let mut attachments = Vec::new();
    collect_attachments(
        &message.payload,
        "0",
        html.as_deref().unwrap_or_default(),
        &mut attachments,
    );
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
        metadata_json: serde_json::to_string(message).map_err(|e| e.to_string())?,
        unsubscribe,
        attachments,
    })
}

fn collect_attachments(
    part: &MimePart,
    path: &str,
    body_html: &str,
    attachments: &mut Vec<crate::models::MessageAttachment>,
) {
    if !part.filename.is_empty() {
        let content_id = header(part, "Content-ID")
            .map(str::trim)
            .map(|value| value.trim_start_matches('<').trim_end_matches('>'))
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let inline = content_id.as_deref().is_some_and(|content_id| {
            body_html
                .to_ascii_lowercase()
                .contains(&format!("cid:{}", content_id.to_ascii_lowercase()))
        });
        attachments.push(crate::models::MessageAttachment {
            id: part
                .body
                .attachment_id
                .clone()
                .unwrap_or_else(|| format!("part:{path}")),
            filename: part.filename.clone(),
            mime_type: if part.mime_type.is_empty() {
                "application/octet-stream".into()
            } else {
                part.mime_type.clone()
            },
            size: part.body.size,
            content_id,
            inline,
        });
    }
    for (index, child) in part.parts.iter().enumerate() {
        collect_attachments(child, &format!("{path}.{index}"), body_html, attachments);
    }
}

pub fn attachment_bytes_from_payload(
    message: &GmailMessage,
    attachment_id: &str,
) -> Result<Option<Vec<u8>>, String> {
    fn find<'a>(part: &'a MimePart, path: &str, id: &str) -> Option<&'a MimePart> {
        let part_id = part
            .body
            .attachment_id
            .clone()
            .unwrap_or_else(|| format!("part:{path}"));
        if !part.filename.is_empty() && part_id == id {
            return Some(part);
        }
        part.parts
            .iter()
            .enumerate()
            .find_map(|(index, child)| find(child, &format!("{path}.{index}"), id))
    }

    let Some(part) = find(&message.payload, "0", attachment_id) else {
        return Err("Attachment not found".into());
    };
    part.body
        .data
        .as_deref()
        .map(|data| {
            URL_SAFE_NO_PAD
                .decode(data.trim_end_matches('='))
                .map_err(|error| format!("Invalid Gmail base64url attachment: {error}"))
        })
        .transpose()
}

/// Resolves either a Gmail attachment ID or the synthetic MIME-part reference
/// used by older cached messages to the current provider attachment ID.
pub fn provider_attachment_id_from_payload(
    message: &GmailMessage,
    attachment_reference: &str,
) -> Result<Option<String>, String> {
    fn find(part: &MimePart, path: &str, reference: &str) -> Option<Option<String>> {
        if !part.filename.is_empty()
            && (part.body.attachment_id.as_deref() == Some(reference)
                || format!("part:{path}") == reference)
        {
            return Some(part.body.attachment_id.clone());
        }
        part.parts
            .iter()
            .enumerate()
            .find_map(|(index, child)| find(child, &format!("{path}.{index}"), reference))
    }

    find(&message.payload, "0", attachment_reference)
        .ok_or_else(|| "Attachment not found".to_string())
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

fn unsubscribe_metadata(part: &MimePart) -> Option<UnsubscribeMetadata> {
    let urls = parse_list_urls(header(part, "List-Unsubscribe")?);
    if urls.is_empty() {
        return None;
    }
    let one_click_enabled = header(part, "List-Unsubscribe-Post").is_some_and(|value| {
        value
            .trim()
            .eq_ignore_ascii_case("List-Unsubscribe=One-Click")
    }) && header(part, "Authentication-Results")
        .is_some_and(|value| value.to_ascii_lowercase().contains("dkim=pass"));
    let one_click_url = urls
        .iter()
        .find(|url| one_click_enabled && is_https(url))
        .map(Url::to_string);
    let mailto_url = urls
        .iter()
        .find(|url| url.scheme().eq_ignore_ascii_case("mailto"))
        .map(Url::to_string);
    let web_url = urls
        .iter()
        .find(|url| is_https(url) && Some(url.as_str()) != one_click_url.as_deref())
        .map(Url::to_string);
    if one_click_url.is_none() && mailto_url.is_none() && web_url.is_none() {
        return None;
    }
    Some(UnsubscribeMetadata {
        one_click_url,
        mailto_url,
        web_url,
        list_id: header(part, "List-ID")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    })
}

fn parse_list_urls(value: &str) -> Vec<Url> {
    let mut urls = Vec::new();
    let mut rest = value;
    loop {
        let Some(start) = rest.find('<') else { break };
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let candidate: String = after_start[..end]
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        let Ok(url) = Url::parse(&candidate) else {
            break;
        };
        if !matches!(
            url.scheme().to_ascii_lowercase().as_str(),
            "https" | "mailto"
        ) {
            break;
        }
        if url.username().is_empty() && url.password().is_none() {
            urls.push(url);
        }
        rest = &after_start[end + 1..];
    }
    urls
}

fn is_https(url: &Url) -> bool {
    url.scheme().eq_ignore_ascii_case("https")
        && url.host_str().is_some()
        && url.port().is_none_or(|port| port == 443)
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
                ..Default::default()
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
        assert_eq!(normalized.attachments.len(), 1);
        assert_eq!(normalized.attachments[0].filename, "notes.txt");
        assert_eq!(normalized.attachments[0].id, "part:0.2");
        assert_eq!(
            attachment_bytes_from_payload(&message, "part:0.2").unwrap(),
            Some(b"attachment".to_vec())
        );
    }

    #[test]
    fn preserves_gmail_attachment_ids_and_resolves_legacy_part_references() {
        let message: GmailMessage = serde_json::from_value(serde_json::json!({
            "id": "m",
            "threadId": "t",
            "payload": {
                "mimeType": "multipart/mixed",
                "parts": [{
                    "mimeType": "application/pdf",
                    "filename": "invoice.pdf",
                    "body": {
                        "attachmentId": "gmail-token",
                        "size": 42
                    }
                }]
            }
        }))
        .unwrap();

        let normalized = normalize(&message).unwrap();
        assert_eq!(normalized.attachments[0].id, "gmail-token");
        assert_eq!(
            provider_attachment_id_from_payload(&message, "part:0.0").unwrap(),
            Some("gmail-token".into())
        );
    }

    #[test]
    fn marks_html_referenced_content_id_images_as_inline() {
        let message: GmailMessage = serde_json::from_value(serde_json::json!({
            "id": "m",
            "threadId": "t",
            "payload": {
                "mimeType": "multipart/related",
                "parts": [{
                    "mimeType": "text/html",
                    "body": { "data": URL_SAFE_NO_PAD.encode("<p>Regards</p><img src=\"cid:Signature.Logo\">") }
                }, {
                    "mimeType": "image/png",
                    "filename": "image.png",
                    "headers": [{ "name": "Content-ID", "value": "<signature.logo>" }],
                    "body": { "attachmentId": "gmail-inline-token", "size": 42 }
                }, {
                    "mimeType": "application/pdf",
                    "filename": "invoice.pdf",
                    "body": { "attachmentId": "gmail-file-token", "size": 99 }
                }]
            }
        }))
        .unwrap();

        let normalized = normalize(&message).unwrap();
        assert!(normalized.attachments[0].inline);
        assert_eq!(
            normalized.attachments[0].content_id.as_deref(),
            Some("signature.logo")
        );
        assert!(!normalized.attachments[1].inline);
        assert_eq!(normalized.attachments[1].content_id, None);
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

    #[test]
    fn extracts_authenticated_one_click_and_safe_fallbacks_in_header_order() {
        let mut message = GmailMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: String::new(),
            payload: part("text/plain", "ok"),
        };
        message.payload.headers = vec![
            MimeHeader { name: "List-Unsubscribe".into(), value: "<https://list.example/one>, <mailto:list@example.com?subject=unsubscribe>, <https://list.example/preferences>".into() },
            MimeHeader { name: "List-Unsubscribe-Post".into(), value: "List-Unsubscribe=One-Click".into() },
            MimeHeader { name: "Authentication-Results".into(), value: "mx.example; dkim=pass header.i=@example".into() },
            MimeHeader { name: "List-ID".into(), value: "news.example".into() },
        ];
        let normalized = normalize(&message).unwrap();
        let metadata = normalized.unsubscribe.unwrap();
        assert_eq!(
            metadata.one_click_url.as_deref(),
            Some("https://list.example/one")
        );
        assert_eq!(
            metadata.mailto_url.as_deref(),
            Some("mailto:list@example.com?subject=unsubscribe")
        );
        assert_eq!(
            metadata.web_url.as_deref(),
            Some("https://list.example/preferences")
        );
        assert_eq!(metadata.list_id.as_deref(), Some("news.example"));
    }

    #[test]
    fn does_not_offer_one_click_without_authenticated_dkim() {
        let mut message = GmailMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: String::new(),
            payload: part("text/plain", "ok"),
        };
        message.payload.headers = vec![
            MimeHeader {
                name: "List-Unsubscribe".into(),
                value: "<https://list.example/one>".into(),
            },
            MimeHeader {
                name: "List-Unsubscribe-Post".into(),
                value: "List-Unsubscribe=One-Click".into(),
            },
        ];
        let metadata = normalize(&message).unwrap().unsubscribe.unwrap();
        assert!(metadata.one_click_url.is_none());
        assert_eq!(
            metadata.web_url.as_deref(),
            Some("https://list.example/one")
        );
    }
}
