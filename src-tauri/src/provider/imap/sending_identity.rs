//! Sending identities: the one source of truth for "which addresses may this
//! account send as", plus the two pure decisions built on it — reply-identity
//! selection and the Message-ID domain.
//!
//! `docs/imap-design.md` ("Identities" / "Sending" > "Message-ID domain").
//! An IMAP account can send from several addresses over one login (Proton
//! Bridge combined-addresses mode), so each account carries an ordered
//! identity list. This module turns that stored list into the normalized,
//! validated, login-anchored set every "sent by me" decision and the From
//! picker read, and nothing here branches on a provider name or id — the
//! provider seam does that, through the defaulted [`MailSend::identities`] and
//! [`MailSend::message_id_domain`] hooks.
//!
//! NORMALIZATION FOR COMPARISON is deliberately an ASCII-lowercase of the
//! WHOLE address: `Me@Proton.ME` and `me@proton.me` are the same identity, but
//! plus-tags are **not** stripped — `me+newsletter@proton.me` is a DISTINCT
//! sending address a user may legitimately configure as its own identity, and
//! silently folding it into `me@proton.me` would send replies from the wrong
//! address. This matches `db::normalize_sender`'s address half (lowercase, no
//! tag stripping), so the two never disagree about whether a From is "mine".

use super::settings::Identity;
use super::{policy, smtp};
use crate::provider::ProviderError;
pub use crate::provider::SendingIdentity;

/// The address of a sending identity, lowercased for comparison (see the
/// module note: whole address, plus-tags NOT stripped).
pub fn identity_normalized(identity: &SendingIdentity) -> String {
    normalize_identity_address(&identity.address)
}

/// Normalize an address for identity comparison: ASCII-lowercase the WHOLE
/// address. Plus-tags are intentionally preserved (see the module note).
pub fn normalize_identity_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

/// Build an account's normalized, validated, ordered sending-identity set from
/// its login address and its stored identity list.
///
/// Invariants (`docs/imap-design.md`, "Identities"):
/// * the LOGIN address is ALWAYS present and FIRST, even when the stored list
///   is empty or omits it (older builds may have stored an empty list);
/// * entries are de-duplicated case-insensitively, preserving first-seen order
///   and the FIRST display name seen for an address;
/// * an entry whose address fails the strict envelope-address validator (CR,
///   LF, NUL, `<`, `>`, whitespace, over-length, non-ASCII, bad `@` shape) is
///   DROPPED, never a panic or an error — a hostile or stale stored entry must
///   not break sending from the good identities.
///
/// If the LOGIN address itself fails validation the result is still non-empty:
/// the login is kept as the sole, first entry regardless, because every other
/// code path assumes `identities()[0]` is the account's primary address. A
/// login that cannot pass envelope validation is a setup-time problem the send
/// path surfaces later (the SMTP `MAIL FROM` rejects it); dropping it here
/// would instead yield an empty set and panic every caller that indexes `[0]`.
pub fn account_identities(login: &str, stored: &[Identity]) -> Vec<SendingIdentity> {
    let mut out: Vec<SendingIdentity> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // The login is first and unconditional.
    let login_norm = normalize_identity_address(login);
    seen.insert(login_norm);
    out.push(SendingIdentity {
        address: login.trim().to_string(),
        display_name: None,
    });

    for identity in stored {
        let normalized = normalize_identity_address(&identity.address);
        if seen.contains(&normalized) {
            // A duplicate (including the login re-listed): keep the first
            // occurrence, including its display name. If the FIRST occurrence
            // was the login (no display name) and a later duplicate carries
            // one, adopt that display name for the login entry so a configured
            // "Me <me@proton.me>" is not lost just because the login was
            // implicitly first.
            if let Some(existing) = out.iter_mut().find(|i| identity_normalized(i) == normalized) {
                if existing.display_name.is_none() {
                    existing.display_name = sanitize_display_name(identity.display_name.as_deref());
                }
            }
            continue;
        }
        // Drop an entry whose address is not a safe envelope address.
        if smtp::validate_envelope_address(identity.address.trim()).is_err() {
            continue;
        }
        seen.insert(normalized);
        out.push(SendingIdentity {
            address: identity.address.trim().to_string(),
            display_name: sanitize_display_name(identity.display_name.as_deref()),
        });
    }
    out
}

/// A display name is header data: drop it rather than carry CR/LF/NUL that
/// could inject a header. (A clean name is kept verbatim; `build_mime` applies
/// the same guard again at the point of writing bytes.) An empty or
/// whitespace-only name becomes `None`.
fn sanitize_display_name(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains(['\r', '\n', '\0']) {
        return None;
    }
    Some(name.to_string())
}

/// The reply-selection headers read from the ORIGINAL message, each the raw
/// header value (an address list) as it appeared. Any may be empty/missing.
#[derive(Clone, Debug, Default)]
pub struct ReplyHeaders {
    pub to: String,
    pub cc: String,
    pub delivered_to: String,
    pub x_original_to: String,
}

/// Choose which identity a reply/forward should be sent from.
///
/// `docs/imap-design.md` ("Identities"): check the original message's `To`,
/// then `Cc`, then `Delivered-To`, then `X-Original-To`; the first identity
/// whose normalized address matches an address in the EARLIEST-matching header
/// wins (header order beats identity order), and within one header the first
/// address that matches any identity wins. If nothing matches, the account's
/// FIRST identity (index 0, the login).
///
/// Returns the INDEX into `identities`. `identities` must be non-empty (it
/// always is — [`account_identities`] guarantees the login is index 0); an
/// empty slice defensively returns 0.
///
/// Address lists are parsed with the shared [`crate::correspondence::stored_addresses`]
/// parser, so display names, quoted commas, groups and angle brackets are
/// handled — never a hand split on commas. A malformed header yields no
/// addresses and simply does not match (no panic).
pub fn choose_reply_identity(identities: &[SendingIdentity], headers: &ReplyHeaders) -> usize {
    if identities.is_empty() {
        return 0;
    }
    // A fast lookup from normalized identity address -> its index. First-seen
    // wins so identity order is the within-header tiebreak.
    let mut index_of: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (i, identity) in identities.iter().enumerate() {
        index_of.entry(identity_normalized(identity)).or_insert(i);
    }

    for header_value in [
        &headers.to,
        &headers.cc,
        &headers.delivered_to,
        &headers.x_original_to,
    ] {
        if header_value.trim().is_empty() {
            continue;
        }
        // Parse the header's address list; the first address that matches ANY
        // identity decides, so iterate the header's addresses in order.
        for (_name, address) in crate::correspondence::stored_addresses(header_value) {
            let normalized = normalize_identity_address(&address);
            if let Some(&index) = index_of.get(&normalized) {
                return index;
            }
        }
        // This header had addresses but none matched: fall through to the next
        // header (header order beats identity order), per the design.
    }
    0
}

/// The Message-ID domain for a From address actually used by an IMAP send.
///
/// `docs/imap-design.md` ("Sending" > "Message-ID domain"): the domain is the
/// part after the LAST `@` of the From address, ASCII-lowercased, validated
/// against DNS-label syntax ([`policy::check_message_id_domain`]). An invalid
/// or missing domain is a typed [`ProviderError`] BEFORE any send — never a
/// silent fallback to `threestrands.local` for IMAP (that fallback would leak
/// the client name, which is the whole point of the decision).
///
/// IDN: a non-ASCII domain would need Punycode conversion. The `idna` crate is
/// present in `Cargo.lock` only transitively (via `url`), not as a direct
/// dependency of this crate, so using it would require adding a dependency —
/// which this run must not do. A non-ASCII domain is therefore REFUSED with a
/// plain error here, exactly as the brief's fallback branch requires. When
/// `idna` is made a direct dependency in a later run, convert here instead.
///
/// NOTE for a later run wiring `find_sent_copy`: that stub must compare
/// `<{operation}@{domain}>` using the domain of the draft's From — the SAME
/// value this computes — so it has to store or recompute the From's domain.
/// The outbox item keeps the whole draft in its payload (`OutboxItem.draft`),
/// and `Draft::from_address()` recovers the From address, so the domain is
/// recomputable from the outbox item alone; nothing extra needs persisting.
pub fn message_id_domain_for_address(from_address: &str) -> Result<String, ProviderError> {
    let address = from_address.trim();
    // Reject a non-ASCII address outright: SMTPUTF8 is not negotiated and IDN
    // conversion is unavailable without adding a dependency (see the doc note).
    if !address.is_ascii() {
        return Err(ProviderError::InvalidOperation(format!(
            "cannot derive a Message-ID domain from a non-ASCII address ({address:?}); \
             internationalised domains are not supported in this build"
        )));
    }
    let domain = address.rsplit_once('@').map(|(_, domain)| domain).ok_or_else(|| {
        ProviderError::InvalidOperation(format!(
            "From address {address:?} has no '@', so it has no Message-ID domain"
        ))
    })?;
    policy::check_message_id_domain(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(address: &str, display: Option<&str>) -> Identity {
        Identity {
            address: address.to_string(),
            display_name: display.map(ToString::to_string),
        }
    }

    fn si(address: &str, display: Option<&str>) -> SendingIdentity {
        SendingIdentity {
            address: address.to_string(),
            display_name: display.map(ToString::to_string),
        }
    }

    // ---- A. account_identities edge cases -----------------------------

    #[test]
    fn login_is_always_first_even_with_an_empty_stored_list() {
        let got = account_identities("me@proton.me", &[]);
        assert_eq!(got, vec![si("me@proton.me", None)]);
    }

    #[test]
    fn login_is_prepended_when_the_stored_list_omits_it() {
        let got = account_identities(
            "me@proton.me",
            &[id("work@proton.me", Some("Work"))],
        );
        assert_eq!(
            got,
            vec![si("me@proton.me", None), si("work@proton.me", Some("Work"))]
        );
    }

    #[test]
    fn duplicates_differing_only_in_case_collapse_keeping_order_and_first_display_name() {
        let got = account_identities(
            "Me@Proton.ME",
            &[
                id("me@proton.me", Some("First")),
                id("ME@PROTON.ME", Some("Second")),
                id("work@proton.me", None),
            ],
        );
        // The login is first (original case preserved for the header), and the
        // first display name seen for it ("First", from the re-listed login)
        // is adopted; the second duplicate is dropped. work is kept.
        assert_eq!(
            got,
            vec![
                si("Me@Proton.ME", Some("First")),
                si("work@proton.me", None)
            ]
        );
    }

    #[test]
    fn a_plus_tag_address_is_a_distinct_identity_not_the_base() {
        let got = account_identities(
            "me@proton.me",
            &[id("me+news@proton.me", Some("Newsletters"))],
        );
        assert_eq!(
            got,
            vec![
                si("me@proton.me", None),
                si("me+news@proton.me", Some("Newsletters"))
            ],
            "plus-tags are preserved: me+news is its own identity"
        );
    }

    #[test]
    fn a_hostile_stored_entry_is_dropped_not_a_panic_and_leaves_the_rest() {
        let got = account_identities(
            "me@proton.me",
            &[
                id("inject@proton.me\r\nRCPT TO:<evil@x>", Some("Nope")),
                id("ok@proton.me", Some("OK")),
                id("no-at-sign", None),
                id("spa ce@proton.me", None),
                id("<brackets@proton.me>", None),
            ],
        );
        // Only the login and the one valid entry survive.
        assert_eq!(
            got,
            vec![si("me@proton.me", None), si("ok@proton.me", Some("OK"))]
        );
    }

    #[test]
    fn a_hostile_display_name_is_dropped_but_the_address_is_kept() {
        let got = account_identities(
            "me@proton.me",
            &[id("work@proton.me", Some("Bad\r\nName: x"))],
        );
        assert_eq!(got[1], si("work@proton.me", None));
    }

    // ---- C. choose_reply_identity table -------------------------------

    fn two_identities() -> Vec<SendingIdentity> {
        vec![si("me@proton.me", Some("Me")), si("work@proton.me", Some("Work"))]
    }

    fn headers(to: &str, cc: &str, delivered_to: &str, x_original_to: &str) -> ReplyHeaders {
        ReplyHeaders {
            to: to.into(),
            cc: cc.into(),
            delivered_to: delivered_to.into(),
            x_original_to: x_original_to.into(),
        }
    }

    #[test]
    fn reply_identity_matches_in_to() {
        let ids = two_identities();
        let h = headers("work@proton.me, bob@x.com", "", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_matches_in_cc_only() {
        let ids = two_identities();
        let h = headers("bob@x.com", "work@proton.me", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_matches_in_delivered_to_only() {
        let ids = two_identities();
        let h = headers("bob@x.com", "carol@x.com", "work@proton.me", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_matches_in_x_original_to_only() {
        let ids = two_identities();
        let h = headers("bob@x.com", "", "", "work@proton.me");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_header_order_beats_identity_order() {
        // identity #2 (work) is in To; identity #1 (me) is in Cc. To is the
        // earlier header, so work wins even though me is the earlier identity.
        let ids = two_identities();
        let h = headers("work@proton.me", "me@proton.me", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_handles_display_names_and_case_differences() {
        let ids = two_identities();
        let h = headers("\"Work Account\" <WORK@Proton.ME>, bob@x.com", "", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_plus_address_does_not_match_the_base() {
        // The original was addressed to me+news@proton.me, which is NOT the
        // base me@proton.me identity, so there is no match -> fall back to 0.
        let ids = two_identities();
        let h = headers("me+news@proton.me", "", "", "");
        assert_eq!(
            choose_reply_identity(&ids, &h),
            0,
            "a plus-tag address must not match the base identity"
        );
    }

    #[test]
    fn reply_identity_falls_back_to_first_on_no_match() {
        let ids = two_identities();
        let h = headers("someone@else.com", "another@else.com", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 0);
    }

    #[test]
    fn reply_identity_first_matching_address_within_a_header_wins() {
        // Both identities appear in the SAME header; the FIRST address in the
        // header that matches an identity wins (work appears before me here).
        let ids = two_identities();
        let h = headers("bob@x.com, work@proton.me, me@proton.me", "", "", "");
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    #[test]
    fn reply_identity_empty_and_missing_headers_fall_back_to_first() {
        let ids = two_identities();
        assert_eq!(choose_reply_identity(&ids, &ReplyHeaders::default()), 0);
    }

    #[test]
    fn reply_identity_malformed_header_does_not_panic() {
        let ids = two_identities();
        // A header the strict parser rejects yields no addresses; fall back.
        let h = headers("not an address <<<@@@", "work@proton.me", "", "");
        // The malformed To matches nothing; Cc then matches work.
        assert_eq!(choose_reply_identity(&ids, &h), 1);
    }

    // ---- E. message_id_domain_for_address -----------------------------

    #[test]
    fn message_id_domain_is_the_part_after_the_last_at_lowercased() {
        assert_eq!(
            message_id_domain_for_address("Me@Proton.ME").unwrap(),
            "proton.me"
        );
        // A quoted local part with an '@' inside: the LAST '@' splits.
        assert_eq!(
            message_id_domain_for_address("weird@local@proton.me").unwrap(),
            "proton.me"
        );
    }

    #[test]
    fn message_id_domain_rejects_a_bad_or_missing_domain() {
        assert!(message_id_domain_for_address("no-at-sign").is_err());
        assert!(message_id_domain_for_address("me@").is_err());
        assert!(message_id_domain_for_address("me@-bad.me").is_err());
        assert!(message_id_domain_for_address("me@under_score.me").is_err());
        // Non-ASCII (IDN) is refused rather than silently converted.
        assert!(message_id_domain_for_address("me@pröton.me").is_err());
    }
}
