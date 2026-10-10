//! Raw RFC 5322 bytes -> [`crate::mime::RawMessage`], via `mail-parser`.
//!
//! `docs/imap-design.md` ("Recommended stack" / MIME) says raw `BODY.PEEK[]`
//! bytes are parsed into the shared `RawMessage` envelope and then flow through
//! the exact same `mime::normalize` pipeline Gmail uses — same sanitizer, same
//! remote-image proxy, same renderer trust boundary (guiding rule 5). This
//! module is the ONLY IMAP-side conversion; it faithfully reproduces the MIME
//! tree and performs no network I/O of any kind.
//!
//! Faithful structure (`AGENTS.md` "Email rendering"): the full multipart tree
//! is preserved, MIME/security headers are carried through verbatim, and no part
//! is dropped for being empty, whitespace-only or "redundant". Sanitisation is
//! the renderer's job downstream, not this layer's.
//!
//! ## The encoding contract with `mime.rs`
//!
//! `mime::normalize` reads a leaf part's content from `MimeBody.data` by
//! base64url-decoding it (`decode_body` / `decode_attachment_data` use
//! `URL_SAFE_NO_PAD`), exactly as Gmail's REST payload delivers it. So this
//! converter base64url-encodes transfer-decoded attachment bytes into `data`,
//! while display text is also converted to UTF-8. Encapsulated emails keep
//! both their original transfer-decoded bytes and their parsed subtree.
//!
//! ## Hostile input
//!
//! All input is sender-controlled. The parse never panics (no `unwrap`/
//! `expect`/indexing on parsed data), and the [`policy`](super::policy) limits
//! on depth and part count are enforced BEFORE the tree is built, so a mail
//! bomb is a typed [`ProviderError`], not an allocation storm. A message
//! `mail-parser` cannot parse at all is a typed error, not a panic.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mail_parser::{Address, Encoding, HeaderValue, Message, MessageParser, MimeHeaders, PartType};

use crate::mime::{MimeBody, MimeHeader, MimePart, RawMessage};
use crate::provider::imap::policy;
use crate::provider::ProviderError;

/// Convert raw RFC 5322 bytes into a [`RawMessage`] for the shared pipeline.
///
/// `id` is the stable message id already derived by [`super::identity`];
/// `thread_id` is set to the message id for now (Slice 5 owns threading, see
/// below); `label_ids` is supplied by the caller (Slice 5 builds labels from
/// location + flags — this slice just accepts and carries them).
///
/// Returns a typed [`ProviderError`] when the bytes exceed a
/// [`policy`](super::policy) limit or cannot be parsed as a message.
pub fn to_raw_message(
    raw: &[u8],
    id: &str,
    label_ids: Vec<String>,
) -> Result<RawMessage, ProviderError> {
    policy::check_raw_message_bytes(raw.len())?;
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| ProviderError::InvalidOperation("could not parse message bytes".into()))?;

    // Enforce the shape limits against the parsed tree before building ours.
    // mail-parser's flat `parts` vector already bounds total parts; we count
    // the reachable tree and its depth so a declared-but-unreachable blow-up
    // is still refused on honest numbers.
    let (count, depth) = tree_shape(&message);
    policy::check_mime_part_count(count)?;
    policy::check_mime_depth(depth)?;

    let payload = build_part(&message, message_root(&message))?;

    // thread_id = the message id for now. Slice 5 owns threading
    // (`docs/imap-design.md` "Threading": thread ids are derived from message
    // references); until then a message threads to itself, which is correct
    // for a singleton and is the stable value Slice 5 will re-point.
    Ok(RawMessage {
        id: id.to_string(),
        thread_id: id.to_string(),
        label_ids,
        snippet: String::new(),
        internal_date: String::new(),
        payload,
    })
}

/// The root part index. mail-parser stores the tree flat in `message.parts`
/// with part 0 as the root (`Message::root_part`); we index by id so nested
/// `message/rfc822` sub-messages (their own `Message`) recurse cleanly.
fn message_root(_message: &Message<'_>) -> usize {
    0
}

/// Walk the parsed tree to its reachable part count and maximum depth, counting
/// a nested `message/rfc822` as one extra level. Iterative, so a pathological
/// depth cannot overflow the stack during measurement.
fn tree_shape(message: &Message<'_>) -> (usize, usize) {
    // (message, part index, depth)
    let mut stack: Vec<(&Message<'_>, usize, usize)> = vec![(message, 0, 1)];
    let mut count = 0usize;
    let mut max_depth = 1usize;
    while let Some((msg, idx, depth)) = stack.pop() {
        let Some(part) = msg.parts.get(idx) else {
            continue;
        };
        count = count.saturating_add(1);
        max_depth = max_depth.max(depth);
        match &part.body {
            PartType::Multipart(children) => {
                for &child in children {
                    stack.push((msg, child as usize, depth.saturating_add(1)));
                }
            }
            PartType::Message(sub) => {
                // The nested message's own root, one level deeper.
                stack.push((sub, 0, depth.saturating_add(1)));
            }
            _ => {}
        }
    }
    (count, max_depth)
}

/// Build one `MimePart` (and its subtree) from the part at `idx` within
/// `message`. Offsets refer to that Message's backing buffer: unencoded nested
/// messages share the outer buffer, while transfer-encoded messages own a
/// decoded buffer. Recursion is bounded by the shape check in `to_raw_message`.
fn build_part(message: &Message<'_>, idx: usize) -> Result<MimePart, ProviderError> {
    let Some(part) = message.parts.get(idx) else {
        // An id that points nowhere is a malformed tree, not a panic.
        return Ok(MimePart::default());
    };

    let headers = reconstruct_headers(message.raw_message.as_ref(), part);
    let mime_type = content_type_string(part);
    let filename = part.attachment_name().unwrap_or_default().to_string();

    match &part.body {
        PartType::Multipart(children) => {
            let mut parts = Vec::with_capacity(children.len());
            for &child in children {
                parts.push(build_part(message, child as usize)?);
            }
            Ok(MimePart {
                mime_type,
                filename,
                headers,
                body: MimeBody::default(),
                parts,
            })
        }
        PartType::Message(sub) => {
            // A nested message/rfc822: its single child is the sub-message's
            // own root. Retain the original email bytes as well as its tree so
            // a named .eml can be downloaded through the shared attachment API.
            let child = build_part(sub, 0)?;
            let bytes = transfer_decoded_bytes(message, part)?;
            let mut result = leaf(mime_type, filename, headers, &bytes);
            result.parts.push(child);
            Ok(result)
        }
        PartType::Text(text) | PartType::Html(text) => {
            // Display bodies need UTF-8; downloads need the original charset's
            // bytes after transfer decoding, never a Unicode re-encoding.
            if filename.is_empty() && !is_attachment(part) {
                Ok(leaf(mime_type, filename, headers, text.as_bytes()))
            } else {
                let bytes = transfer_decoded_bytes(message, part)?;
                Ok(leaf(mime_type, filename, headers, &bytes))
            }
        }
        PartType::Binary(bytes) | PartType::InlineBinary(bytes) => {
            Ok(leaf(mime_type, filename, headers, bytes.as_ref()))
        }
    }
}

fn is_attachment(part: &mail_parser::MessagePart<'_>) -> bool {
    part.content_disposition()
        .is_some_and(|value| value.c_type.eq_ignore_ascii_case("attachment"))
}

/// Decode transfer encoding only; charset conversion would corrupt a download.
fn transfer_decoded_bytes(
    message: &Message<'_>,
    part: &mail_parser::MessagePart<'_>,
) -> Result<Vec<u8>, ProviderError> {
    let bytes = message
        .raw_message
        .get(part.offset_body as usize..part.offset_end as usize)
        .ok_or_else(|| ProviderError::InvalidOperation("invalid MIME body offsets".into()))?;
    let decoded = match part.encoding {
        Encoding::None => Some(bytes.to_vec()),
        Encoding::Base64 => mail_parser::decoders::base64::base64_decode(bytes),
        Encoding::QuotedPrintable => {
            mail_parser::decoders::quoted_printable::quoted_printable_decode(bytes)
        }
    };
    decoded.ok_or_else(|| ProviderError::InvalidOperation("invalid MIME transfer encoding".into()))
}

/// Assemble a leaf `MimePart`, base64url-encoding its decoded content into
/// `data` per the encoding contract above.
fn leaf(mime_type: String, filename: String, headers: Vec<MimeHeader>, decoded: &[u8]) -> MimePart {
    MimePart {
        mime_type,
        filename,
        headers,
        body: MimeBody {
            data: Some(URL_SAFE_NO_PAD.encode(decoded)),
            attachment_id: None,
            size: decoded.len() as u64,
        },
        parts: Vec::new(),
    }
}

/// `type/subtype` for a part, lowercased, defaulting to `text/plain` when the
/// part declares no Content-Type (RFC 2045 §5.2). Parameters (charset, name,
/// boundary) are intentionally dropped from this field — the shared pipeline
/// reads `mime_type` as a bare media type and reads parameters it needs (the
/// filename) from the headers we carry through verbatim.
fn content_type_string(part: &mail_parser::MessagePart<'_>) -> String {
    match part.content_type() {
        Some(ct) => {
            let main = ct.c_type.to_ascii_lowercase();
            match ct.c_subtype.as_ref() {
                Some(sub) => format!("{main}/{}", sub.to_ascii_lowercase()),
                None => main,
            }
        }
        None => "text/plain".to_string(),
    }
}

/// Reconstruct a part's headers from the raw bytes between its header offset
/// and its body offset, retaining every header in order and unfolding
/// continuation lines. Only encoded display fields are then re-serialized;
/// MIME and security fields retain their original values.
///
/// Operating on the raw slice keeps this faithful and provider-neutral: the
/// shared pipeline reads `Subject`/`From`/`To`/`Date`/`Content-ID`/
/// `Content-Disposition` by name from exactly these headers.
fn reconstruct_headers(raw: &[u8], part: &mail_parser::MessagePart<'_>) -> Vec<MimeHeader> {
    let start = part.offset_header as usize;
    let end = part.offset_body as usize;
    if start >= end || end > raw.len() {
        return Vec::new();
    }
    let block = &raw[start..end];
    let text = String::from_utf8_lossy(block);
    let mut headers: Vec<MimeHeader> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // Folded continuation of the previous header value.
            if let Some(last) = headers.last_mut() {
                last.value.push(' ');
                last.value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push(MimeHeader {
                name: name.trim().to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    for header in &mut headers {
        decode_display_header(header);
    }
    headers
}

/// Decode only presentation fields. Authentication, unsubscribe and MIME
/// headers retain their raw semantics. Parse address lists before decoding
/// names so encoded punctuation cannot turn one mailbox into several.
fn decode_display_header(header: &mut MimeHeader) {
    if !header.value.contains("=?") {
        return;
    }
    let name = header.name.to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "subject" | "from" | "to" | "cc" | "bcc" | "reply-to" | "sender"
    ) {
        return;
    }
    let raw = format!("{}: {}\r\n\r\n", header.name, header.value);
    let Some(message) = MessageParser::default().parse(raw.as_bytes()) else {
        return;
    };
    let Some(value) = message.header(header.name.as_str()) else {
        return;
    };
    match value {
        HeaderValue::Text(text) if name == "subject" => header.value = safe_header_text(text),
        HeaderValue::Address(address) => {
            header.value = match address {
                Address::List(list) => list
                    .iter()
                    .map(display_address)
                    .collect::<Vec<_>>()
                    .join(", "),
                Address::Group(groups) => groups
                    .iter()
                    .map(|group| {
                        let members = group
                            .addresses
                            .iter()
                            .map(display_address)
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "{}: {members};",
                            quote_name(group.name.as_deref().unwrap_or_default())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            };
        }
        _ => {}
    }
}

fn safe_header_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn quote_name(name: &str) -> String {
    let name = safe_header_text(name);
    if name.contains([
        '(', ')', '<', '>', '[', ']', ':', ';', '@', '\\', ',', '.', '"',
    ]) {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        name
    }
}

fn display_address(address: &mail_parser::Addr<'_>) -> String {
    let email = safe_header_text(address.address.as_deref().unwrap_or_default());
    match address.name.as_deref() {
        Some(name) if !name.is_empty() => format!("{} <{email}>", quote_name(name)),
        _ => email,
    }
}

/// Re-slice an attachment out of cached raw bytes by its IMAP MIME section
/// path (RFC 3501 §6.4.5, 1-based, dot-separated — e.g. `1`, `1.2`), returning
/// the part's DECODED content bytes.
///
/// `docs/imap-design.md` ("Bodies"): attachments are fetched lazily by MIME
/// section number, which becomes the `attachment_bytes` handle. This resolves
/// that handle against the raw message we cached, so a later attachment read
/// needs no second server fetch. Returns `None` for a missing section or a
/// multipart container. Encapsulated messages return their complete bytes.
/// Malformed paths, decoding failures and policy violations are typed errors.
pub fn attachment_bytes_from_raw(
    raw: &[u8],
    section: &str,
) -> Result<Option<Vec<u8>>, ProviderError> {
    policy::check_raw_message_bytes(raw.len())?;
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| ProviderError::InvalidOperation("could not parse cached message".into()))?;
    let (count, depth) = tree_shape(&message);
    policy::check_mime_part_count(count)?;
    policy::check_mime_depth(depth)?;
    let mut components = Vec::new();
    for token in section.split('.') {
        let n: usize = token.parse().map_err(|_| {
            ProviderError::InvalidOperation(format!("invalid MIME section: {section}"))
        })?;
        if n == 0 {
            return Err(ProviderError::InvalidOperation(format!(
                "MIME section is 1-based: {section}"
            )));
        }
        components.push(n);
    }
    let Some((owner, part)) = resolve_message_section(&message, &components) else {
        return Ok(None);
    };
    if matches!(part.body, PartType::Multipart(_)) {
        return Ok(None);
    }
    transfer_decoded_bytes(owner, part).map(Some)
}

/// Resolve an IMAP section path to the flat part index within `message`,
/// descending into `message/rfc822` sub-messages. The 1-based child numbers
/// map onto the ordered children of each multipart.
fn resolve_message_section<'a>(
    message: &'a Message<'_>,
    components: &[usize],
) -> Option<(&'a Message<'a>, &'a mail_parser::MessagePart<'a>)> {
    let root = message.parts.first()?;
    if matches!(root.body, PartType::Multipart(_)) {
        resolve_part_section(message, 0, components)
    } else {
        // Every non-multipart message has a virtual section 1, including a
        // single attachment or an encapsulated message's single body part.
        let (&first, rest) = components.split_first()?;
        (first == 1).then_some(())?;
        resolve_part_section(message, 0, rest)
    }
}

fn resolve_part_section<'a>(
    message: &'a Message<'_>,
    idx: usize,
    components: &[usize],
) -> Option<(&'a Message<'a>, &'a mail_parser::MessagePart<'a>)> {
    let part = message.parts.get(idx)?;
    let Some((&first, rest)) = components.split_first() else {
        return Some((message, part));
    };
    match &part.body {
        PartType::Multipart(children) => {
            let child = *children.get(first.checked_sub(1)?)?;
            resolve_part_section(message, child as usize, rest)
        }
        PartType::Message(sub) => {
            // The next number addresses the enclosed message's body. A
            // multipart root contributes no extra '.1' to the IMAP path.
            resolve_message_section(sub, components)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::{self, decode_attachment_data};

    /// Decode a leaf part's `data` back to bytes the way the shared pipeline
    /// does, so the encoding-contract assertions read the real content.
    fn decoded(part: &MimePart) -> Vec<u8> {
        decode_attachment_data(part.body.data.as_deref().unwrap_or_default()).unwrap()
    }

    fn raw_of(id: &str, bytes: &[u8]) -> RawMessage {
        to_raw_message(bytes, id, vec!["INBOX".into()]).unwrap()
    }

    #[test]
    fn carries_labels_and_sets_thread_id_to_the_message_id() {
        let msg = raw_of(
            "imap:me@x:abc",
            b"Subject: Hi\r\nFrom: a@b.com\r\n\r\nbody\r\n",
        );
        assert_eq!(msg.id, "imap:me@x:abc");
        assert_eq!(msg.thread_id, "imap:me@x:abc");
        assert_eq!(msg.label_ids, vec!["INBOX".to_string()]);
    }

    #[test]
    fn multipart_alternative_round_trips_through_normalize() {
        let raw = b"Subject: Alt\r\nFrom: a@b.com\r\nTo: c@d.com\r\n\
Content-Type: multipart/alternative; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: text/plain\r\n\r\nplain text\r\n\
--B\r\nContent-Type: text/html\r\n\r\n<p>html body</p>\r\n--B--\r\n";
        let message = raw_of("imap:me@x:alt", raw);
        assert_eq!(message.payload.mime_type, "multipart/alternative");
        assert_eq!(message.payload.parts.len(), 2);
        let normalized = mime::normalize(&message).unwrap();
        assert_eq!(normalized.subject, "Alt");
        assert_eq!(normalized.body_html, "<p>html body</p>");
        assert_eq!(normalized.body_text, "plain text");
    }

    #[test]
    fn multipart_related_with_inline_cid_image_keeps_the_image_and_cid() {
        let png = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let b64 = URL_SAFE_NO_PAD.encode(png);
        // Standard base64 for the wire body (mail-parser decodes it back).
        let wire_b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let raw = format!(
            "Subject: Rel\r\nFrom: a@b.com\r\n\
Content-Type: multipart/related; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/html\r\n\r\n<p>hi</p><img src=\"cid:logo\">\r\n\
--R\r\nContent-Type: image/png\r\nContent-ID: <logo>\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{wire_b64}\r\n--R--\r\n"
        );
        let message = raw_of("imap:me@x:rel", raw.as_bytes());
        assert_eq!(message.payload.mime_type, "multipart/related");
        let image = &message.payload.parts[1];
        assert_eq!(image.mime_type, "image/png");
        // The decoded bytes equal the original PNG, and re-encode to our b64.
        assert_eq!(decoded(image), png);
        assert_eq!(image.body.data.as_deref(), Some(b64.as_str()));
        // normalize treats the referenced CID image as inline.
        let normalized = mime::normalize(&message).unwrap();
        assert!(normalized
            .attachments
            .iter()
            .any(|a| a.inline && a.content_id.as_deref() == Some("logo")));
    }

    #[test]
    fn an_attachment_section_path_re_slices_the_cached_raw_bytes() {
        let raw = b"Subject: Doc\r\nFrom: a@b.com\r\n\
Content-Type: multipart/mixed; boundary=\"M\"\r\n\r\n\
--M\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n\
--M\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"note.txt\"\r\n\r\nATTACHED-BYTES\r\n--M--\r\n";
        // IMAP section 2 is the second child of the mixed container.
        let bytes = attachment_bytes_from_raw(raw, "2").unwrap().unwrap();
        assert_eq!(bytes, b"ATTACHED-BYTES");
        // A section that names a container (the root) yields no leaf bytes.
        assert!(attachment_bytes_from_raw(raw, "9").unwrap().is_none());
        // A malformed / zero section is a typed error, not a panic.
        assert!(attachment_bytes_from_raw(raw, "0").is_err());
        assert!(attachment_bytes_from_raw(raw, "x").is_err());
    }

    #[test]
    fn nested_message_rfc822_is_preserved_as_a_subtree() {
        let raw = b"Subject: Fwd\r\nFrom: a@b.com\r\n\
Content-Type: multipart/mixed; boundary=\"F\"\r\n\r\n\
--F\r\nContent-Type: text/plain\r\n\r\nForwarded below\r\n\
--F\r\nContent-Type: message/rfc822\r\n\r\n\
Subject: Inner\r\nFrom: inner@x.com\r\n\r\ninner body\r\n--F--\r\n";
        let message = raw_of("imap:me@x:fwd", raw);
        let nested = &message.payload.parts[1];
        assert_eq!(nested.mime_type, "message/rfc822");
        assert_eq!(nested.parts.len(), 1);
        // The inner message's own Subject survives on its root part.
        let inner = &nested.parts[0];
        assert!(inner
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("Subject") && h.value == "Inner"));
        // And normalize still produces sane output for the outer message.
        assert_eq!(mime::normalize(&message).unwrap().subject, "Fwd");
    }

    #[test]
    fn a_non_utf8_charset_body_does_not_panic_and_normalizes() {
        // ISO-8859-1 0xE9 = 'é'. mail-parser decodes per the declared charset.
        let raw = b"Subject: Accent\r\nFrom: a@b.com\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\n\r\ncaf\xe9\r\n";
        let message = raw_of("imap:me@x:acc", raw);
        let normalized = mime::normalize(&message).unwrap();
        assert_eq!(normalized.body_text.trim_end(), "café");
    }

    #[test]
    fn text_attachment_downloads_preserve_charset_bytes_for_every_transfer_encoding() {
        for (encoding, wire) in [
            ("8bit", &b"caf\xe9"[..]),
            ("quoted-printable", &b"caf=E9"[..]),
            ("base64", &b"Y2Fm6Q=="[..]),
        ] {
            for media_type in ["text/plain", "text/html"] {
                let mut raw = format!(
                    "Content-Type: multipart/mixed; boundary=T\r\n\r\n\
                     --T\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\ncaf=E9\r\n\
                     --T\r\nContent-Type: {media_type}; charset=iso-8859-1\r\n\
                     Content-Disposition: attachment; filename=note.txt\r\n\
                     Content-Transfer-Encoding: {encoding}\r\n\r\n"
                )
                .into_bytes();
                raw.extend_from_slice(wire);
                raw.extend_from_slice(b"\r\n--T--\r\n");
                let converted = raw_of("attachment", &raw);
                let normalized = mime::normalize(&converted).unwrap();
                assert_eq!(normalized.attachments[0].size, 4);
                assert_eq!(decoded(&converted.payload.parts[1]), b"caf\xe9");
                assert_eq!(
                    mime::attachment_bytes_from_payload(&converted, "part:0.1")
                        .unwrap()
                        .unwrap(),
                    b"caf\xe9"
                );
                assert_eq!(
                    attachment_bytes_from_raw(&raw, "2").unwrap().unwrap(),
                    b"caf\xe9"
                );
            }
        }
    }

    #[test]
    fn attached_emails_retain_download_bytes_and_the_nested_tree() {
        let inner = b"Subject: Inner\r\nFrom: inner@example.com\r\n\r\ninner body";
        for encoded in [false, true] {
            let (media_type, encoding, body) = if encoded {
                (
                    "message/global",
                    "base64",
                    base64::engine::general_purpose::STANDARD.encode(inner),
                )
            } else {
                (
                    "message/rfc822",
                    "8bit",
                    String::from_utf8_lossy(inner).into_owned(),
                )
            };
            let raw = format!(
                "Subject: Outer\r\nContent-Type: multipart/mixed; boundary=E\r\n\r\n\
                 --E\r\nContent-Type: text/plain\r\n\r\nouter\r\n\
                 --E\r\nContent-Type: {media_type}\r\n\
                 Content-Disposition: attachment; filename=forward.eml\r\n\
                 Content-Transfer-Encoding: {encoding}\r\n\r\n{body}\r\n--E--\r\n"
            );
            let converted = raw_of("eml", raw.as_bytes());
            let attachment = &converted.payload.parts[1];
            assert_eq!(attachment.parts.len(), 1);
            assert!(attachment.parts[0]
                .headers
                .iter()
                .any(|h| h.name == "Subject" && h.value == "Inner"));
            assert_eq!(decoded(attachment), inner);
            let normalized = mime::normalize(&converted).unwrap();
            assert_eq!(normalized.subject, "Outer");
            assert_eq!(normalized.attachments[0].filename, "forward.eml");
            assert_eq!(normalized.attachments[0].size, inner.len() as u64);
            assert_eq!(
                mime::attachment_bytes_from_payload(&converted, "part:0.1")
                    .unwrap()
                    .unwrap(),
                inner
            );
            assert_eq!(
                attachment_bytes_from_raw(raw.as_bytes(), "2")
                    .unwrap()
                    .unwrap(),
                inner
            );
        }
    }

    #[test]
    fn imap_sections_follow_singlepart_multipart_and_encapsulated_message_numbering() {
        let single = b"Content-Type: application/pdf\r\n\r\n%PDF-content";
        assert_eq!(
            attachment_bytes_from_raw(single, "1").unwrap().unwrap(),
            b"%PDF-content"
        );
        assert!(attachment_bytes_from_raw(single, "1.1").unwrap().is_none());
        assert!(attachment_bytes_from_raw(single, "2").unwrap().is_none());

        let inner_multipart = "Content-Type: multipart/mixed; boundary=I\r\n\r\n\
            --I\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
            --I\r\nContent-Type: application/octet-stream\r\n\r\nFILE\r\n--I--";
        for (inner, section, expected) in [
            (inner_multipart, "1.2", &b"FILE"[..]),
            (
                "Content-Type: application/pdf\r\n\r\n%PDF-content",
                "1.1",
                &b"%PDF-content"[..],
            ),
        ] {
            let raw = format!(
                "Content-Type: multipart/mixed; boundary=O\r\n\r\n\
                --O\r\nContent-Type: message/rfc822\r\n\r\n{inner}\r\n--O--\r\n"
            );
            assert_eq!(
                attachment_bytes_from_raw(raw.as_bytes(), section)
                    .unwrap()
                    .unwrap(),
                expected
            );
            assert!(attachment_bytes_from_raw(raw.as_bytes(), "1.3")
                .unwrap()
                .is_none());
            assert!(attachment_bytes_from_raw(raw.as_bytes(), "1.1.2")
                .unwrap()
                .is_none());
        }
        // A message/rfc822 at the top level itself occupies virtual part 1.
        let raw = format!("Content-Type: message/rfc822\r\n\r\n{inner_multipart}");
        assert_eq!(
            attachment_bytes_from_raw(raw.as_bytes(), "1.2")
                .unwrap()
                .unwrap(),
            b"FILE"
        );
    }

    #[test]
    fn encoded_nested_messages_use_their_own_header_and_body_offsets() {
        let inner = "Subject: Inner\r\nFrom: inner@example.com\r\nContent-Type: text/html\r\n\r\n<p>INNER</p>";
        for (encoding, wire) in [
            (
                "base64",
                base64::engine::general_purpose::STANDARD.encode(inner),
            ),
            (
                "quoted-printable",
                inner.replace('=', "=3D").replace('<', "=3C"),
            ),
        ] {
            let raw = format!("Subject: Outer\r\nFrom: outer@example.com\r\n\
                Content-Type: message/global\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{wire}");
            let converted = raw_of("encoded", raw.as_bytes());
            let nested = &converted.payload.parts[0];
            for (name, value) in [
                ("Subject", "Inner"),
                ("From", "inner@example.com"),
                ("Content-Type", "text/html"),
            ] {
                assert!(
                    nested
                        .headers
                        .iter()
                        .any(|h| h.name == name && h.value == value),
                    "{encoding}: {name}"
                );
            }
            assert_eq!(decoded(nested), b"<p>INNER</p>");
            assert_eq!(
                attachment_bytes_from_raw(raw.as_bytes(), "1.1")
                    .unwrap()
                    .unwrap(),
                b"<p>INNER</p>"
            );
        }
    }

    #[test]
    fn encoded_display_headers_decode_without_changing_mime_or_security_headers() {
        for subject in ["=?UTF-8?B?Y2Fmw6k=?=", "=?ISO-8859-1?Q?caf=E9?="] {
            let raw = format!(
                "Subject: {subject}\r\n\
                From: =?UTF-8?B?Sm9zw6k=?= <j@example.com>\r\n\
                To: =?UTF-8?Q?Doe=2C_Jane?= <jane@example.com>, Other <other@example.com>\r\n\
                Cc: =?UTF-8?Q?Ren=C3=A9?= <r@example.com>\r\n\
                Reply-To: =?UTF-8?B?Sm9zw6k=?= <reply@example.com>\r\n\
                Authentication-Results: mx.example; dkim=fail header.d=example.com\r\n\
                List-Unsubscribe: <https://example.com/=?UTF-8?Q?literal?=>\r\n\
                Content-Type: text/plain; charset=utf-8\r\n\r\nbody"
            );
            let converted = raw_of("headers", raw.as_bytes());
            let normalized = mime::normalize(&converted).unwrap();
            assert_eq!(normalized.subject, "café");
            assert_eq!(normalized.from, "José <j@example.com>");
            assert_eq!(
                normalized.to,
                ["Doe, Jane <jane@example.com>", "Other <other@example.com>"]
            );
            for (name, value) in [
                ("Cc", "René <r@example.com>"),
                ("Reply-To", "José <reply@example.com>"),
                (
                    "Authentication-Results",
                    "mx.example; dkim=fail header.d=example.com",
                ),
                (
                    "List-Unsubscribe",
                    "<https://example.com/=?UTF-8?Q?literal?=>",
                ),
                ("Content-Type", "text/plain; charset=utf-8"),
            ] {
                assert!(converted
                    .payload
                    .headers
                    .iter()
                    .any(|h| h.name == name && h.value == value));
            }
        }
    }

    #[test]
    fn decoded_header_controls_cannot_inject_headers_or_extra_recipients() {
        let raw = b"Subject: =?UTF-8?Q?Hi=0D=0ABcc:_victim@example.com?=\r\n\
            To: =?UTF-8?Q?Name=0D=0ABcc:_victim@example.com?= <real@example.com>\r\n\r\nbody";
        let converted = raw_of("hostile", raw);
        assert_eq!(converted.payload.headers.len(), 2);
        assert!(converted
            .payload
            .headers
            .iter()
            .all(|h| !h.value.contains(['\r', '\n'])));
        let normalized = mime::normalize(&converted).unwrap();
        assert_eq!(normalized.to.len(), 1);
        assert!(normalized.to[0].ends_with("<real@example.com>"));
    }

    #[test]
    fn empty_and_whitespace_only_parts_are_preserved_not_dropped() {
        // AGENTS.md: structure is not stripped for being empty/whitespace.
        let raw = b"Subject: Empty\r\nFrom: a@b.com\r\n\
Content-Type: multipart/mixed; boundary=\"E\"\r\n\r\n\
--E\r\nContent-Type: text/plain\r\n\r\n\r\n\
--E\r\nContent-Type: text/plain\r\n\r\n   \r\n\
--E\r\nContent-Type: text/html\r\n\r\n<p>real</p>\r\n--E--\r\n";
        let message = raw_of("imap:me@x:empty", raw);
        assert_eq!(message.payload.parts.len(), 3, "no part was dropped");
    }

    #[test]
    fn a_truncated_message_never_panics() {
        // Headers only, no terminating blank line, cut mid-header.
        for bytes in [
            &b"Subject: trunc"[..],
            &b"Content-Type: multipart/mixed; boundary=\"X\"\r\n\r\n--X\r\nContent-Type: text/pl"[..],
            &b""[..],
        ] {
            // Either a parsed best-effort tree or a typed error; never a panic.
            let _ = to_raw_message(bytes, "imap:me@x:trunc", vec![]);
        }
    }

    #[test]
    fn missing_boundaries_and_nul_bytes_are_handled() {
        let raw = b"Subject: Weird\r\nFrom: a@b.com\r\n\
Content-Type: multipart/mixed; boundary=\"Z\"\r\n\r\n\
no boundary ever appears\x00\x00 and then nul bytes\r\n";
        let message = to_raw_message(raw, "imap:me@x:weird", vec![]).unwrap();
        // Still normalizes without panicking.
        assert!(mime::normalize(&message).is_ok());
    }

    #[test]
    fn a_deeply_nested_message_is_rejected_by_policy_not_a_stack_overflow() {
        // Build MAX_MIME_DEPTH+2 nested multiparts so the depth gate fires.
        let depth = policy::MAX_MIME_DEPTH + 2;
        let mut body = String::from("deepest\r\n");
        for level in 0..depth {
            let b = format!("B{level}");
            body = format!(
                "Content-Type: multipart/mixed; boundary=\"{b}\"\r\n\r\n--{b}\r\n{body}--{b}--\r\n"
            );
        }
        let raw = format!("Subject: Deep\r\nFrom: a@b.com\r\n{body}");
        let err = to_raw_message(raw.as_bytes(), "imap:me@x:deep", vec![]).unwrap_err();
        assert!(
            matches!(err, ProviderError::PermanentClientRejection(_)),
            "a mail bomb is a typed rejection: {err:?}"
        );
    }

    #[test]
    fn an_oversize_message_is_rejected_before_parsing() {
        // A buffer one byte over the raw-bytes limit; content need not be valid.
        let big = vec![b'a'; policy::MAX_RAW_MESSAGE_BYTES + 1];
        let err = to_raw_message(&big, "imap:me@x:big", vec![]).unwrap_err();
        assert!(matches!(err, ProviderError::PermanentClientRejection(_)));
    }
}
