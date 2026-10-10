//! Mailbox discovery and RFC 6154 special-use mapping (Phase 2 Slice 3).
//!
//! `docs/imap-design.md` ("Mailbox discovery" / "Special-use mapping" /
//! "Data model"): an IMAP account's mailboxes are enumerated with a plain
//! `LIST "" "*"` (NOT `RETURN (SPECIAL-USE)` — the primary account does not
//! advertise `LIST-EXTENDED`, yet still returns the attributes on a plain
//! LIST), and the app's system mailboxes are mapped from what the server
//! returns:
//!
//! 1. **Prefer an explicit special-use attribute.** When a `LIST` entry
//!    carries `\Sent`, `\Archive`, `\Drafts`, `\Trash`, `\Junk` or `\All`,
//!    that attribute decides the role with no guessing and nothing to confirm.
//! 2. **Fall back to conservative, case-insensitive name matching** only for
//!    roles no attribute claimed. `Sent` / `Sent Mail` / `Sent Items`,
//!    `Archive`, `Drafts`, `Trash` / `Deleted` / `Deleted Items`,
//!    `Junk` / `Spam`. The match is delimiter-aware: `[Gmail]/Sent Mail`
//!    matches on its leaf. A name match is a GUESS, surfaced for the user to
//!    confirm or override on the mapping screen — never silently committed.
//! 3. **Skip `\Noselect` containers** — they are hierarchy nodes, not
//!    locations.
//!
//! **Provider-neutral (AGENTS.md invariant):** nothing here branches on a
//! sender, domain, host or provider brand. It acts only on RFC 6154
//! attributes and generic folder names, exactly like the email-rendering
//! policy. The one place `[Gmail]` appears is as a hierarchy PREFIX stripped
//! by the generic delimiter rule, not as a brand check — any container prefix
//! is handled the same way.
//!
//! This module is PURE: [`propose_mapping`] takes the `Vec<MailboxEntry>` that
//! [`ImapSession::list_mailboxes`](super::session::ImapSession) already
//! returns and produces a [`MailboxMapping`]. Persisting the catalog
//! (EXAMINE + `upsert_mailbox`) and writing the confirmed mapping into the
//! settings row live in the tauri commands; keeping the mapping logic pure is
//! what makes the whole role matrix testable without a live server.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::session::MailboxEntry;

/// One of the app's system mailbox roles that discovery maps a server mailbox
/// onto. These are the RFC 6154 special-use roles the design cares about;
/// `\All` is an aggregate view (not synced as a location) but is still mapped
/// so the sync slices can recognise and skip it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxRole {
    Sent,
    Archive,
    Drafts,
    Trash,
    Junk,
    /// The RFC 6154 `\All` aggregate mailbox ("All Mail"). A view of mail
    /// stored elsewhere, so it is never synced as a location; mapped so the
    /// sync engine can skip it rather than labelling everything `folder:All`.
    All,
}

impl MailboxRole {
    /// The RFC 6154 attribute spelling (as [`MailboxEntry::special_use`]
    /// captures it) that names this role.
    pub fn attribute(self) -> &'static str {
        match self {
            MailboxRole::Sent => "\\Sent",
            MailboxRole::Archive => "\\Archive",
            MailboxRole::Drafts => "\\Drafts",
            MailboxRole::Trash => "\\Trash",
            MailboxRole::Junk => "\\Junk",
            MailboxRole::All => "\\All",
        }
    }

    /// A stable snake_case key for the role, used as the `mailbox_overrides`
    /// map key when a confirmed mapping is written into the settings row.
    pub fn key(self) -> &'static str {
        match self {
            MailboxRole::Sent => "sent",
            MailboxRole::Archive => "archive",
            MailboxRole::Drafts => "drafts",
            MailboxRole::Trash => "trash",
            MailboxRole::Junk => "junk",
            MailboxRole::All => "all",
        }
    }

    /// Every role, in a stable order (also the mapping-screen display order).
    pub const ALL: [MailboxRole; 6] = [
        MailboxRole::Sent,
        MailboxRole::Archive,
        MailboxRole::Drafts,
        MailboxRole::Trash,
        MailboxRole::Junk,
        MailboxRole::All,
    ];

    /// The role named by a `mailbox_overrides` key, inverse of [`Self::key`].
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
    }

    /// Case-insensitive leaf names that match this role when no attribute is
    /// present. Kept conservative — common, unambiguous folder names only.
    fn name_candidates(self) -> &'static [&'static str] {
        match self {
            MailboxRole::Sent => &["sent", "sent mail", "sent items", "sent messages"],
            MailboxRole::Archive => &["archive", "archives"],
            MailboxRole::Drafts => &["drafts", "draft"],
            MailboxRole::Trash => &["trash", "deleted", "deleted items", "deleted messages", "bin"],
            MailboxRole::Junk => &["junk", "spam", "junk email", "junk e-mail", "bulk mail"],
            MailboxRole::All => &["all mail", "all"],
        }
    }
}

/// How a particular role's mailbox was chosen — which the mapping screen uses
/// to tell a certain attribute match from a name guess the user should check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingSource {
    /// From an explicit RFC 6154 `LIST` attribute — authoritative.
    SpecialUse,
    /// From a case-insensitive name match — a GUESS to confirm/override.
    NameMatch,
}

/// One role's proposed mailbox and how it was chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleProposal {
    pub role: MailboxRole,
    /// The decoded mailbox name proposed for this role.
    pub mailbox: String,
    pub source: MappingSource,
}

/// One selectable mailbox, surfaced to the mapping screen so the user can
/// override any role's choice or pick a mailbox for a role nothing matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredMailbox {
    pub name: String,
    pub delimiter: Option<String>,
    pub special_use: Option<String>,
}

/// The proposed role mapping plus the selectable-mailbox list the mapping
/// screen renders. Roles with no attribute AND no name match are simply
/// absent from `proposals` — the UI shows them as "choose a mailbox" (or, for
/// Archive, "Create `Archive`").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxMapping {
    /// One proposal per role that an attribute or a name matched, in
    /// [`MailboxRole::ALL`] order.
    pub proposals: Vec<RoleProposal>,
    /// Every selectable (non-`\Noselect`) mailbox, for override pickers.
    pub selectable: Vec<DiscoveredMailbox>,
    /// Label containers may be selectable folders or `\Noselect` hierarchy nodes.
    /// Listing them separately keeps hierarchy nodes out of system-role pickers.
    pub label_containers: Vec<DiscoveredMailbox>,
}

impl MailboxMapping {
    /// The mailbox proposed for `role`, if any.
    pub fn mailbox_for(&self, role: MailboxRole) -> Option<&str> {
        self.proposals
            .iter()
            .find(|proposal| proposal.role == role)
            .map(|proposal| proposal.mailbox.as_str())
    }
}

/// The decoded leaf of a hierarchical mailbox name, honouring the server's
/// delimiter. `[Gmail]/Sent Mail` with delimiter `/` has leaf `Sent Mail`;
/// `INBOX.Sent` with delimiter `.` has leaf `Sent`. A `None` or empty
/// delimiter leaves the name whole. This is the one, generic place hierarchy
/// is stripped — no container name is special-cased.
fn leaf<'a>(name: &'a str, delimiter: Option<&str>) -> &'a str {
    match delimiter {
        Some(delimiter) if !delimiter.is_empty() => {
            name.rsplit(delimiter).next().unwrap_or(name)
        }
        _ => name,
    }
}

/// Produce the proposed role → mailbox mapping from a plain-`LIST` result.
///
/// Attribute matches win and are taken in a first-pass over all entries; name
/// matching then fills only the roles still unclaimed, and only from
/// selectable mailboxes whose leaf matches a conservative candidate. A
/// selectable mailbox already claimed by an attribute is never also used as a
/// name-match for a different role. The input order is preserved for the
/// selectable list so the UI is stable.
pub fn propose_mapping(entries: &[MailboxEntry]) -> MailboxMapping {
    // Selectable mailboxes only — `\Noselect` containers are not locations.
    let selectable: Vec<&MailboxEntry> = entries.iter().filter(|entry| !entry.no_select).collect();

    let mut by_role: BTreeMap<MailboxRole, RoleProposal> = BTreeMap::new();

    // Pass 1: explicit special-use attributes. Authoritative; first wins if a
    // server somehow attributes two mailboxes the same role (not expected).
    for entry in &selectable {
        if let Some(attribute) = entry.special_use.as_deref() {
            if let Some(role) = MailboxRole::ALL
                .into_iter()
                .find(|role| role.attribute().eq_ignore_ascii_case(attribute))
            {
                by_role.entry(role).or_insert_with(|| RoleProposal {
                    role,
                    mailbox: entry.name.clone(),
                    source: MappingSource::SpecialUse,
                });
            }
        }
    }

    // Each mailbox can supply at most one proposed role, including name guesses.
    let mut claimed: std::collections::BTreeSet<String> =
        by_role.values().map(|proposal| proposal.mailbox.clone()).collect();

    // Pass 2: name matching for roles no attribute claimed.
    for role in MailboxRole::ALL {
        if by_role.contains_key(&role) {
            continue;
        }
        if let Some(entry) = selectable.iter().find(|entry| {
            !claimed.contains(&entry.name)
                && role.name_candidates().iter().any(|candidate| {
                    leaf(&entry.name, entry.delimiter.as_deref())
                        .eq_ignore_ascii_case(candidate)
                })
        }) {
            claimed.insert(entry.name.clone());
            by_role.insert(
                role,
                RoleProposal {
                    role,
                    mailbox: entry.name.clone(),
                    source: MappingSource::NameMatch,
                },
            );
        }
    }

    // Emit proposals in the stable role order.
    let proposals = MailboxRole::ALL
        .into_iter()
        .filter_map(|role| by_role.remove(&role))
        .collect();

    let selectable = selectable
        .into_iter()
        .map(|entry| DiscoveredMailbox {
            name: entry.name.clone(),
            delimiter: entry.delimiter.clone(),
            special_use: entry.special_use.clone(),
        })
        .collect();

    let label_containers = entries
        .iter()
        .map(|entry| DiscoveredMailbox {
            name: entry.name.clone(),
            delimiter: entry.delimiter.clone(),
            special_use: entry.special_use.clone(),
        })
        .collect();

    MailboxMapping { proposals, selectable, label_containers }
}

/// Discover an account's mailboxes end to end over an authenticated session:
/// `LIST "" "*"`, EXAMINE each selectable mailbox read-only for its UID
/// counters, upsert the catalog into `imap_mailboxes`, and
/// return the proposed role mapping for the user to confirm.
///
/// EXAMINE (not SELECT) is deliberate: discovery is read-only and must not set
/// `\Recent` semantics or otherwise mutate server state (`docs/imap-design.md`,
/// "Mailbox discovery"). A mailbox that cannot be EXAMINEd (it vanished
/// between the LIST and the EXAMINE, or is momentarily busy) is skipped rather
/// than failing the whole discovery — its catalog row is simply not refreshed
/// this round. EXAMINE's PERMANENTFLAGS describe this read-only selection,
/// not the mailbox's write capabilities. Those remain unknown until a writable
/// SELECT supplies them; discovery preserves capabilities already learned there.
pub async fn discover_and_persist(
    session: &mut dyn super::session::ImapSession,
    store: &super::ImapStateStore,
) -> Result<MailboxMapping, crate::provider::ProviderError> {
    let entries = session.list_mailboxes().await?;
    for entry in &entries {
        if entry.no_select {
            continue;
        }
        // EXAMINE is read-only; a per-mailbox failure must not sink discovery.
        let status = match session.examine(&entry.name).await {
            Ok(status) => status,
            Err(_) => continue,
        };
        let row = super::ImapMailbox {
            name: entry.name.clone(),
            delimiter: entry.delimiter.clone(),
            special_use: entry.special_use.clone(),
            // EXAMINE reports UID counters as unsigned; store as i64 (the
            // table's type). A server that omits them yields 0, which the
            // next real sync overwrites.
            uidvalidity: status.uid_validity.unwrap_or(0) as i64,
            uidnext: status.uid_next.unwrap_or(0) as i64,
            highestmodseq: None, // CONDSTORE is a later slice.
            permanent_flags_json: None,
            permanent_keywords: None,
        };
        store
            .upsert_mailbox(&row)
            .map_err(|e| crate::provider::ProviderError::InvalidOperation(format!(
                "failed to persist mailbox catalog row for {}: {e}",
                entry.name
            )))?;
    }
    Ok(propose_mapping(&entries))
}

/// Create `mailbox`, tolerating a server that answers the create itself with
/// `TRYCREATE` or that already has the mailbox. This is the one place Slice 3
/// creates a mailbox — the confirmed-mapping commit's "Create `Archive`"
/// choice — so the design's `TRYCREATE` create-then-retry rule is wired HERE
/// and nowhere speculative:
///
/// * A plain `CREATE` that succeeds is done.
/// * `TRYCREATE` surfaces (through Slice 1's error map) as a transient
///   transport error whose message names the code; we treat that as "the
///   mailbox is missing, which is exactly what we are creating" and retry the
///   CREATE once.
/// * An "already exists" rejection is success — creating an existing mailbox
///   is idempotent for our purposes.
pub async fn create_mailbox_tolerant(
    session: &mut dyn super::session::ImapSession,
    mailbox: &str,
) -> Result<(), crate::provider::ProviderError> {
    match session.create_mailbox(mailbox).await {
        Ok(()) => Ok(()),
        Err(error) if mentions_already_exists(&error) => Ok(()),
        Err(error) if mentions_trycreate(&error) => {
            // Create-then-retry once: the server told us the target is absent.
            match session.create_mailbox(mailbox).await {
                Ok(()) => Ok(()),
                Err(retry) if mentions_already_exists(&retry) => Ok(()),
                Err(retry) => Err(retry),
            }
        }
        Err(error) => Err(error),
    }
}

fn mentions_trycreate(error: &crate::provider::ProviderError) -> bool {
    error_message(error).to_ascii_uppercase().contains("TRYCREATE")
}

fn mentions_already_exists(error: &crate::provider::ProviderError) -> bool {
    let message = error_message(error).to_ascii_lowercase();
    message.contains("already exists") || message.contains("alreadyexists")
}

fn error_message(error: &crate::provider::ProviderError) -> &str {
    use crate::provider::ProviderError::*;
    match error {
        ReauthenticationRequired(m)
        | TransientTransport(m)
        | PermanentClientRejection(m)
        | InvalidOperation(m)
        | RetryableServer(m)
        | Other(m)
        | Authentication(m) => m.as_str(),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, delimiter: &str, special_use: Option<&str>, no_select: bool) -> MailboxEntry {
        MailboxEntry {
            name: name.to_string(),
            delimiter: Some(delimiter.to_string()),
            no_select,
            special_use: special_use.map(str::to_string),
        }
    }

    /// The primary account's exact `LIST` (docs/imap-design.md): every system
    /// mailbox carries an attribute on a plain LIST, so mapping is pure
    /// attribute work and NOTHING falls back to a name guess.
    #[test]
    fn primary_account_maps_entirely_from_attributes() {
        let entries = vec![
            entry("Trash", "/", Some("\\Trash"), false),
            entry("Sent", "/", Some("\\Sent"), false),
            entry("Drafts", "/", Some("\\Drafts"), false),
            entry("All Mail", "/", Some("\\All"), false),
            entry("Folders", "/", None, true), // \Noselect container
            entry("Labels", "/", None, true),  // \Noselect container
            entry("INBOX", "/", None, false),
            entry("Starred", "/", Some("\\Flagged"), false), // not a mapped role
            entry("Archive", "/", Some("\\Archive"), false),
            entry("Spam", "/", Some("\\Junk"), false),
        ];
        let mapping = propose_mapping(&entries);

        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("Sent"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Archive), Some("Archive"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Drafts), Some("Drafts"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Trash), Some("Trash"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Junk), Some("Spam"));
        assert_eq!(mapping.mailbox_for(MailboxRole::All), Some("All Mail"));
        // Every proposal is attribute-sourced — nothing to confirm.
        assert!(mapping
            .proposals
            .iter()
            .all(|proposal| proposal.source == MappingSource::SpecialUse));
        // \Noselect containers never appear as selectable mailboxes.
        assert!(mapping.selectable.iter().all(|mailbox| mailbox.name != "Folders"));
        assert!(mapping.selectable.iter().all(|mailbox| mailbox.name != "Labels"));
        // INBOX is selectable but maps to no special-use role.
        assert!(mapping.selectable.iter().any(|mailbox| mailbox.name == "INBOX"));
    }

    /// A server that returns no attributes at all: every role falls back to a
    /// conservative, case-insensitive name match, flagged as a guess.
    #[test]
    fn name_matching_fills_roles_when_no_attributes_are_returned() {
        let entries = vec![
            entry("INBOX", "/", None, false),
            entry("Sent Items", "/", None, false),
            entry("Deleted Items", "/", None, false),
            entry("Junk Email", "/", None, false),
            entry("Archive", "/", None, false),
            entry("DRAFTS", "/", None, false), // case-insensitive
        ];
        let mapping = propose_mapping(&entries);

        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("Sent Items"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Trash), Some("Deleted Items"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Junk), Some("Junk Email"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Archive), Some("Archive"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Drafts), Some("DRAFTS"));
        assert!(mapping
            .proposals
            .iter()
            .all(|proposal| proposal.source == MappingSource::NameMatch));
    }

    /// An attribute wins over a name that would match a *different* role, and
    /// the name-matched mailbox is not stolen by the attribute's role.
    #[test]
    fn attributes_take_precedence_over_name_matches() {
        let entries = vec![
            // Attribute says this odd name is Sent.
            entry("Outbound", "/", Some("\\Sent"), false),
            // A folder literally named "Sent" exists but Sent is already
            // claimed by attribute — it must NOT override the attribute, and
            // must not be reused for another role.
            entry("Sent", "/", None, false),
        ];
        let mapping = propose_mapping(&entries);
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("Outbound"));
        assert_eq!(
            mapping
                .proposals
                .iter()
                .find(|p| p.role == MailboxRole::Sent)
                .unwrap()
                .source,
            MappingSource::SpecialUse
        );
    }

    /// Delimiter-aware matching: a hierarchical name matches on its leaf, so
    /// `[Gmail]/Sent Mail` and `INBOX.Trash` map correctly — with no brand
    /// special-case, just the generic delimiter rule.
    #[test]
    fn name_matching_is_delimiter_aware_on_the_leaf() {
        let slash = vec![
            entry("[Gmail]/Sent Mail", "/", None, false),
            entry("[Gmail]/All Mail", "/", None, false),
            entry("[Gmail]/Trash", "/", None, false),
        ];
        let mapping = propose_mapping(&slash);
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("[Gmail]/Sent Mail"));
        assert_eq!(mapping.mailbox_for(MailboxRole::All), Some("[Gmail]/All Mail"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Trash), Some("[Gmail]/Trash"));

        // A dot-delimited hierarchy matches the same way.
        let dot = vec![entry("INBOX.Sent", ".", None, false)];
        let mapping = propose_mapping(&dot);
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("INBOX.Sent"));
    }

    /// `\Noselect` containers never become a mapped location, even when their
    /// name would match a role.
    #[test]
    fn noselect_containers_are_skipped_even_when_the_name_matches() {
        let entries = vec![
            entry("Archive", "/", None, true), // a \Noselect "Archive" container
            entry("INBOX", "/", None, false),
        ];
        let mapping = propose_mapping(&entries);
        assert_eq!(mapping.mailbox_for(MailboxRole::Archive), None);
        assert!(mapping.selectable.iter().all(|mailbox| mailbox.name != "Archive"));
        assert!(mapping.label_containers.iter().any(|mailbox| mailbox.name == "Archive"));
        assert!(mapping.label_containers.iter().any(|mailbox| mailbox.name == "INBOX"));
    }

    #[test]
    fn all_mail_name_fallback_is_an_aggregate_never_an_archive_destination() {
        for (name, delimiter) in [("All Mail", "/"), ("INBOX.All Mail", ".")] {
            let mapping = propose_mapping(&[entry(name, delimiter, None, false)]);
            assert_eq!(mapping.mailbox_for(MailboxRole::All), Some(name));
            assert_eq!(mapping.mailbox_for(MailboxRole::Archive), None);
        }
        let mapping = propose_mapping(&[
            entry("Archive", "/", None, false),
            entry("Views/All Mail", "/", None, false),
        ]);
        assert_eq!(mapping.mailbox_for(MailboxRole::Archive), Some("Archive"));
        assert_eq!(
            mapping.mailbox_for(MailboxRole::All),
            Some("Views/All Mail")
        );
        let names: std::collections::BTreeSet<_> = mapping
            .proposals
            .iter()
            .map(|proposal| &proposal.mailbox)
            .collect();
        assert_eq!(names.len(), mapping.proposals.len());
    }

    #[test]
    fn attribute_mapping_preserves_exact_mailbox_and_container_names() {
        let mapping = propose_mapping(&[
            entry(" Sent ", "/", Some("\\Sent"), false),
            entry(" Labels ", "/", None, true),
        ]);
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some(" Sent "));
        assert_eq!(mapping.selectable[0].name, " Sent ");
        assert_eq!(mapping.label_containers[1].name, " Labels ");
    }

    /// A role nothing matches is simply absent — the UI then offers "choose a
    /// mailbox" (and, for Archive, "Create `Archive`"). Discovery proposes
    /// nothing it cannot justify.
    #[test]
    fn an_unmatched_role_is_absent_rather_than_guessed() {
        let entries = vec![
            entry("INBOX", "/", None, false),
            entry("Work", "/", None, false),
            entry("Personal", "/", None, false),
        ];
        let mapping = propose_mapping(&entries);
        for role in MailboxRole::ALL {
            assert_eq!(mapping.mailbox_for(role), None, "{role:?} should be unmatched");
        }
        // But the user's own folders are still selectable for an override.
        assert_eq!(mapping.selectable.len(), 3);
    }

    /// The first name candidate found wins, but an ambiguous pair (both
    /// "Spam" and "Junk" present) still resolves to exactly one Junk mailbox
    /// rather than erroring or double-claiming.
    #[test]
    fn ambiguous_names_resolve_to_a_single_proposal() {
        let entries = vec![
            entry("Junk", "/", None, false),
            entry("Spam", "/", None, false),
        ];
        let mapping = propose_mapping(&entries);
        // Exactly one Junk proposal, and it is one of the two.
        let junk: Vec<_> = mapping
            .proposals
            .iter()
            .filter(|p| p.role == MailboxRole::Junk)
            .collect();
        assert_eq!(junk.len(), 1);
        assert!(matches!(junk[0].mailbox.as_str(), "Junk" | "Spam"));
    }

    #[test]
    fn role_keys_round_trip() {
        for role in MailboxRole::ALL {
            assert_eq!(MailboxRole::from_key(role.key()), Some(role));
        }
        assert_eq!(MailboxRole::from_key("nope"), None);
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::provider::imap::session::{ImapSession, MailboxStatus};
    use crate::provider::imap::ImapStateStore;
    use crate::provider::ProviderError;
    use async_trait::async_trait;
    use async_imap::types::Fetch;
    use std::sync::Arc;

    /// A scripted `ImapSession` with no network: a LIST result, a per-mailbox
    /// EXAMINE outcome, and a queue of CREATE outcomes to drive the TRYCREATE
    /// path. Only the methods discovery and create use are meaningful; the
    /// rest are unreachable in these tests.
    ///
    /// EXAMINE outcomes are stored as `Result<MailboxStatus, ()>` because
    /// `ProviderError` is not `Clone`; a scripted failure becomes a transient
    /// transport error on read, which is all discovery's skip path inspects.
    struct FakeSession {
        list: Vec<MailboxEntry>,
        examine: std::collections::BTreeMap<String, Result<MailboxStatus, ()>>,
        creates: std::cell::RefCell<Vec<Result<(), ProviderError>>>,
        create_calls: std::cell::RefCell<Vec<String>>,
    }

    fn status(uid_validity: u32, uid_next: u32, flags: &[&str], keywords: bool) -> MailboxStatus {
        MailboxStatus {
            exists: 0,
            uid_next: Some(uid_next),
            uid_validity: Some(uid_validity),
            permanent_keywords: keywords,
            permanent_flags: flags.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[async_trait]
    impl ImapSession for FakeSession {
        async fn select(&mut self, _: &str) -> Result<MailboxStatus, ProviderError> {
            unreachable!("discovery uses EXAMINE, never SELECT")
        }
        async fn examine(&mut self, mailbox: &str) -> Result<MailboxStatus, ProviderError> {
            match self.examine.get(mailbox) {
                Some(Ok(status)) => Ok(status.clone()),
                Some(Err(())) => Err(ProviderError::TransientTransport("scripted examine failure".into())),
                None => Err(ProviderError::InvalidOperation("no script".into())),
            }
        }
        async fn uid_search(&mut self, _: &str) -> Result<Vec<u32>, ProviderError> {
            unreachable!()
        }
        async fn uid_fetch(&mut self, _: &str, _: &str) -> Result<Vec<Fetch>, ProviderError> {
            unreachable!()
        }
        async fn capabilities(&mut self) -> Result<Vec<String>, ProviderError> {
            unreachable!()
        }
        async fn list_mailboxes(&mut self) -> Result<Vec<MailboxEntry>, ProviderError> {
            Ok(self.list.clone())
        }
        async fn run_command_capture_code(&mut self, _: &str) -> Result<Option<String>, ProviderError> {
            unreachable!()
        }
        async fn noop(&mut self) -> Result<(), ProviderError> {
            unreachable!()
        }
        async fn create_mailbox(&mut self, mailbox: &str) -> Result<(), ProviderError> {
            self.create_calls.borrow_mut().push(mailbox.to_string());
            self.creates
                .borrow_mut()
                .drain(..1)
                .next()
                .unwrap_or_else(|| Err(ProviderError::InvalidOperation("no create script".into())))
        }
        async fn logout(&mut self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn entry(name: &str, special_use: Option<&str>, no_select: bool) -> MailboxEntry {
        MailboxEntry {
            name: name.to_string(),
            delimiter: Some("/".into()),
            no_select,
            special_use: special_use.map(str::to_string),
        }
    }

    fn store() -> ImapStateStore {
        ImapStateStore::new(Arc::new(crate::db::Database::open_memory()), "me@proton.me")
    }

    /// Discovery EXAMINEs each selectable mailbox, persists the catalog with
    /// its counters and unknown write capabilities, skips `\Noselect` containers, and
    /// returns the attribute-sourced mapping.
    #[tokio::test]
    async fn discovery_persists_the_catalog_and_proposes_the_mapping() {
        let list = vec![
            entry("INBOX", None, false),
            entry("Sent", Some("\\Sent"), false),
            entry("Archive", Some("\\Archive"), false),
            entry("Labels", None, true), // \Noselect container
        ];
        let mut examine = std::collections::BTreeMap::new();
        examine.insert("INBOX".to_string(), Ok(status(95479608, 979, &["\\Seen", "$Forwarded"], false)));
        examine.insert("Sent".to_string(), Ok(status(1, 10, &["\\Seen"], false)));
        examine.insert("Archive".to_string(), Ok(status(2, 20, &["\\Seen"], false)));
        let mut session = FakeSession {
            list,
            examine,
            creates: Default::default(),
            create_calls: Default::default(),
        };
        let store = store();

        let mapping = discover_and_persist(&mut session, &store).await.unwrap();
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("Sent"));
        assert_eq!(mapping.mailbox_for(MailboxRole::Archive), Some("Archive"));

        // The catalog persisted exactly the three selectable mailboxes, with
        // the EXAMINE counters, but not read-only permissions — never the
        // \Noselect container.
        let rows = store.mailboxes().unwrap();
        let names: Vec<_> = rows.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Archive", "INBOX", "Sent"]); // ordered by name
        let inbox = rows.iter().find(|m| m.name == "INBOX").unwrap();
        assert_eq!(inbox.uidvalidity, 95479608);
        assert_eq!(inbox.uidnext, 979);
        assert_eq!(inbox.permanent_flags_json, None);
        assert_eq!(inbox.permanent_keywords, None);
    }

    /// A mailbox that fails EXAMINE (vanished or busy) is skipped, not fatal:
    /// discovery still persists the others and returns a mapping.
    #[tokio::test]
    async fn a_single_examine_failure_does_not_sink_discovery() {
        let list = vec![entry("INBOX", None, false), entry("Sent", Some("\\Sent"), false)];
        let mut examine = std::collections::BTreeMap::new();
        examine.insert("INBOX".to_string(), Ok(status(1, 2, &["\\Seen"], false)));
        examine.insert("Sent".to_string(), Err(()));
        let mut session = FakeSession {
            list,
            examine,
            creates: Default::default(),
            create_calls: Default::default(),
        };
        let store = store();
        let mapping = discover_and_persist(&mut session, &store).await.unwrap();
        // The mapping still proposes Sent from its attribute (LIST), even
        // though its catalog row could not be refreshed this round.
        assert_eq!(mapping.mailbox_for(MailboxRole::Sent), Some("Sent"));
        let names: Vec<_> = store.mailboxes().unwrap().iter().map(|m| m.name.clone()).collect();
        assert_eq!(names, vec!["INBOX".to_string()]);
    }

    #[tokio::test]
    async fn read_only_discovery_preserves_known_write_capabilities() {
        for keywords in [false, true] {
            let store = store();
            let known = super::super::ImapMailbox {
                name: "INBOX".into(),
                delimiter: Some("/".into()),
                special_use: None,
                uidvalidity: 1,
                uidnext: 2,
                highestmodseq: None,
                permanent_flags_json: Some(
                    if keywords {
                        r#"["\\Seen","\\*"]"#
                    } else {
                        r#"["\\Seen"]"#
                    }
                    .into(),
                ),
                permanent_keywords: Some(keywords),
            };
            store.upsert_mailbox(&known).unwrap();
            let mut session = FakeSession {
                list: vec![entry("INBOX", None, false)],
                examine: [("INBOX".into(), Ok(status(1, 9, &[], false)))].into(),
                creates: Default::default(),
                create_calls: Default::default(),
            };
            discover_and_persist(&mut session, &store).await.unwrap();
            let rows = store.mailboxes().unwrap();
            assert_eq!(rows[0].uidnext, 9);
            assert_eq!(rows[0].permanent_flags_json, known.permanent_flags_json);
            assert_eq!(rows[0].permanent_keywords, Some(keywords));
        }
    }

    /// A plain CREATE that succeeds creates exactly once.
    #[tokio::test]
    async fn create_tolerant_succeeds_on_a_plain_create() {
        let mut session = FakeSession {
            list: vec![],
            examine: Default::default(),
            creates: std::cell::RefCell::new(vec![Ok(())]),
            create_calls: Default::default(),
        };
        create_mailbox_tolerant(&mut session, "Archive").await.unwrap();
        assert_eq!(session.create_calls.borrow().len(), 1);
    }

    /// A `TRYCREATE` response triggers exactly one retry; the retry succeeds.
    #[tokio::test]
    async fn create_tolerant_retries_once_on_trycreate() {
        let mut session = FakeSession {
            list: vec![],
            examine: Default::default(),
            creates: std::cell::RefCell::new(vec![
                Err(ProviderError::TransientTransport(
                    "mailbox does not exist (TRYCREATE); create-and-retry lands in Slice 3".into(),
                )),
                Ok(()),
            ]),
            create_calls: Default::default(),
        };
        create_mailbox_tolerant(&mut session, "Archive").await.unwrap();
        assert_eq!(session.create_calls.borrow().len(), 2, "exactly one retry");
    }

    /// An "already exists" rejection is treated as success (idempotent), with
    /// no retry.
    #[tokio::test]
    async fn create_tolerant_treats_already_exists_as_success() {
        let mut session = FakeSession {
            list: vec![],
            examine: Default::default(),
            creates: std::cell::RefCell::new(vec![Err(ProviderError::InvalidOperation(
                "Mailbox already exists".into(),
            ))]),
            create_calls: Default::default(),
        };
        create_mailbox_tolerant(&mut session, "Archive").await.unwrap();
        assert_eq!(session.create_calls.borrow().len(), 1, "no retry for already-exists");
    }

    /// A hard CREATE failure that is neither TRYCREATE nor already-exists
    /// surfaces, with no retry.
    #[tokio::test]
    async fn create_tolerant_surfaces_a_hard_failure() {
        let mut session = FakeSession {
            list: vec![],
            examine: Default::default(),
            creates: std::cell::RefCell::new(vec![Err(ProviderError::PermanentClientRejection(
                "over quota".into(),
            ))]),
            create_calls: Default::default(),
        };
        let err = create_mailbox_tolerant(&mut session, "Archive").await.unwrap_err();
        assert!(matches!(err, ProviderError::PermanentClientRejection(_)));
        assert_eq!(session.create_calls.borrow().len(), 1, "no retry on a hard failure");
    }

    // Optional live coverage of mailbox discovery against the Dovecot test
    // container. GATED on `DOVECOT_TEST_FP` so the default `cargo test` never
    // needs docker, exactly like Slice 1/2's live tests:
    //   scripts/dovecot-test-server.sh up
    //   DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
    //     cargo test --lib provider::imap::mailboxes::discovery_tests::live_ -- --nocapture
    // Proves the real path end to end: connect with the pinned cert, LIST,
    // EXAMINE each selectable mailbox, persist the catalog, and map the
    // special-use roles. The harness advertises special-use attributes for
    // its system mailboxes, so this covers the attribute path; the pure unit
    // tests above cover the NAME-match fallback the primary account needs.
    #[tokio::test]
    async fn live_discovery_persists_and_maps_against_dovecot() {
        use crate::provider::imap::connection::{ConnectionConfig, ImapConnectionManager, ConnectionRole, TlsMode};
        let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return;
        };
        let fp = crate::provider::imap::tls::parse_sha256_fingerprint(&fp_str)
            .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
        let cfg = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        };
        let mgr = ImapConnectionManager::new(cfg);
        let (mut session, _lease) = mgr
            .connect_leased(ConnectionRole::Command, "test@threestrands.test", "testpassword")
            .await
            .expect("live connect should succeed");
        let store = store();
        let mapping = discover_and_persist(&mut session, &store)
            .await
            .expect("discovery should succeed against the live container");
        // At least INBOX is in the catalog, and the mapping is non-empty for a
        // standard Dovecot layout (Sent/Drafts/Trash/Junk).
        let rows = store.mailboxes().unwrap();
        assert!(rows.iter().any(|m| m.name.eq_ignore_ascii_case("INBOX")));
        assert!(
            !mapping.proposals.is_empty(),
            "the harness should map at least one special-use role, got {mapping:?}"
        );
        let _ = session.logout().await;
    }
}
