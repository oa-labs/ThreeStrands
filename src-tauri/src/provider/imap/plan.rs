//! The pure SYNC PLAN (Phase 2 Slice 5b-1).
//!
//! `docs/imap-design.md` ("What gets synced"). Slice 5a synced INBOX only.
//! This module turns the account's mailbox catalog + settings into an ordered
//! list of what to sync and how, so the sync round is driven by data rather
//! than an INBOX hard-code. It is PURE — no I/O — so the whole role matrix is
//! table-testable without a live server, exactly like `mailboxes.rs`.
//!
//! ## Resolution
//! Each selectable catalog mailbox resolves to a role by the design's
//! priority: **user override** (`mailbox_overrides`) > **RFC 6154 attribute**
//! (`special_use`) > the existing conservative **name match** in `mailboxes.rs`
//! (reused, never duplicated). INBOX is matched case-insensitively by name
//! (RFC 3501 reserves it; it carries no special-use role).
//!
//! ## What run 2 syncs
//! INBOX, Trash and Junk are `synced_now`. **Sent is classified but NOT synced
//! now** (`synced_now = false`) — it is run 3. Archive, user folders and
//! label-folder children are classified `synced_now = false` (Slice 5b-2).
//! `\All`, `\Flagged` (Proton "Starred" has `\Flagged` and NO role), `\Drafts`
//! and the label container itself are `NotSynced`.
//!
//! Discovery never persists `\Noselect` containers (see `mailboxes.rs`), so a
//! catalog row is always a selectable mailbox; the plan therefore does not need
//! to re-check `\Noselect`.

use super::mailboxes::MailboxRole;
use super::policy::SyncWindowClass;
use super::settings::ImapAccountSettings;

/// INBOX is a reserved name, not a special-use role.
pub const INBOX: &str = "INBOX";

/// The label kind a synced mailbox contributes to its messages. Run 2 produces
/// only system-role location labels; `lf:`/`folder:` kinds arrive in 5b-2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    /// A system location label (INBOX / SENT / SPAM / TRASH).
    SystemRole,
    /// A user folder (`folder:<name>`) — classified, not synced in run 2.
    UserFolder,
    /// A label-container child (`lf:<name>`) — classified, not synced in run 2.
    LabelFolder,
    /// Contributes no label (aggregates, Drafts, the label container).
    None,
}

/// One planned mailbox: which role it plays, its sync window class, the label
/// kind it contributes, and whether this run syncs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    /// The mailbox's exact (decoded) catalog name.
    pub mailbox: String,
    /// Its resolved role, or `None` for INBOX (reserved name) and for a
    /// mailbox that resolves to no system role.
    pub role: Option<MailboxRole>,
    /// Whether this is the reserved INBOX (role-less but synced).
    pub is_inbox: bool,
    /// The sync window class that governs its window + eviction semantics.
    pub window_class: SyncWindowClass,
    /// The label kind it contributes.
    pub label_kind: LabelKind,
    /// Whether THIS run syncs the mailbox.
    pub synced_now: bool,
}

impl PlanEntry {
    /// The location [`MailboxLabel`](super::labels::MailboxLabel) a message in
    /// this mailbox carries, if any. INBOX -> Inbox; a system role maps through
    /// [`MailboxLabel::for_role`]; everything else contributes none this run.
    pub fn location_label(&self) -> Option<super::labels::MailboxLabel> {
        if self.is_inbox {
            return Some(super::labels::MailboxLabel::Inbox);
        }
        self.role.and_then(super::labels::MailboxLabel::for_role)
    }
}

/// The whole plan: the ordered mailbox entries. Order places INBOX first, then
/// the other synced mailboxes (Trash, Junk this run), then the rest — so a
/// caller can walk `synced_now` entries in a stable, INBOX-first order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncPlan {
    pub entries: Vec<PlanEntry>,
}

impl SyncPlan {
    /// The entries this run actually syncs, INBOX first.
    pub fn synced(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries.iter().filter(|entry| entry.synced_now)
    }

    /// The entry for a mailbox by exact name, if present.
    pub fn entry_for(&self, mailbox: &str) -> Option<&PlanEntry> {
        self.entries.iter().find(|entry| entry.mailbox == mailbox)
    }

    /// Resolve a stored mailbox name to its location label (used by
    /// fetch_thread / fetch_message to label copies by role, not by name). A
    /// mailbox absent from the plan contributes no label.
    pub fn label_for_mailbox(&self, mailbox: &str) -> Option<super::labels::MailboxLabel> {
        self.entry_for(mailbox).and_then(PlanEntry::location_label)
    }
}

/// One selectable catalog mailbox the plan reasons over: its exact name and the
/// RFC 6154 `special_use` attribute discovery captured (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMailbox {
    pub name: String,
    pub special_use: Option<String>,
}

/// Resolve a mailbox's role, by the design priority: user override > RFC 6154
/// attribute > conservative name match. Returns `None` for INBOX (reserved
/// name) and for a mailbox nothing resolves.
fn resolve_role(
    mailbox: &CatalogMailbox,
    delimiter: Option<&str>,
    settings: &ImapAccountSettings,
) -> Option<MailboxRole> {
    // 1. User override: `mailbox_overrides` maps a role key -> mailbox name.
    for (key, name) in &settings.mailbox_overrides {
        if name == &mailbox.name {
            if let Some(role) = MailboxRole::from_key(key) {
                return Some(role);
            }
        }
    }
    // 2. RFC 6154 attribute.
    if let Some(attribute) = mailbox.special_use.as_deref() {
        if let Some(role) = MailboxRole::ALL
            .into_iter()
            .find(|role| role.attribute().eq_ignore_ascii_case(attribute))
        {
            return Some(role);
        }
    }
    // 3. Conservative name match (reuse mailboxes.rs's logic).
    MailboxRole::match_by_name(&mailbox.name, delimiter)
}

/// Whether a mailbox is the label container itself (not synced).
fn is_label_container(name: &str, settings: &ImapAccountSettings) -> bool {
    settings.label_container.as_deref() == Some(name)
}

/// Whether a mailbox is a CHILD of the label container (a label folder).
fn is_label_folder_child(
    name: &str,
    delimiter: Option<&str>,
    settings: &ImapAccountSettings,
) -> bool {
    let Some(container) = settings.label_container.as_deref() else {
        return false;
    };
    let delimiter = delimiter.filter(|d| !d.is_empty()).unwrap_or("/");
    name.starts_with(&format!("{container}{delimiter}"))
}

/// Build the sync plan from the account's selectable catalog + settings.
/// `delimiter` is the hierarchy delimiter (same for every mailbox on a server);
/// pass the one discovery recorded, defaulting to `/`.
pub fn build_plan(
    catalog: &[CatalogMailbox],
    delimiter: Option<&str>,
    settings: &ImapAccountSettings,
) -> SyncPlan {
    let mut inbox: Vec<PlanEntry> = Vec::new();
    let mut synced: Vec<PlanEntry> = Vec::new();
    let mut rest: Vec<PlanEntry> = Vec::new();

    for mailbox in catalog {
        let is_inbox = mailbox.name.eq_ignore_ascii_case(INBOX);

        if is_inbox {
            inbox.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: true,
                window_class: SyncWindowClass::Inbox,
                label_kind: LabelKind::SystemRole,
                synced_now: true,
            });
            continue;
        }

        // The label container itself and its children are special.
        if is_label_container(&mailbox.name, settings) {
            rest.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: false,
                window_class: SyncWindowClass::NotSynced,
                label_kind: LabelKind::None,
                synced_now: false,
            });
            continue;
        }
        if is_label_folder_child(&mailbox.name, delimiter, settings) {
            rest.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: false,
                window_class: SyncWindowClass::Folder,
                label_kind: LabelKind::LabelFolder,
                synced_now: false, // 5b-2
            });
            continue;
        }

        let role = resolve_role(mailbox, delimiter, settings);
        let (window_class, label_kind, synced_now, bucket_synced) = match role {
            // Aggregates and Drafts: never synced.
            Some(MailboxRole::All) | Some(MailboxRole::Drafts) => {
                (SyncWindowClass::NotSynced, LabelKind::None, false, false)
            }
            // Sent: classified now, synced in run 3.
            Some(MailboxRole::Sent) => {
                (SyncWindowClass::Sent, LabelKind::SystemRole, false, false)
            }
            // Trash and Junk: synced THIS run, Folder class.
            Some(MailboxRole::Trash) | Some(MailboxRole::Junk) => {
                (SyncWindowClass::Folder, LabelKind::SystemRole, true, true)
            }
            // Archive: classified now, synced in 5b-2.
            Some(MailboxRole::Archive) => {
                (SyncWindowClass::Folder, LabelKind::SystemRole, false, false)
            }
            // User folders: classified now, synced in 5b-2.
            None => (SyncWindowClass::Folder, LabelKind::UserFolder, false, false),
        };
        let entry = PlanEntry {
            mailbox: mailbox.name.clone(),
            role,
            is_inbox: false,
            window_class,
            label_kind,
            synced_now,
        };
        if bucket_synced {
            synced.push(entry);
        } else {
            rest.push(entry);
        }
    }

    // Stable order within the synced-non-INBOX bucket: Trash before Junk,
    // then by name, so the per-poll walk is deterministic.
    synced.sort_by(|a, b| {
        role_order(a.role)
            .cmp(&role_order(b.role))
            .then_with(|| a.mailbox.cmp(&b.mailbox))
    });

    let mut entries = Vec::with_capacity(inbox.len() + synced.len() + rest.len());
    entries.extend(inbox);
    entries.extend(synced);
    entries.extend(rest);
    SyncPlan { entries }
}

/// A stable ordering key for the synced-mailbox walk: Trash before Junk.
fn role_order(role: Option<MailboxRole>) -> u8 {
    match role {
        Some(MailboxRole::Trash) => 0,
        Some(MailboxRole::Junk) => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::settings::{LabelStorage, SecurityMode};
    use std::collections::BTreeMap;

    fn settings() -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@proton.me".into(),
            mailbox_overrides: BTreeMap::new(),
            archive_mailbox: None,
            label_storage: LabelStorage::Folders,
            label_container: Some("Labels".into()),
            identities: vec![],
            pinned_fingerprints: BTreeMap::new(),
            server_saves_sent: false,
        }
    }

    fn mb(name: &str, special_use: Option<&str>) -> CatalogMailbox {
        CatalogMailbox {
            name: name.into(),
            special_use: special_use.map(str::to_string),
        }
    }

    /// The Bridge `LIST` profile from docs/imap-design.md, exactly. Only INBOX,
    /// Trash and Junk are synced THIS run; Sent and Archive are classified but
    /// not synced; All Mail / Starred / Drafts / the label container are never
    /// synced. (`\Noselect` containers like `Folders`/`Labels` are not in the
    /// catalog — discovery never persists them — so here `Labels` stands in as
    /// a selectable container to prove it is classified NotSynced.)
    #[test]
    fn the_bridge_profile_plans_exactly() {
        let catalog = vec![
            mb("INBOX", None),
            mb("Trash", Some("\\Trash")),
            mb("Sent", Some("\\Sent")),
            mb("Drafts", Some("\\Drafts")),
            mb("All Mail", Some("\\All")),
            mb("Starred", Some("\\Flagged")), // \Flagged, NO MailboxRole
            mb("Archive", Some("\\Archive")),
            mb("Spam", Some("\\Junk")),
            mb("Labels", None),               // the label container
            mb("Labels/Work", None),          // a label folder child
            mb("Clients/Acme", None),         // a user folder
        ];
        let plan = build_plan(&catalog, Some("/"), &settings());

        let find = |name: &str| plan.entry_for(name).unwrap().clone();

        // INBOX: synced, Inbox class, first.
        assert_eq!(plan.entries[0].mailbox, "INBOX");
        assert!(find("INBOX").is_inbox);
        assert!(find("INBOX").synced_now);
        assert_eq!(find("INBOX").window_class, SyncWindowClass::Inbox);

        // Trash + Junk: synced THIS run, Folder class, SystemRole labels.
        for (name, role) in [("Trash", MailboxRole::Trash), ("Spam", MailboxRole::Junk)] {
            let e = find(name);
            assert!(e.synced_now, "{name} is synced this run");
            assert_eq!(e.role, Some(role));
            assert_eq!(e.window_class, SyncWindowClass::Folder);
            assert_eq!(e.label_kind, LabelKind::SystemRole);
        }

        // Sent: classified, Sent class, NOT synced this run (run 3).
        let sent = find("Sent");
        assert_eq!(sent.role, Some(MailboxRole::Sent));
        assert_eq!(sent.window_class, SyncWindowClass::Sent);
        assert!(!sent.synced_now, "Sent is run 3, not synced now");

        // Archive: classified, Folder class, NOT synced (5b-2).
        let archive = find("Archive");
        assert_eq!(archive.role, Some(MailboxRole::Archive));
        assert!(!archive.synced_now);

        // All Mail (\All) and Drafts: NotSynced, no label.
        for name in ["All Mail", "Drafts"] {
            let e = find(name);
            assert_eq!(e.window_class, SyncWindowClass::NotSynced);
            assert_eq!(e.label_kind, LabelKind::None);
            assert!(!e.synced_now);
        }

        // Starred (\Flagged, no MailboxRole): never synced — it is NOT a role,
        // and name-match does not claim it.
        let starred = find("Starred");
        assert!(!starred.synced_now, "Starred (\\Flagged) is never synced");
        assert_eq!(starred.role, None);

        // The label container: NotSynced, no label.
        let labels = find("Labels");
        assert!(!labels.synced_now);
        assert_eq!(labels.label_kind, LabelKind::None);

        // A label-folder child: classified LabelFolder, not synced (5b-2).
        let work = find("Labels/Work");
        assert_eq!(work.label_kind, LabelKind::LabelFolder);
        assert!(!work.synced_now);

        // A user folder: classified UserFolder, not synced (5b-2).
        let acme = find("Clients/Acme");
        assert_eq!(acme.label_kind, LabelKind::UserFolder);
        assert!(!acme.synced_now);

        // Exactly INBOX, Trash, Spam are synced, INBOX first.
        let synced: Vec<&str> = plan.synced().map(|e| e.mailbox.as_str()).collect();
        assert_eq!(synced, vec!["INBOX", "Trash", "Spam"]);
    }

    #[test]
    fn a_user_override_wins_over_attribute_and_name() {
        let mut settings = settings();
        // Override Trash to a mailbox that carries NO attribute and whose name
        // would not match.
        settings.mailbox_overrides.insert("trash".into(), "Bin42".into());
        let catalog = vec![mb("INBOX", None), mb("Bin42", None), mb("Trash", Some("\\Junk"))];
        let plan = build_plan(&catalog, Some("/"), &settings);
        // Bin42 resolves to Trash by override and is synced.
        let bin = plan.entry_for("Bin42").unwrap();
        assert_eq!(bin.role, Some(MailboxRole::Trash));
        assert!(bin.synced_now);
        // The mailbox literally named "Trash" carries \Junk, so it is Junk.
        assert_eq!(plan.entry_for("Trash").unwrap().role, Some(MailboxRole::Junk));
    }

    #[test]
    fn inbox_name_matches_case_insensitively() {
        let plan = build_plan(&[mb("inbox", None)], Some("/"), &settings());
        assert!(plan.entry_for("inbox").unwrap().is_inbox);
        assert!(plan.entry_for("inbox").unwrap().synced_now);
    }

    #[test]
    fn location_labels_follow_the_resolved_role() {
        use super::super::labels::MailboxLabel;
        let catalog = vec![
            mb("INBOX", None),
            mb("Trash", Some("\\Trash")),
            mb("Spam", Some("\\Junk")),
            mb("Sent", Some("\\Sent")),
            mb("Archive", Some("\\Archive")),
        ];
        let plan = build_plan(&catalog, Some("/"), &settings());
        assert_eq!(plan.label_for_mailbox("INBOX"), Some(MailboxLabel::Inbox));
        assert_eq!(plan.label_for_mailbox("Trash"), Some(MailboxLabel::Trash));
        assert_eq!(plan.label_for_mailbox("Spam"), Some(MailboxLabel::Junk));
        assert_eq!(plan.label_for_mailbox("Sent"), Some(MailboxLabel::Sent));
        // Archive contributes no system location label this slice.
        assert_eq!(plan.label_for_mailbox("Archive"), None);
        // A mailbox absent from the plan contributes nothing.
        assert_eq!(plan.label_for_mailbox("Nope"), None);
    }
}
