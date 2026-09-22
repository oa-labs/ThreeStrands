use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mail_parser::decoders::html::html_to_text;
use mail_parser::MessageParser;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::limits::MAX_ATTACHMENT_BYTES;

pub(crate) const MAX_THREAD_MESSAGES: usize = 100;
pub(crate) const MAX_MIME_DEPTH: usize = 32;
pub(crate) const MAX_MIME_PARTS: usize = 1_000;
pub(crate) const MAX_MESSAGE_HEADERS: usize = 2_000;
pub(crate) const MAX_MESSAGE_HEADER_BYTES: usize = 256 * 1024;
pub(crate) const MAX_DECODED_BODY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_NORMALIZED_THREAD_BYTES: usize = 16 * 1024 * 1024;

/// A raw message as the provider handed it over: the MIME tree plus the
/// provider-assigned ids and labels that came with it.
///
/// The shape is Gmail's REST payload because that is where it came from, but
/// it is the ingest envelope for every provider — a MIME-native provider
/// parses its bytes into this tree rather than bypassing it. Keeping one
/// envelope keeps `normalize`, attachment extraction, inline-CID handling,
/// unsubscribe parsing, and the email rendering trust boundary shared instead
/// of reimplemented per provider.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMessage {
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

pub fn normalize(message: &RawMessage) -> Result<NormalizedMessage, String> {
    validate_mime_structure(&message.payload)?;
    let mut html = None;
    let mut text = None;
    let mut decoded_body_bytes = 0;
    select_bodies(
        &message.payload,
        &mut html,
        &mut text,
        &mut decoded_body_bytes,
    )?;
    let unsubscribe = unsubscribe_metadata(&message.payload);
    let mut attachments = Vec::new();
    collect_attachments(
        &message.payload,
        "0",
        html.as_deref().unwrap_or_default(),
        &mut attachments,
    );
    let mut body_html = html.unwrap_or_default();
    // An explicitly inline MIME image is itself sender-authored structure. If
    // it has no placement in the HTML, display it after the authored body so
    // classifying it as inline never makes it disappear entirely.
    for attachment in attachments.iter().filter(|attachment| attachment.inline) {
        let Some(content_id) = attachment.content_id.as_deref() else {
            continue;
        };
        if body_references_content_id(&body_html, content_id) {
            continue;
        }
        body_html.push_str(&format!(
            "<img src=\"cid:{}\" alt=\"{}\">",
            escape_html_attribute(content_id),
            escape_html_attribute(&attachment.filename),
        ));
    }
    // Some messages (newsletters, marketing mail) omit the text/plain alternative
    // entirely, so fall back to deriving plain text from the HTML body.
    let body_text = text.unwrap_or_else(|| html_to_text(&body_html));
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
        date: millis_to_rfc3339(&message.internal_date)
            .or_else(|| header(&message.payload, "Date").and_then(normalize_date))
            .unwrap_or_default(),
        body_html,
        body_text,
        snippet: message.snippet.clone(),
        labels: message.label_ids.clone(),
        metadata_json: serde_json::to_string(message).map_err(|e| e.to_string())?,
        unsubscribe,
        attachments,
    })
}

pub(crate) fn normalized_size(message: &NormalizedMessage) -> Option<usize> {
    let fixed_lengths = [
        message.id.len(),
        message.thread_id.len(),
        message.subject.len(),
        message.from.len(),
        message.date.len(),
        message.body_html.len(),
        message.body_text.len(),
        message.snippet.len(),
        message.metadata_json.len(),
    ];
    fixed_lengths
        .into_iter()
        .chain(message.to.iter().map(String::len))
        .chain(message.labels.iter().map(String::len))
        .chain(message.attachments.iter().flat_map(|attachment| {
            [
                attachment.id.len(),
                attachment.filename.len(),
                attachment.mime_type.len(),
                attachment.content_id.as_ref().map_or(0, String::len),
            ]
        }))
        .try_fold(0_usize, usize::checked_add)
}

fn validate_mime_structure(root: &MimePart) -> Result<(), String> {
    let mut stack = vec![(root, 1_usize)];
    let mut part_count = 0_usize;
    let mut header_count = 0_usize;
    let mut header_bytes = 0_usize;
    while let Some((part, depth)) = stack.pop() {
        if depth > MAX_MIME_DEPTH {
            return Err(format!(
                "MIME depth exceeds the {MAX_MIME_DEPTH} part limit"
            ));
        }
        part_count = part_count
            .checked_add(1)
            .ok_or_else(|| "MIME part count overflow".to_string())?;
        if part_count > MAX_MIME_PARTS {
            return Err(format!(
                "MIME message exceeds the {MAX_MIME_PARTS} part limit"
            ));
        }
        header_count = header_count
            .checked_add(part.headers.len())
            .ok_or_else(|| "MIME header count overflow".to_string())?;
        if header_count > MAX_MESSAGE_HEADERS {
            return Err(format!(
                "MIME message exceeds the {MAX_MESSAGE_HEADERS} header limit"
            ));
        }
        for header in &part.headers {
            header_bytes = header_bytes
                .checked_add(header.name.len())
                .and_then(|size| size.checked_add(header.value.len()))
                .ok_or_else(|| "MIME header size overflow".to_string())?;
        }
        if header_bytes > MAX_MESSAGE_HEADER_BYTES {
            return Err(format!(
                "MIME headers exceed the {} KB limit",
                MAX_MESSAGE_HEADER_BYTES / 1024
            ));
        }
        stack.extend(part.parts.iter().map(|child| (child, depth + 1)));
    }
    Ok(())
}

fn collect_attachments(
    part: &MimePart,
    path: &str,
    body_html: &str,
    attachments: &mut Vec<crate::models::MessageAttachment>,
) {
    let content_id = header(part, "Content-ID")
        .map(str::trim)
        .map(|value| value.trim_start_matches('<').trim_end_matches('>'))
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let referenced_by_html = content_id
        .as_deref()
        .is_some_and(|content_id| body_references_content_id(body_html, content_id));
    // Gmail preserves the sender's MIME disposition in the part headers. An
    // inline image can use an encoded cid: URL (or HTML that our deliberately
    // small matcher does not reinterpret), so the disposition is independent
    // semantic evidence that it belongs in the body instead of the download
    // tray. Requiring both an image media type and Content-ID keeps ordinary
    // files, including explicitly attached images, downloadable.
    let declared_inline_image =
        content_id.is_some()
            && part.mime_type.split(';').next().is_some_and(|mime_type| {
                mime_type.trim().to_ascii_lowercase().starts_with("image/")
            })
            && header(part, "Content-Disposition").is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|disposition| disposition.trim().eq_ignore_ascii_case("inline"))
            });
    let inline = referenced_by_html || declared_inline_image;
    if !part.filename.is_empty() || inline {
        attachments.push(crate::models::MessageAttachment {
            id: part
                .body
                .attachment_id
                .clone()
                .unwrap_or_else(|| format!("part:{path}")),
            filename: if part.filename.is_empty() {
                "inline-image".into()
            } else {
                crate::attachment_security::normalize_filename(&part.filename)
            },
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

fn body_references_content_id(body_html: &str, content_id: &str) -> bool {
    let lowercase_html = body_html.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(relative_start) = lowercase_html[offset..].find("cid:") {
        let value_start = offset + relative_start + 4;
        let rest = body_html[value_start..]
            .strip_prefix('<')
            .unwrap_or(&body_html[value_start..]);
        let value_end = rest
            .find(|character: char| {
                character.is_ascii_whitespace() || matches!(character, '\'' | '"' | '<' | '>' | ')')
            })
            .unwrap_or(rest.len());
        if percent_decode(&rest[..value_end])
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .eq_ignore_ascii_case(content_id)
        {
            return true;
        }
        offset = value_start;
    }
    false
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn decoded_base64_len(encoded_len: usize) -> Option<usize> {
    let trailing_bytes = match encoded_len % 4 {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    (encoded_len / 4)
        .checked_mul(3)
        .and_then(|length| length.checked_add(trailing_bytes))
}

pub(crate) fn decode_attachment_data(data: &str) -> Result<Vec<u8>, String> {
    let encoded = data.trim_end_matches('=');
    let decoded_len = decoded_base64_len(encoded.len())
        .ok_or_else(|| "Gmail returned invalid attachment data".to_string())?;
    if decoded_len > MAX_ATTACHMENT_BYTES {
        return Err("Gmail attachment exceeds the 18 MB local limit".into());
    }

    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("Invalid Gmail base64url attachment: {error}"))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err("Gmail attachment exceeds the 18 MB local limit".into());
    }
    Ok(bytes)
}

pub fn attachment_bytes_from_payload(
    message: &RawMessage,
    attachment_id: &str,
) -> Result<Option<Vec<u8>>, String> {
    fn find<'a>(part: &'a MimePart, path: &str, id: &str) -> Option<&'a MimePart> {
        let part_id = part
            .body
            .attachment_id
            .clone()
            .unwrap_or_else(|| format!("part:{path}"));
        if (!part.filename.is_empty() || header(part, "Content-ID").is_some()) && part_id == id {
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
        .map(decode_attachment_data)
        .transpose()
}

/// Resolves either a Gmail attachment ID or the synthetic MIME-part reference
/// used by older cached messages to the current provider attachment ID.
pub fn provider_attachment_id_from_payload(
    message: &RawMessage,
    attachment_reference: &str,
) -> Result<Option<String>, String> {
    fn find(part: &MimePart, path: &str, reference: &str) -> Option<Option<String>> {
        if (!part.filename.is_empty() || header(part, "Content-ID").is_some())
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
    decoded_body_bytes: &mut usize,
) -> Result<(), String> {
    // A named part is an attachment even when its content type is text.
    if part.filename.is_empty() {
        if let Some(data) = part.body.data.as_deref() {
            let decoded = decode_body(data, decoded_body_bytes)?;
            match part.mime_type.as_str() {
                "text/html" if html.is_none() => *html = Some(decoded),
                "text/plain" if text.is_none() => *text = Some(decoded),
                _ => {}
            }
        }
    }
    for child in &part.parts {
        select_bodies(child, html, text, decoded_body_bytes)?;
    }
    Ok(())
}

fn decode_body(value: &str, decoded_body_bytes: &mut usize) -> Result<String, String> {
    let encoded = value.trim_end_matches('=');
    let decoded_len = decoded_base64_len(encoded.len())
        .ok_or_else(|| "Gmail returned invalid body data".to_string())?;
    let next_size = decoded_body_bytes
        .checked_add(decoded_len)
        .ok_or_else(|| "Decoded MIME body size overflow".to_string())?;
    if next_size > MAX_DECODED_BODY_BYTES {
        return Err(format!(
            "Decoded MIME bodies exceed the {} MB limit",
            MAX_DECODED_BODY_BYTES / 1024 / 1024
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("Invalid Gmail base64url body: {error}"))?;
    *decoded_body_bytes = decoded_body_bytes
        .checked_add(bytes.len())
        .ok_or_else(|| "Decoded MIME body size overflow".to_string())?;
    if *decoded_body_bytes > MAX_DECODED_BODY_BYTES {
        return Err(format!(
            "Decoded MIME bodies exceed the {} MB limit",
            MAX_DECODED_BODY_BYTES / 1024 / 1024
        ));
    }
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
    let raw = format!("To: {value}\r\n\r\n");
    let Some(message) = MessageParser::default().parse(raw.as_bytes()) else {
        return Vec::new();
    };
    let Some(list) = message.to() else {
        return Vec::new();
    };

    list.iter()
        .filter_map(|address| {
            let email = address.address()?.trim();
            if email.is_empty() {
                return None;
            }
            let name = address.name().unwrap_or_default().trim();
            Some(if name.is_empty() {
                email.to_owned()
            } else {
                format!("{name} <{email}>")
            })
        })
        .collect()
}

fn millis_to_rfc3339(value: &str) -> Option<String> {
    value
        .parse::<i64>()
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|date| date.to_rfc3339())
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

    #[test]
    fn attachment_decoded_size_policy_includes_the_limit() {
        let exact_encoded_len = MAX_ATTACHMENT_BYTES / 3 * 4;
        assert_eq!(
            decoded_base64_len(exact_encoded_len),
            Some(MAX_ATTACHMENT_BYTES)
        );
        assert_eq!(
            decoded_base64_len(exact_encoded_len + 2),
            Some(MAX_ATTACHMENT_BYTES + 1)
        );
        assert_eq!(decoded_base64_len(exact_encoded_len + 1), None);
    }

    #[test]
    fn decodes_padded_and_unpadded_attachment_data() {
        assert_eq!(decode_attachment_data("SGVsbG8").unwrap(), b"Hello");
        assert_eq!(decode_attachment_data("SGVsbG8=").unwrap(), b"Hello");
    }

    #[test]
    fn decoded_body_limit_accepts_exactly_the_limit_and_rejects_above_it() {
        let exact = URL_SAFE_NO_PAD.encode(vec![b'a'; MAX_DECODED_BODY_BYTES]);
        let mut decoded = 0;
        assert_eq!(
            decode_body(&exact, &mut decoded).unwrap().len(),
            MAX_DECODED_BODY_BYTES
        );

        let above = URL_SAFE_NO_PAD.encode(vec![b'a'; MAX_DECODED_BODY_BYTES + 1]);
        assert!(decode_body(&above, &mut 0).is_err());
    }

    #[test]
    fn mime_structure_limits_include_the_boundary() {
        let mut exact_depth = MimePart::default();
        for _ in 1..MAX_MIME_DEPTH {
            exact_depth = MimePart {
                parts: vec![exact_depth],
                ..Default::default()
            };
        }
        assert!(validate_mime_structure(&exact_depth).is_ok());
        let above_depth = MimePart {
            parts: vec![exact_depth],
            ..Default::default()
        };
        assert!(validate_mime_structure(&above_depth)
            .unwrap_err()
            .contains("depth"));

        let exact_parts = MimePart {
            parts: vec![MimePart::default(); MAX_MIME_PARTS - 1],
            ..Default::default()
        };
        assert!(validate_mime_structure(&exact_parts).is_ok());
        let above_parts = MimePart {
            parts: vec![MimePart::default(); MAX_MIME_PARTS],
            ..Default::default()
        };
        assert!(validate_mime_structure(&above_parts)
            .unwrap_err()
            .contains("part limit"));
    }

    #[test]
    fn header_limits_include_the_boundary() {
        let header = MimeHeader {
            name: String::new(),
            value: String::new(),
        };
        let exact_count = MimePart {
            headers: vec![header.clone(); MAX_MESSAGE_HEADERS],
            ..Default::default()
        };
        assert!(validate_mime_structure(&exact_count).is_ok());
        let above_count = MimePart {
            headers: vec![header; MAX_MESSAGE_HEADERS + 1],
            ..Default::default()
        };
        assert!(validate_mime_structure(&above_count)
            .unwrap_err()
            .contains("header limit"));

        let exact_bytes = MimePart {
            headers: vec![MimeHeader {
                name: String::new(),
                value: "x".repeat(MAX_MESSAGE_HEADER_BYTES),
            }],
            ..Default::default()
        };
        assert!(validate_mime_structure(&exact_bytes).is_ok());
        let above_bytes = MimePart {
            headers: vec![MimeHeader {
                name: String::new(),
                value: "x".repeat(MAX_MESSAGE_HEADER_BYTES + 1),
            }],
            ..Default::default()
        };
        assert!(validate_mime_structure(&above_bytes)
            .unwrap_err()
            .contains("headers"));
    }

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
        let message = RawMessage {
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
    fn derives_body_text_from_html_when_no_plain_part_exists() {
        let message = RawMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: "0".into(),
            payload: part("text/html", "<p>Joel, this is the math.</p>"),
        };
        let normalized = normalize(&message).unwrap();
        assert_eq!(normalized.body_text.trim(), "Joel, this is the math.");
    }

    #[test]
    fn keeps_quoted_display_name_commas_inside_one_address() {
        assert_eq!(
            split_addresses("\"Bates, Daniel R\" <daniel@example.com>, bethgold@gmail.com"),
            vec![
                "Bates, Daniel R <daniel@example.com>",
                "bethgold@gmail.com",
            ]
        );
    }

    #[test]
    fn preserves_gmail_attachment_ids_and_resolves_legacy_part_references() {
        let message: RawMessage = serde_json::from_value(serde_json::json!({
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
    fn marks_encoded_html_content_id_images_as_inline() {
        let message: RawMessage = serde_json::from_value(serde_json::json!({
            "id": "m",
            "threadId": "t",
            "payload": {
                "mimeType": "multipart/related",
                "parts": [{
                    "mimeType": "text/html",
                    "body": { "data": URL_SAFE_NO_PAD.encode("<p>Regards</p><img src=\"cid:Signature%2ELogo\">") }
                }, {
                    "mimeType": "image/png",
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
        assert_eq!(normalized.body_html.matches("cid:").count(), 1);
        assert!(!normalized.attachments[1].inline);
        assert_eq!(normalized.attachments[1].content_id, None);
    }

    #[test]
    fn displays_a_declared_inline_image_without_an_html_placement() {
        let message: RawMessage = serde_json::from_value(serde_json::json!({
            "id": "m",
            "threadId": "t",
            "payload": {
                "mimeType": "multipart/mixed",
                "parts": [{
                    "mimeType": "multipart/alternative",
                    "parts": [{
                        "mimeType": "text/plain",
                        "body": { "data": URL_SAFE_NO_PAD.encode("See chart") }
                    }, {
                        "mimeType": "text/html",
                        "body": { "data": URL_SAFE_NO_PAD.encode("<p>See chart</p>") }
                    }]
                }, {
                    "mimeType": "image/png; name=\"chart.png\"",
                    "filename": "chart.png",
                    "headers": [
                        { "name": "Content-ID", "value": "<chart.one>" },
                        { "name": "Content-Disposition", "value": " Inline ; filename=\"chart.png\"" }
                    ],
                    "body": { "attachmentId": "inline-chart", "size": 42 }
                }]
            }
        }))
        .unwrap();

        let normalized = normalize(&message).unwrap();
        assert_eq!(normalized.attachments.len(), 1);
        assert!(normalized.attachments[0].inline);
        assert_eq!(normalized.attachments[0].id, "inline-chart");
        assert_eq!(
            normalized.body_html,
            "<p>See chart</p><img src=\"cid:chart.one\" alt=\"chart.png\">"
        );
    }

    #[test]
    fn keeps_explicit_image_attachments_downloadable_even_with_a_content_id() {
        let message: RawMessage = serde_json::from_value(serde_json::json!({
            "id": "m",
            "threadId": "t",
            "payload": {
                "mimeType": "multipart/mixed",
                "parts": [{
                    "mimeType": "text/html",
                    "body": { "data": URL_SAFE_NO_PAD.encode("<p>Photos attached.</p>") }
                }, {
                    "mimeType": "image/jpeg",
                    "filename": "photo.jpg",
                    "headers": [
                        { "name": "Content-ID", "value": "<photo.attachment>" },
                        { "name": "Content-Disposition", "value": "attachment; filename=\"photo.jpg\"" }
                    ],
                    "body": { "attachmentId": "photo-file", "size": 99 }
                }]
            }
        }))
        .unwrap();

        let normalized = normalize(&message).unwrap();
        assert_eq!(normalized.attachments.len(), 1);
        assert!(!normalized.attachments[0].inline);
        assert_eq!(
            normalized.attachments[0].content_id.as_deref(),
            Some("photo.attachment")
        );
    }

    #[test]
    fn rejects_invalid_base64_at_provider_boundary() {
        let mut message = RawMessage {
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
        let mut message = RawMessage {
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
        let mut message = RawMessage {
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
