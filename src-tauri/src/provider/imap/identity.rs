//! Stable, cross-account IMAP message identity — pure functions, no I/O.
//!
//! `docs/imap-design.md` ("Message identity") fixes the scheme. A message's id
//! must stay stable across moves (IMAP UIDs do not) and be unique across
//! accounts (`messages.id` is a primary key over every account):
//!
//! * `imap:<account>:<EMAILID>` when the server returned an RFC 8474
//!   `OBJECTID` `EMAILID`.
//! * otherwise `imap:<account>:<hash>`, where `<hash>` is a hex SHA-256 over
//!   the normalized `Message-ID` header.
//! * a message with NO usable `Message-ID` hashes `Date`, `From`, `Subject`
//!   and `RFC822.SIZE` instead.
//!
//! The two hash inputs are **domain-separated** (each prefixed with a distinct,
//! unambiguous tag) so a crafted `Message-ID` can never produce the same hash
//! as a real no-`Message-ID` message, and vice versa.
//!
//! Everything here takes server- or sender-controlled input, so nothing may
//! panic: no `unwrap`/`expect`/indexing on that data. The one shared
//! [`normalize_message_id`] is `pub(super)` because Slice 5 threading reuses
//! it for `In-Reply-To` / `References`.

use sha2::{Digest, Sha256};

/// The identity prefix every IMAP message id carries.
const SCHEME: &str = "imap:";

/// Domain-separation tag for the `Message-ID` hash input. The trailing newline
/// cannot appear inside the single normalized Message-ID token (whitespace is
/// stripped by [`normalize_message_id`]), so this prefix can never be forged
/// by the no-Message-ID input below.
const TAG_MESSAGE_ID: &str = "msgid\n";

/// Domain-separation tag for the no-`Message-ID` fallback hash input. Distinct
/// from [`TAG_MESSAGE_ID`]; the fields are joined with a NUL that cannot occur
/// in the single-token Message-ID path.
const TAG_SYNTHETIC: &str = "synthetic\n";

/// Normalize a `Message-ID`-style header value into a single comparable token.
///
/// Strips RFC 5322 comments (`(...)`, which may nest), surrounding angle
/// brackets, and all whitespace, then ASCII-lowercases the result. This is the
/// one spelling every part of the IMAP provider compares `Message-ID`,
/// `In-Reply-To` and `References` by, so two references to the same message
/// always match regardless of folding, case or stray comments.
///
/// Never panics on hostile input: unbalanced brackets or comments degrade to a
/// best-effort token rather than erroring.
pub(super) fn normalize_message_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth: usize = 0;
    for ch in raw.chars() {
        match ch {
            // RFC 5322 comments may nest; drop everything inside them.
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            // Angle brackets delimit the id; whitespace is never significant.
            '<' | '>' => {}
            c if c.is_whitespace() => {}
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Encode an account id so it is unambiguous inside `imap:<account>:<id>`.
///
/// `account` is an email, which cannot contain a space but CAN contain the
/// `:` the id uses as its delimiter (quoted local parts, or a hostile crafted
/// value), and `messages.id` is a primary key where a collision would silently
/// merge two accounts' mail. Percent-encode `:` and `%` (the escape char
/// itself) so decoding is unambiguous and no two distinct accounts can ever
/// encode to the same string. Everything else is left readable.
fn encode_account(account: &str) -> String {
    let mut out = String::with_capacity(account.len());
    for byte in account.bytes() {
        match byte {
            b':' => out.push_str("%3A"),
            b'%' => out.push_str("%25"),
            _ => out.push(byte as char),
        }
    }
    out
}

/// Assemble `imap:<encoded-account>:<body>` from an already-chosen id body
/// (an EMAILID or a hash). The account is delimiter-encoded; the body is used
/// verbatim (callers only ever pass a hex hash or a server OBJECTID).
fn assemble(account: &str, body: &str) -> String {
    format!("{SCHEME}{}:{body}", encode_account(account))
}

/// Hash the normalized `Message-ID` into the id body, domain-separated so it
/// can never equal a [`synthetic_body`] hash.
fn message_id_body(message_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TAG_MESSAGE_ID.as_bytes());
    hasher.update(normalize_message_id(message_id).as_bytes());
    hex(&hasher.finalize())
}

/// Hash `Date` / `From` / `Subject` / `RFC822.SIZE` into the id body for a
/// message with no usable `Message-ID`, domain-separated from the Message-ID
/// path. Fields are NUL-joined; NUL cannot appear in the single-token
/// Message-ID input, so the two input spaces are disjoint.
fn synthetic_body(date: &str, from: &str, subject: &str, rfc822_size: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TAG_SYNTHETIC.as_bytes());
    for field in [date, from, subject] {
        hasher.update(field.as_bytes());
        hasher.update([0u8]);
    }
    hasher.update(rfc822_size.to_be_bytes());
    hex(&hasher.finalize())
}

/// Lowercase hex of a byte slice. Local helper so identity never pulls in a
/// hex crate and never indexes.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// The header inputs needed to derive an id for one message, as the identity
/// fetch pass collects them. All fields are optional because a hostile or
/// minimal message may omit any of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityInputs {
    /// RFC 8474 `EMAILID` (`OBJECTID`), when the server returned one.
    pub email_id: Option<String>,
    /// The raw `Message-ID` header value, when present.
    pub message_id: Option<String>,
    /// `Date` header (raw), for the no-Message-ID fallback.
    pub date: Option<String>,
    /// `From` header (raw), for the no-Message-ID fallback.
    pub from: Option<String>,
    /// `Subject` header (raw), for the no-Message-ID fallback.
    pub subject: Option<String>,
    /// `RFC822.SIZE`, for the no-Message-ID fallback.
    pub rfc822_size: u32,
}

/// Derive the stable message id for `account` from `inputs`.
///
/// Order, per the design: a server EMAILID wins; otherwise the normalized
/// `Message-ID` is hashed; a message with no usable `Message-ID` (absent, or
/// present but normalizing to the empty string) hashes `Date`/`From`/`Subject`/
/// `RFC822.SIZE`. The account id is delimiter-encoded in every branch.
///
/// Sticky reuse ([the design's "a server that later advertises OBJECTID must
/// not re-key messages we already know"]) is handled by the caller, which
/// looks the location up first and only calls this when no id is on record.
pub fn derive_message_id(account: &str, inputs: &IdentityInputs) -> String {
    if let Some(email_id) = inputs.email_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
        return assemble(account, email_id);
    }
    if let Some(message_id) = inputs.message_id.as_deref() {
        if !normalize_message_id(message_id).is_empty() {
            return assemble(account, &message_id_body(message_id));
        }
    }
    assemble(
        account,
        &synthetic_body(
            inputs.date.as_deref().unwrap_or_default(),
            inputs.from.as_deref().unwrap_or_default(),
            inputs.subject.as_deref().unwrap_or_default(),
            inputs.rfc822_size,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_brackets_comments_whitespace_and_lowercases() {
        // A table of spellings that must all collapse to one token.
        let canonical = "abc123@example.com";
        for raw in [
            "<abc123@example.com>",
            "  <ABC123@Example.COM>  ",
            "<abc123@example.com> (generated by server)",
            "(leading comment)<abc123@example.com>",
            "<abc1\r\n 23@example.com>", // folded
            "ABC123@EXAMPLE.COM",
        ] {
            assert_eq!(normalize_message_id(raw), canonical, "raw: {raw:?}");
        }
    }

    #[test]
    fn normalization_handles_hostile_input_without_panicking() {
        // Unbalanced brackets/comments, nested comments, only punctuation,
        // and empty — none may panic, all return a best-effort token.
        // An UNterminated comment safely swallows the rest (depth never
        // returns to 0): that is the conservative, non-panicking behaviour.
        assert_eq!(normalize_message_id("(((unclosed"), "");
        assert_eq!(normalize_message_id(")))"), "");
        assert_eq!(normalize_message_id("<<<>>>"), "");
        assert_eq!(normalize_message_id(""), "");
        // A balanced nested comment keeps the token after it.
        assert_eq!(normalize_message_id("(a(b(c)d)e)keep"), "keep");
        // Extra stray close-parens never underflow (saturating), and the
        // token after a fully-closed comment survives.
        assert_eq!(normalize_message_id("())(x)y"), "y");
    }

    #[test]
    fn an_emailid_takes_precedence_and_is_used_verbatim() {
        let inputs = IdentityInputs {
            email_id: Some("M6d952b5c6f82bfd8".into()),
            message_id: Some("<ignored@example.com>".into()),
            ..Default::default()
        };
        assert_eq!(
            derive_message_id("me@example.com", &inputs),
            "imap:me@example.com:M6d952b5c6f82bfd8"
        );
    }

    #[test]
    fn without_an_emailid_the_normalized_message_id_is_hashed() {
        // Two spellings of the same Message-ID yield the same id; a different
        // Message-ID yields a different one.
        let id = |raw: &str| {
            derive_message_id(
                "me@example.com",
                &IdentityInputs {
                    message_id: Some(raw.into()),
                    ..Default::default()
                },
            )
        };
        assert_eq!(id("<a@b.com>"), id("  <A@B.COM> (x)"));
        assert_ne!(id("<a@b.com>"), id("<c@d.com>"));
        assert!(id("<a@b.com>").starts_with("imap:me@example.com:"));
        // The body is a 64-char hex SHA-256.
        let body = id("<a@b.com>").rsplit(':').next().unwrap().to_string();
        assert_eq!(body.len(), 64);
        assert!(body.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_message_without_a_usable_message_id_hashes_the_envelope_fields() {
        // Absent Message-ID and a Message-ID that normalizes to empty both
        // take the synthetic path, and both are stable.
        let synthetic = |message_id: Option<&str>| {
            derive_message_id(
                "me@example.com",
                &IdentityInputs {
                    message_id: message_id.map(str::to_string),
                    date: Some("Mon, 06 Oct 2025 09:00:00 +0000".into()),
                    from: Some("alice@example.test".into()),
                    subject: Some("Hi".into()),
                    rfc822_size: 1234,
                    ..Default::default()
                },
            )
        };
        assert_eq!(synthetic(None), synthetic(Some("<>")));
        assert_eq!(synthetic(None), synthetic(Some("   ")));
        // Changing any field changes the id.
        let base = synthetic(None);
        let mut other = IdentityInputs {
            date: Some("Mon, 06 Oct 2025 09:00:00 +0000".into()),
            from: Some("alice@example.test".into()),
            subject: Some("Hi".into()),
            rfc822_size: 1235, // one byte different
            ..Default::default()
        };
        assert_ne!(base, derive_message_id("me@example.com", &other));
        other.rfc822_size = 1234;
        other.subject = Some("Hi ".into()); // trailing space differs
        assert_ne!(base, derive_message_id("me@example.com", &other));
    }

    #[test]
    fn the_two_hash_inputs_are_domain_separated_and_cannot_collide() {
        // A crafted Message-ID equal to the synthetic field join must NOT
        // produce the synthetic id. Field order joined by NUL is unspellable
        // inside a single Message-ID token (whitespace and NUL are stripped),
        // but prove the tags keep them apart regardless.
        let via_message_id = derive_message_id(
            "me@example.com",
            &IdentityInputs {
                message_id: Some("synthetic".into()),
                ..Default::default()
            },
        );
        let via_synthetic = derive_message_id(
            "me@example.com",
            &IdentityInputs {
                subject: Some("synthetic".into()),
                ..Default::default()
            },
        );
        assert_ne!(via_message_id, via_synthetic);
        // And the raw hash bodies differ for identical-looking content.
        assert_ne!(message_id_body("x"), synthetic_body("x", "", "", 0));
    }

    #[test]
    fn a_delimiter_confusing_account_id_cannot_collide_with_another() {
        // Two different accounts whose emails differ only around the ':'
        // delimiter must never encode to the same id. Without encoding,
        // ("a:b", "c") and ("a", "b:c") would both render "imap:a:b:c".
        let a = derive_message_id(
            "a:b",
            &IdentityInputs {
                email_id: Some("c".into()),
                ..Default::default()
            },
        );
        let b = derive_message_id(
            "a",
            &IdentityInputs {
                email_id: Some("b:c".into()),
                ..Default::default()
            },
        );
        assert_ne!(a, b);
        assert_eq!(a, "imap:a%3Ab:c");
        assert_eq!(b, "imap:a:b:c");
        // The escape character itself is encoded, so "a%3Ab" (literal) and
        // "a:b" (encoded to a%3Ab) stay distinct.
        let literal_percent = derive_message_id(
            "a%3Ab",
            &IdentityInputs {
                email_id: Some("c".into()),
                ..Default::default()
            },
        );
        assert_ne!(a, literal_percent);
        assert_eq!(literal_percent, "imap:a%253Ab:c");
    }

    #[test]
    fn derivation_never_panics_on_empty_or_hostile_input() {
        // Entirely empty inputs still produce a well-formed synthetic id.
        let id = derive_message_id("", &IdentityInputs::default());
        assert!(id.starts_with("imap::"));
        // A hostile account and Message-ID do not panic.
        let _ = derive_message_id("\u{0}:\u{0}", &IdentityInputs {
            message_id: Some("<\u{0}>".into()),
            ..Default::default()
        });
    }
}
