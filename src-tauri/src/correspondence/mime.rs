//! Address parsing, reply and forward helpers, and building the MIME
//! message a draft sends as.

use crate::{
    limits::MAX_ATTACHMENT_BYTES,
    mime::{MimePart, RawMessage},
};
use mail_builder::MessageBuilder;
use mail_parser::MessageParser;
use std::path::Path;
use uuid::Uuid;

use super::{error, Attachment, Draft};

const MAX_ENCODED_MIME_BYTES: usize = 24 * 1024 * 1024;

/// Reads addresses already stored from received mail, where a display name
/// may hold an unquoted comma (`Daniel O'Connor, CFA® <dan@example.com>`)
/// that strict parsing rejects. Falls back to the one bracketed or bare
/// address in the value; anything else yields nothing. Never use this to
/// validate addresses the user enters.
pub fn stored_addresses(value: &str) -> Vec<(String, String)> {
    if let Ok(list) = addresses(value) {
        return list;
    }
    let value = value.trim();
    if value.contains(['\r', '\n']) {
        return vec![];
    }
    let (name, address) = match value.strip_suffix('>').and_then(|rest| rest.rsplit_once('<')) {
        Some((name, address)) => (name.trim().trim_matches('"').trim(), address.trim()),
        None => ("", value),
    };
    let valid = address
        .rsplit_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.') && !domain.starts_with('.'))
        && !address.contains([' ', '\t', '<', '>', ',', '"']);
    if valid { vec![(name.to_string(), address.to_string())] } else { vec![] }
}

pub fn addresses(value: &str) -> Result<Vec<(String, String)>, String> {
    if value.trim().is_empty() {
        return Ok(vec![]);
    }
    if value.contains(['\r', '\n']) {
        return Err("Addresses cannot contain newlines".into());
    }
    let raw = format!("To: {value}\r\n\r\n");
    let message = MessageParser::default()
        .parse(raw.as_bytes())
        .ok_or("Invalid recipients")?;
    let list = message.to().ok_or("Invalid recipients")?;
    let mut result = vec![];
    for a in list.iter() {
        let address = a.address().ok_or("Invalid email address")?;
        let (local, domain) = address
            .rsplit_once('@')
            .ok_or("Enter a complete email address")?;
        if local.is_empty() || domain.is_empty() || address.contains([' ', '\t', '<', '>']) {
            return Err("Invalid email address".into());
        }
        result.push((
            a.name().unwrap_or_default().to_string(),
            address.to_string(),
        ));
    }
    if result.is_empty() {
        return Err("Enter an email address".into());
    }
    Ok(result)
}
pub(super) fn header<'a>(part: &'a MimePart, name: &str) -> &'a str {
    part.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
        .unwrap_or_default()
}
pub(super) fn reference_ids(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|s| s.trim_matches(['<', '>']).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
/// Renders a source message's normalized RFC 3339 date the way a reader
/// expects to see it in a reply attribution ("Mon, Oct 5, 2026 at 9:32 AM"),
/// in the reader's time zone. An unparseable date is quoted as-is rather than
/// dropped, so the attribution line keeps its shape.
pub(super) fn attribution_date<Tz: chrono::TimeZone>(rfc3339: &str, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|date| {
            date.with_timezone(zone)
                .format("%a, %b %-d, %Y at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

pub(super) fn plain_text_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}

pub(super) fn forward_attachments(source: &RawMessage, normalized: &crate::mime::NormalizedMessage) -> Result<Vec<Attachment>, String> {
    normalized.attachments.iter().map(|attachment| {
        Ok(Attachment {
            id: Uuid::new_v4().to_string(),
            name: attachment.filename.clone(),
            size: attachment.size,
            mime: attachment.mime_type.clone(),
            ready: false,
            message_id: Some(source.id.clone()),
            provider_id: crate::mime::provider_attachment_id_from_payload(source, &attachment.id)?,
            inline: attachment.inline,
            content_id: attachment.content_id.clone(),
        })
    }).collect()
}

pub(crate) fn build_mime(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    id: &str,
    root: &Path,
) -> Result<Vec<u8>, String> {
    build_message(d, identities, account_display_name, message_id_domain, id, root, true)
}

/// The server copy of a draft: the message exactly as it would be sent, but
/// with no recipients allowed and a `Message-ID` that stays the same across
/// every push, so any device can find the copy again
/// (`docs/server-drafts-design.md`, "Server message").
pub(crate) fn build_draft_mime(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    root: &Path,
) -> Result<Vec<u8>, String> {
    let id = crate::draft_push::server_message_local_part(&d.id);
    build_message(d, identities, account_display_name, message_id_domain, &id, root, false)
}

fn build_message(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    id: &str,
    root: &Path,
    sending: bool,
) -> Result<Vec<u8>, String> {
    if d.subject.contains(['\r', '\n']) {
        return Err("Subject cannot contain newlines".into());
    }
    let to = addresses(&d.to)?;
    let cc = addresses(&d.cc)?;
    let bcc = addresses(&d.bcc)?;
    if sending && to.len() + cc.len() + bcc.len() == 0 {
        return Err("Add at least one recipient".into());
    }

    // Resolve the From address and its display name from the account's
    // identity list (the one source of truth), NOT from a value stored on the
    // draft. `sender_address()` is `d.from` when a non-primary identity was
    // chosen, else the account's own address.
    //
    // An EMPTY identity list is treated as the single identity `[account]`:
    // the production callers always pass the provider's non-empty,
    // login-first list, so this only serves a defensive/test path and keeps
    // the account's own address a valid From.
    let account_identity = [crate::provider::SendingIdentity {
        address: d.account.clone(),
        display_name: None,
    }];
    let identities: &[crate::provider::SendingIdentity] = if identities.is_empty() {
        &account_identity
    } else {
        identities
    };
    let from_address = d.sender_address();
    let from_normalized =
        crate::provider::imap::normalize_identity_address(from_address);
    let matched = identities
        .iter()
        .find(|identity| {
            crate::provider::imap::normalize_identity_address(&identity.address) == from_normalized
        });
    // A `from` explicitly set to an address that is NOT one of the account's
    // identities (a stale identity removed from settings) must fail CLEARLY,
    // never silently send as another address. When `from` is unset the From is
    // the account address, which is always identity 0, so this only bites a
    // deliberately-chosen stale identity.
    if d.from.is_some() && matched.is_none() {
        return Err(format!(
            "The chosen sending identity ({from_address}) is no longer on this account. \
             Pick a current identity before sending."
        ));
    }
    // Display name: prefer the matched identity's own display name; for the
    // account's primary address fall back to the account display name, so
    // Gmail and single-identity sends keep their existing From display name
    // byte-for-byte.
    let display_name = matched
        .and_then(|identity| identity.display_name.as_deref())
        .or_else(|| {
            let is_primary = identities
                .first()
                .map(|primary| {
                    crate::provider::imap::normalize_identity_address(&primary.address)
                        == from_normalized
                })
                .unwrap_or(true);
            if is_primary {
                account_display_name
            } else {
                None
            }
        });

    // Header-injection guard: neither the From address nor its display name may
    // carry CR/LF/NUL. The address already passed envelope validation when the
    // identity set was built, but a draft's `from`/account could in principle
    // hold anything, and the display name is free-form, so assert here at the
    // point bytes are produced.
    if from_address.contains(['\r', '\n', '\0']) {
        return Err("Sender address cannot contain line breaks".into());
    }
    if let Some(name) = display_name {
        if name.contains(['\r', '\n', '\0']) {
            return Err("Sender display name cannot contain line breaks".into());
        }
    }

    let builder = MessageBuilder::new();
    let builder = if let Some(name) = display_name {
        builder.from((name.to_string(), from_address.to_string()))
    } else {
        builder.from(from_address.to_string())
    };
    let mut body = d.body.clone();
    let mut body_html = d.body_html.clone();
    let mut signature_images = Vec::new();
    if let Some(signature) = &d.signature {
        signature.validate()?;
        if !signature.html.is_empty() {
            let (html, images) = crate::signatures::embed_images(&signature.html, id)?;
            signature_images = images;
            body = crate::signatures::insert_signature(&body, signature.text_offset, &signature.text, false)?;
            if body_html.is_empty() {
                // Native-created drafts have no authored HTML yet. The native
                // signature boundary starts at zero, above their reply history.
                let byte = crate::signatures::utf16_byte_offset(&d.body, signature.text_offset)?;
                let prefix = plain_text_html(&d.body[..byte]);
                let suffix = plain_text_html(&d.body[byte..]);
                body_html = format!("{prefix}{}{html}{}{suffix}", if byte > 0 { "<br><br>" } else { "" }, if byte < d.body.len() && !d.body[byte..].starts_with("\n\n") { "<br><br>" } else { "" });
            } else {
                body_html = crate::signatures::insert_signature(&body_html, signature.html_offset, &html, true)?;
            }
        }
    }
    if let Some(forwarded) = d.forwarded_edit.as_ref().filter(|_| d.forwarded_content.is_some()).or(d.forwarded_content.as_ref()) {
        body.push_str(&format!("\n\n{}", forwarded.text));
        if body_html.trim().is_empty() {
            body_html = plain_text_html(&d.body);
        }
        body_html.push_str(&format!("<br><br>{}", forwarded.html));
    }
    // The Message-ID domain: the provider's hook result when it supplied one
    // (IMAP returns the From address's domain), else the app's reserved
    // `threestrands.local`. Gmail passes `None`, so its Message-ID is
    // byte-identical to before. The builder adds the surrounding `<…>`.
    let domain = message_id_domain.unwrap_or("threestrands.local");
    let mut builder = builder
        .to(to)
        .cc(cc)
        .bcc(bcc)
        .subject(d.subject.clone())
        .text_body(body.clone())
        .message_id(format!("{id}@{domain}"));
    if !body_html.trim().is_empty() {
        builder = builder.html_body(body_html.clone());
    }
    if let Some(reply) = &d.reply_id {
        if reply.contains(['\r', '\n']) || d.references.iter().any(|r| r.contains(['\r', '\n'])) {
            return Err("Invalid reply headers".into());
        }
        builder = builder
            .in_reply_to(reply.clone())
            .references(d.references.clone());
    }
    let mut size = body.len() + body_html.len();
    for (mime, cid, bytes) in signature_images {
        size += bytes.len();
        builder = builder.inline(mime, cid, bytes);
    }
    for attachment in &d.attachments {
        if !attachment.ready {
            return Err(if attachment.message_id.is_some() {
                format!("Download or remove {} before sending", attachment.name)
            } else {
                // Attachment bytes stay on the device that added them.
                format!("{} was attached on another device. Attach it again here, or remove it, before sending", attachment.name)
            });
        }
        if Uuid::parse_str(&attachment.id).is_err() {
            return Err("Invalid attachment ID".into());
        }
        let path = root.join(&attachment.id);
        let metadata = std::fs::metadata(&path)
            .map_err(|_| format!("Attachment missing: {}", attachment.name))?;
        size += metadata.len() as usize;
        if size > MAX_ATTACHMENT_BYTES {
            return Err("Attachments exceed the 18 MB local limit".into());
        }
        let bytes = std::fs::read(path).map_err(error)?;
        builder = if attachment.inline {
            let content_id = attachment
                .content_id
                .as_deref()
                .ok_or("Inline image has no content ID")?;
            builder.inline(attachment.mime.clone(), content_id.to_string(), bytes)
        } else {
            builder.attachment(attachment.mime.clone(), attachment.name.clone(), bytes)
        };
    }
    let raw = builder.write_to_vec().map_err(error)?;
    if raw.len() > MAX_ENCODED_MIME_BYTES {
        return Err("Encoded message exceeds the 24 MB local limit".into());
    }
    Ok(raw)
}
