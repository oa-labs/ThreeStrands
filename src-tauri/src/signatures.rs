//! Portable signature templates and per-address defaults. Drafts carry their
//! own snapshot; sending never consults the mutable template library.
use serde::{Deserialize, Serialize};
use threestrands_sync_protocol::EntityType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signature {
    pub id: String,
    pub name: String,
    pub html: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignatureDefaults {
    pub id: String,
    pub account: String,
    /// Empty for account defaults; an alias row inherits when absent.
    pub address: String,
    pub new_signature_id: Option<String>,
    pub reply_signature_id: Option<String>,
}
impl SignatureDefaults {
    pub fn key(account: &str, address: &str) -> String {
        let account = account.trim().to_lowercase();
        let address = address.trim().to_lowercase();
        // Length-prefix the account so even unusual address punctuation
        // cannot make two pairs share a key. IDs stay within envelope bounds.
        let mut bytes = (account.len() as u64).to_be_bytes().to_vec();
        bytes.extend_from_slice(account.as_bytes());
        bytes.extend_from_slice(address.as_bytes());
        format!("signature-default:{}", sha256_hex(&bytes))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSignature {
    pub id: Option<String>,
    pub name: String,
    pub html: String,
    pub text: String,
    pub automatic: bool,
    // UTF-16 offsets in the body WITHOUT the signature. The composer captures
    // the authored/quoted boundary so MIME places the snapshot above history.
    pub html_offset: usize,
    pub text_offset: usize,
}
impl DraftSignature {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_none() && self.html.is_empty() && self.text.is_empty() {
            return Ok(());
        }
        EntityType::Signature.validate_payload(&serde_json::json!({
            "id": self.id.as_deref().unwrap_or("draft"), "name": self.name,
            "html": self.html, "text": self.text,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignatureLibrary {
    pub signatures: Vec<Signature>,
    pub defaults: Vec<SignatureDefaults>,
}

impl SignatureLibrary {
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for signature in &self.signatures {
            EntityType::Signature
                .validate_payload(&serde_json::to_value(signature).map_err(|e| e.to_string())?)?;
            embed_images(&signature.html, &signature.id)?;
            signature_sync_payload(signature)?;
            if !ids.insert(&signature.id) {
                return Err("Duplicate signature".into());
            }
        }
        ids.clear();
        for defaults in &self.defaults {
            EntityType::SignatureDefaults
                .validate_payload(&serde_json::to_value(defaults).map_err(|e| e.to_string())?)?;
            if defaults.id != SignatureDefaults::key(&defaults.account, &defaults.address) {
                return Err("Invalid signature defaults identity".into());
            }
            if !ids.insert(&defaults.id) {
                return Err("Duplicate signature defaults".into());
            }
        }
        Ok(())
    }
}

/// Offsets are validated, never clamped. JS counts UTF-16 code units while
/// Rust slices UTF-8; rejecting a split surrogate keeps Unicode intact.
pub fn utf16_byte_offset(body: &str, offset: usize) -> Result<usize, String> {
    let mut units = 0;
    let mut byte = body.len();
    for (index, ch) in body.char_indices() {
        if units == offset {
            byte = index;
            break;
        }
        units += ch.len_utf16();
    }
    if units != offset {
        return Err("The signature position is invalid. Reopen the draft.".into());
    }
    Ok(byte)
}

pub fn insert_signature(
    body: &str,
    offset: usize,
    signature: &str,
    html: bool,
) -> Result<String, String> {
    let byte = utf16_byte_offset(body, offset)?;
    let separator = if html { "<br><br>" } else { "\n\n" };
    Ok(format!(
        "{}{}{}{}{}",
        &body[..byte],
        if byte > 0 && !body[..byte].ends_with(separator) {
            separator
        } else {
            ""
        },
        signature,
        if byte < body.len() && !body[byte..].starts_with(separator) {
            separator
        } else {
            ""
        },
        &body[byte..]
    ))
}

/// Signature editor HTML uses canonical double-quoted raster data URIs. No
/// remote fetch or local file read is permitted here. Validate decoded pixels
/// again at the native boundary and send each image as a MIME related part.
pub fn embed_images(
    html: &str,
    message_id: &str,
) -> Result<(String, Vec<(String, String, Vec<u8>)>), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut output = String::new();
    let mut rest = html;
    let mut images = Vec::new();
    let sources = canonical_image_sources(html);
    while let Some(start) = rest.find("src=\"data:") {
        let uri_start = start + "src=\"".len();
        // Stored editor HTML is canonical. A data URI mentioned in ordinary
        // text is not an image capability and must remain ordinary text.
        if !sources.contains(&(html.len() - rest.len() + start)) {
            output.push_str(&rest[..uri_start]);
            rest = &rest[uri_start..];
            continue;
        }
        let end = rest[uri_start..]
            .find('"')
            .ok_or("Invalid signature image")?
            + uri_start;
        let uri = &rest[uri_start..end];
        let (mime, encoded) = uri
            .strip_prefix("data:")
            .and_then(|value| value.split_once(";base64,"))
            .ok_or("Invalid signature image")?;
        if !crate::image_format::is_supported_raster_mime(mime) {
            return Err("Unsupported signature image format".into());
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid signature image data")?;
        crate::image_format::validate_raster(&bytes)?;
        let cid = format!("signature-{}-{message_id}@threestrands.local", images.len());
        output.push_str(&rest[..uri_start]);
        output.push_str(&format!("cid:{cid}"));
        images.push((mime.to_string(), cid, bytes));
        rest = &rest[end..];
    }
    output.push_str(rest);
    Ok((output, images))
}

/// The editable sanitizer serializes quoted attributes. Track their bounds
/// so an angle bracket or URI mentioned in alt text cannot become a tag or
/// source, and genuine image sources are found regardless of attribute order.
fn canonical_image_sources(html: &str) -> std::collections::BTreeSet<usize> {
    let bytes = html.as_bytes();
    let mut sources = std::collections::BTreeSet::new();
    let (mut in_tag, mut image, mut quote) = (false, false, None);
    for (index, byte) in bytes.iter().copied().enumerate() {
        if !in_tag {
            if byte == b'<' {
                in_tag = true;
                image = bytes
                    .get(index + 1..index + 4)
                    .is_some_and(|name| name.eq_ignore_ascii_case(b"img"))
                    && bytes
                        .get(index + 4)
                        .is_some_and(|ch| ch.is_ascii_whitespace() || *ch == b'>');
            }
        } else if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else {
            if image
                && index > 0
                && bytes[index - 1].is_ascii_whitespace()
                && bytes[index..].starts_with(b"src=\"data:")
            {
                sources.insert(index);
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'>' => in_tag = false,
                _ => {}
            }
        }
    }
    sources
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn template(id: &str) -> Signature {
        Signature {
            id: id.into(),
            name: id.into(),
            html: format!("<p>{id}</p>"),
            text: id.into(),
        }
    }
    pub(crate) fn defaults(
        account: &str,
        address: &str,
        new: Option<&str>,
        reply: Option<&str>,
    ) -> SignatureDefaults {
        SignatureDefaults {
            id: SignatureDefaults::key(account, address),
            account: account.into(),
            address: address.into(),
            new_signature_id: new.map(str::to_string),
            reply_signature_id: reply.map(str::to_string),
        }
    }

    #[test]
    fn signature_chunks_respect_envelope_value_boundaries_and_keep_unicode_intact() {
        let limit = threestrands_sync_envelope::limits::MAX_VALUE_BYTES;
        for bytes in [limit - 1, limit, limit + 1] {
            let mut signature = template("work");
            signature.html = "a".repeat(bytes - 2);
            let wire = signature_sync_payload(&signature).unwrap();
            for value in wire.as_object().unwrap().values() {
                assert!(serde_json::to_vec(value).unwrap().len() <= limit);
            }
            assert_eq!(
                wire.as_object()
                    .unwrap()
                    .keys()
                    .filter(|key| key.starts_with("html:"))
                    .count(),
                if bytes <= limit { 1 } else { 2 }
            );
            assert_eq!(signature_from_sync(&wire).unwrap().html, signature.html);
        }
        for html in [
            format!("a{}", "\n".repeat(limit / 2)),
            "😀".repeat(limit / 4),
        ] {
            let mut signature = template("unicode");
            signature.html = html;
            let wire = signature_sync_payload(&signature).unwrap();
            assert_eq!(signature_from_sync(&wire).unwrap().html, signature.html);
            for value in wire.as_object().unwrap().values() {
                assert!(serde_json::to_vec(value).unwrap().len() <= limit);
            }
        }
    }

    #[test]
    fn a_chosen_content_hash_selects_one_complete_concurrent_version() {
        let mut first = template("work");
        first.html = format!("<b>{}</b>", "A".repeat(80_000));
        let mut second = first.clone();
        second.html = format!("<p>{}</p>", "B".repeat(90_000));
        second.text = "Second".into();
        let mut merged = signature_sync_payload(&first).unwrap();
        let second_wire = signature_sync_payload(&second).unwrap();
        merged
            .as_object_mut()
            .unwrap()
            .extend(second_wire.as_object().unwrap().clone());
        let chosen = selected_signature_payload(&merged).unwrap();
        EntityType::Signature.validate_payload(&chosen).unwrap();
        let result = signature_from_sync(&chosen).unwrap();
        assert_eq!((result.html, result.text), (second.html, second.text));
        let mut corrupted = chosen;
        let key = corrupted
            .as_object()
            .unwrap()
            .keys()
            .find(|key| key.starts_with("html:"))
            .unwrap()
            .clone();
        corrupted[&key] = serde_json::json!("Changed chunk");
        assert!(signature_from_sync(&corrupted).is_err());
    }

    #[test]
    fn signature_boundaries_preserve_unicode_and_reject_invalid_offsets() {
        assert_eq!(
            insert_signature("Hi 😀\n\nQuote", 5, "Thanks", false).unwrap(),
            "Hi 😀\n\nThanks\n\nQuote"
        );
        assert!(insert_signature("😀", 1, "s", true).is_err());
        assert!(insert_signature("a", 2, "s", true).is_err());
        assert_eq!(insert_signature("a", 1, "s", true).unwrap(), "a<br><br>s");
    }

    #[test]
    fn image_embedding_never_fetches_and_rejects_active_or_invalid_image_data() {
        for source in [
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:text/html;base64,PGI+",
            "data:image/png;base64,YmFk",
        ] {
            assert!(embed_images(&format!("<img src=\"{source}\">"), "message").is_err());
        }
        let (html, images) =
            embed_images("<img src=\"https://remote.invalid/x\">", "message").unwrap();
        assert!(images.is_empty());
        assert!(html.contains("https://remote.invalid"));
        let literal = "<p>Example src=\"data:text/plain;base64,aA==\"</p>";
        let (html, images) = embed_images(literal, "message").unwrap();
        assert_eq!(html, literal);
        assert!(images.is_empty());
        assert!(embed_images(
            "<img alt=\"Company <name>\" src=\"data:image/png;base64,YmFk\">",
            "message"
        )
        .is_err());
        let literal = "<img alt='Example src=\"data:text/plain;base64,aA==\"'>";
        assert!(embed_images(literal, "message").unwrap().1.is_empty());
    }
}

/// Each chunk is one envelope value. The hash identifies a complete HTML
/// and text pair; choosing a concurrent content hash therefore selects one
/// coherent version, never chunks from different edits.
pub(crate) fn signature_sync_payload(item: &Signature) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let hash = signature_content_hash(&item.html, &item.text)?;
    let mut value = json!({"id": item.id, "name": item.name, "contentHash": hash});
    for (kind, content) in [("html", &item.html), ("text", &item.text)] {
        for (index, chunk) in value_sized_chunks(content).into_iter().enumerate() {
            value[format!("{kind}:{hash}:{index}")] = json!(chunk);
        }
    }
    EntityType::Signature.validate_payload(&value)?;
    Ok(value)
}

/// Splits `content` into pieces whose JSON string encoding each fits the
/// envelope's per-value budget. Always returns at least one piece.
pub(crate) fn value_sized_chunks(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut size = 2; // JSON's surrounding quotes.
    for ch in content.chars() {
        let cost = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\x08' | '\x0c' => 2,
            ch if ch <= '\x1f' => 6,
            ch => ch.len_utf8(),
        };
        if size + cost > threestrands_sync_envelope::limits::MAX_VALUE_BYTES {
            chunks.push(std::mem::take(&mut chunk));
            size = 2;
        }
        chunk.push(ch);
        size += cost;
    }
    chunks.push(chunk);
    chunks
}

/// Inactive concurrent chunks remain in the replica until the next local
/// edit observes and removes them. Only the chosen version materializes.
pub(crate) fn selected_signature_payload(
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    if value.get("html").is_some() {
        return Ok(value.clone());
    }
    let hash = value
        .get("contentHash")
        .and_then(serde_json::Value::as_str)
        .ok_or("Missing signature content hash")?;
    let mut selected = value.as_object().ok_or("Invalid signature record")?.clone();
    selected.retain(|key, _| {
        let parts: Vec<_> = key.split(':').collect();
        !(parts.len() == 3
            && ["html", "text"].contains(&parts[0])
            && parts[1].len() == 64
            && parts[1].bytes().all(|ch| ch.is_ascii_hexdigit())
            && parts[1] != hash)
    });
    Ok(serde_json::Value::Object(selected))
}

pub(crate) fn signature_from_sync(value: &serde_json::Value) -> Result<Signature, String> {
    let (html, text) = threestrands_sync_protocol::signature_content(value)?;
    if let Some(hash) = value.get("contentHash").and_then(serde_json::Value::as_str) {
        let actual = signature_content_hash(&html, &text)?;
        if hash != actual {
            return Err("Signature content checksum mismatch".into());
        }
    }
    Ok(Signature {
        id: value["id"].as_str().ok_or("Missing signature id")?.into(),
        name: value["name"]
            .as_str()
            .ok_or("Missing signature name")?
            .into(),
        html,
        text,
    })
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn signature_content_hash(html: &str, text: &str) -> Result<String, String> {
    Ok(sha256_hex(
        &serde_json::to_vec(&(html, text)).map_err(|e| e.to_string())?,
    ))
}
