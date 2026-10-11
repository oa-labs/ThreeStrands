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
//! ## What this run syncs
//! INBOX, Sent, Trash, Junk and Archive are `synced_now` (system roles).
//! Archive is Folder-class (an evicting 2,000 window) and contributes NO
//! location label (archived = not in INBOX, so a system label would be wrong).
//! Sent's window is a non-evicting acquisition bound filled in chunked
//! background rounds after INBOX.
//!
//! Slice 5b-2 run B ALSO syncs the account's own mail folders, Folder-class:
//! **user folders** (a selectable mailbox with no resolved role, not INBOX, not
//! an aggregate/Drafts, not the label container or its children) contribute a
//! dynamic `folder:<full catalog name>` label; and, when the account's
//! `label_storage` is [`LabelStorage::Folders`](super::settings::LabelStorage),
//! **label-folder children** (selectable children of the configured label
//! container) contribute a dynamic `lf:<name relative to the container>` label.
//! Under any other `label_storage` the label container is not in play, so a
//! "child of the container" is treated as an ordinary user folder (there is no
//! container to be relative to). `\All`, `\Flagged` (Proton "Starred" has
//! `\Flagged` and NO role), `\Drafts` and the label container itself are
//! `NotSynced` and contribute no label.
//!
//! Discovery never persists `\Noselect` containers (see `mailboxes.rs`), so a
//! catalog row is always a selectable mailbox; the plan therefore does not need
//! to re-check `\Noselect`.

use super::mailboxes::MailboxRole;
use super::policy::SyncWindowClass;
use super::settings::ImapAccountSettings;

/// INBOX is a reserved name, not a special-use role.
pub const INBOX: &str = "INBOX";

/// The label kind a synced mailbox contributes to its messages: a system
/// location label (INBOX/SENT/SPAM/TRASH), a `folder:` user-folder label, an
/// `lf:` label-folder label, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    /// A system location label (INBOX / SENT / SPAM / TRASH).
    SystemRole,
    /// A user folder: contributes `folder:<full catalog name>`.
    UserFolder,
    /// A label-container child (label_storage == Folders): contributes
    /// `lf:<name relative to the container>`.
    LabelFolder,
    /// Contributes no label (aggregates, Drafts, Archive, the label container).
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
    /// The OWNED location label id a message stored in this mailbox carries, if
    /// any — computed once at plan-build time so `fetch` needs no delimiter or
    /// container knowledge. `Some("INBOX")`/`Some("SENT")`/… for system roles,
    /// `Some("folder:<name>")` for a user folder, `Some("lf:<rel>")` for a
    /// label-folder child, `None` for a mailbox that contributes no location
    /// label (Archive, Drafts, aggregates, the label container).
    pub location_label_id: Option<String>,
    /// Whether THIS run syncs the mailbox.
    pub synced_now: bool,
}

impl PlanEntry {
    /// The OWNED location label ids a message stored in this mailbox carries.
    /// Zero or more label-id strings (a message in several mailboxes unions the
    /// per-copy results upstream). Today a single mailbox contributes at most
    /// one location label, but the owned-`Vec` shape is what lets a dynamic
    /// `folder:`/`lf:` id live here instead of a `Copy` enum (fact 4).
    pub fn location_label_ids(&self) -> Vec<String> {
        self.location_label_id.iter().cloned().collect()
    }
}

/// The whole plan: the ordered mailbox entries. Order places INBOX first, then
/// the other synced mailboxes (Trash, Junk, Sent, then Archive), then the rest.
/// The per-poll SCHEDULER (provider.rs, Slice 5b-2) is what actually bounds and
/// fairly orders the walk; this stable plan order is its deterministic input.
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

    /// Resolve a stored mailbox name to its OWNED location label ids (used by
    /// fetch_thread / fetch_message to label copies by role / folder, not by
    /// an ad-hoc name parse). A mailbox absent from the plan contributes none.
    pub fn location_label_ids_for_mailbox(&self, mailbox: &str) -> Vec<String> {
        self.entry_for(mailbox)
            .map(PlanEntry::location_label_ids)
            .unwrap_or_default()
    }

    /// Every dynamic (`folder:` / `lf:`) label this plan declares, as
    /// `(id, kind)` pairs in list order: first the `lf:` labels (every
    /// label-folder child, `kind = "user"`) then the `folder:` labels (every
    /// user folder, `kind = "folder"`), each sorted by id. `list_labels` reads
    /// this to publish the dynamic catalog whether or not any message is
    /// currently in a given folder (fact 2).
    pub fn dynamic_labels(&self) -> Vec<(String, &'static str)> {
        let mut lf: Vec<&PlanEntry> = self
            .entries
            .iter()
            .filter(|e| e.label_kind == LabelKind::LabelFolder)
            .collect();
        lf.sort_by(|a, b| a.location_label_id.cmp(&b.location_label_id));
        let mut folders: Vec<&PlanEntry> = self
            .entries
            .iter()
            .filter(|e| e.label_kind == LabelKind::UserFolder)
            .collect();
        folders.sort_by(|a, b| a.location_label_id.cmp(&b.location_label_id));

        let mut out: Vec<(String, &'static str)> = Vec::new();
        for entry in lf {
            if let Some(id) = &entry.location_label_id {
                out.push((id.clone(), "user"));
            }
        }
        for entry in folders {
            if let Some(id) = &entry.location_label_id {
                out.push((id.clone(), "folder"));
            }
        }
        out
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

/// Whether a mailbox is a CHILD of the label container AND the account stores
/// labels as folders. A container child counts as a label folder only under
/// [`LabelStorage::Folders`](super::settings::LabelStorage): with any other
/// `label_storage` there is no label container in play, so such a mailbox is an
/// ordinary user folder (fact 3). Requires a configured container.
fn is_label_folder_child(
    name: &str,
    delimiter: Option<&str>,
    settings: &ImapAccountSettings,
) -> bool {
    use super::settings::LabelStorage;
    if settings.label_storage != LabelStorage::Folders {
        return false;
    }
    let Some(container) = settings.label_container.as_deref() else {
        return false;
    };
    let delimiter = hierarchy_delimiter(delimiter);
    name.starts_with(&format!("{container}{delimiter}"))
}

/// The hierarchy delimiter to use, defaulting to `/` when none / empty.
fn hierarchy_delimiter(delimiter: Option<&str>) -> &str {
    delimiter.filter(|d| !d.is_empty()).unwrap_or("/")
}

/// The `lf:` label name for a label-folder child: its mailbox name with the
/// `<container><delimiter>` prefix stripped, so `Labels/Clients` -> `Clients`
/// and a nested `Labels/A/B` -> `A/B`. Assumes the caller already confirmed the
/// mailbox is a container child under label-folder mode.
fn label_folder_relative_name(name: &str, delimiter: Option<&str>, container: &str) -> String {
    let delimiter = hierarchy_delimiter(delimiter);
    let prefix = format!("{container}{delimiter}");
    name.strip_prefix(&prefix).unwrap_or(name).to_string()
}

/// Whether a mailbox's RFC 6154 `special_use` attribute marks it as an
/// AGGREGATE view (`\All` or `\Flagged`) rather than a real folder. Such a
/// mailbox is a view of mail stored elsewhere, so it is never synced as a
/// location and never contributes a `folder:` label (the design's aggregate
/// rule). `\All` also resolves to [`MailboxRole::All`]; `\Flagged` carries no
/// role, so this is the only guard that catches a "Starred"-style mailbox.
fn is_aggregate_attribute(special_use: Option<&str>) -> bool {
    matches!(special_use, Some(attr)
        if attr.eq_ignore_ascii_case("\\All") || attr.eq_ignore_ascii_case("\\Flagged"))
}

/// The owned location label id a resolved SYSTEM role contributes, if any:
/// `SENT`/`SPAM`/`TRASH`. Archive / Drafts / `\All` contribute none (an
/// archived message is simply not in INBOX; the others are aggregates/drafts).
fn system_role_label_id(role: MailboxRole) -> Option<&'static str> {
    match role {
        MailboxRole::Sent => Some("SENT"),
        MailboxRole::Junk => Some("SPAM"),
        MailboxRole::Trash => Some("TRASH"),
        MailboxRole::Archive | MailboxRole::Drafts | MailboxRole::All => None,
    }
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
                location_label_id: Some("INBOX".to_string()),
                synced_now: true,
            });
            continue;
        }

        // The label container itself: never synced, no label.
        if is_label_container(&mailbox.name, settings) {
            rest.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: false,
                window_class: SyncWindowClass::NotSynced,
                label_kind: LabelKind::None,
                location_label_id: None,
                synced_now: false,
            });
            continue;
        }
        // A label-container child under label-folder mode: synced (5b-2), a
        // Folder-class window, contributing `lf:<name relative to container>`.
        if is_label_folder_child(&mailbox.name, delimiter, settings) {
            let container = settings
                .label_container
                .as_deref()
                .expect("is_label_folder_child requires a container");
            let rel = label_folder_relative_name(&mailbox.name, delimiter, container);
            synced.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: false,
                window_class: SyncWindowClass::Folder,
                label_kind: LabelKind::LabelFolder,
                location_label_id: Some(format!("lf:{rel}")),
                synced_now: true, // 5b-2 run B
            });
            continue;
        }

        let role = resolve_role(mailbox, delimiter, settings);
        // An aggregate mailbox with an attribute that maps to NO MailboxRole
        // (Proton's "Starred" carries `\Flagged`) is a VIEW of mail stored
        // elsewhere, never a user folder: it must not be synced as a location
        // nor contribute a `folder:` label (fact 4 / the design's aggregate
        // rule). `\All` already resolves to MailboxRole::All below; `\Flagged`
        // has no role, so guard it explicitly here.
        if role.is_none() && is_aggregate_attribute(mailbox.special_use.as_deref()) {
            rest.push(PlanEntry {
                mailbox: mailbox.name.clone(),
                role: None,
                is_inbox: false,
                window_class: SyncWindowClass::NotSynced,
                label_kind: LabelKind::None,
                location_label_id: None,
                synced_now: false,
            });
            continue;
        }
        let (window_class, label_kind, location_label_id, synced_now, bucket_synced) = match role {
            // Aggregates and Drafts: never synced, no label.
            Some(MailboxRole::All) | Some(MailboxRole::Drafts) => {
                (SyncWindowClass::NotSynced, LabelKind::None, None, false, false)
            }
            // Sent: synced THIS run (run 3), Sent class, SENT label.
            // Its 5,000 is a non-evicting ACQUISITION bound (see
            // SyncWindowClass::Sent / SyncLimits::sent), filled in chunked
            // background rounds after INBOX; see provider.rs.
            Some(role @ MailboxRole::Sent) => (
                SyncWindowClass::Sent,
                LabelKind::SystemRole,
                system_role_label_id(role).map(str::to_string),
                true,
                true,
            ),
            // Trash and Junk: synced THIS run, Folder class, SystemRole label.
            Some(role @ (MailboxRole::Trash | MailboxRole::Junk)) => (
                SyncWindowClass::Folder,
                LabelKind::SystemRole,
                system_role_label_id(role).map(str::to_string),
                true,
                true,
            ),
            // Archive: synced THIS run (5b-2), Folder class (evicting 2,000
            // window), SystemRole classification but NO location label — an
            // archived message carries no system label (archived == no INBOX).
            Some(MailboxRole::Archive) => (
                SyncWindowClass::Folder,
                LabelKind::SystemRole,
                None,
                true,
                true,
            ),
            // User folders: synced in 5b-2 run B, Folder class, contributing
            // `folder:<full catalog name>`. Bucketed with the other synced
            // mailboxes so plan order is deterministic (role_order 4, by name).
            None => (
                SyncWindowClass::Folder,
                LabelKind::UserFolder,
                Some(format!("folder:{}", mailbox.name)),
                true,
                true,
            ),
        };
        let entry = PlanEntry {
            mailbox: mailbox.name.clone(),
            role,
            is_inbox: false,
            window_class,
            label_kind,
            location_label_id,
            synced_now,
        };
        if bucket_synced {
            synced.push(entry);
        } else {
            rest.push(entry);
        }
    }

    // Stable order within the synced-non-INBOX bucket: system roles first
    // (Trash, Junk, Sent, Archive) then user/label folders, each then by name,
    // so the per-poll walk input is deterministic. Sent sorts after Trash/Junk
    // so its (potentially long, chunked) backfill never starves the cheap
    // Trash/Junk rounds within one poll's folder budget; user/label folders
    // (role_order 4) sort after every system role. The per-poll SCHEDULER
    // (provider.rs) is what actually bounds and fairly orders the walk.
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

/// A stable ordering key for the synced-mailbox walk: Trash, Junk, Sent, then
/// Archive. Sent precedes Archive but both sort after the cheap Trash/Junk
/// rounds; the per-poll SCHEDULER (provider.rs) is what actually bounds and
/// fairly orders the walk, but a deterministic plan order keeps that input
/// stable.
fn role_order(role: Option<MailboxRole>) -> u8 {
    match role {
        Some(MailboxRole::Trash) => 0,
        Some(MailboxRole::Junk) => 1,
        Some(MailboxRole::Sent) => 2,
        Some(MailboxRole::Archive) => 3,
        _ => 4,
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

        // Sent: classified, Sent class, synced THIS run (run 3).
        let sent = find("Sent");
        assert_eq!(sent.role, Some(MailboxRole::Sent));
        assert_eq!(sent.window_class, SyncWindowClass::Sent);
        assert!(sent.synced_now, "Sent is synced in run 3");
        assert_eq!(sent.label_kind, LabelKind::SystemRole);

        // Archive: classified, Folder class, synced THIS run (5b-2 contract
        // change — this test previously asserted !archive.synced_now). It
        // contributes NO location label (archived = not in INBOX).
        let archive = find("Archive");
        assert_eq!(archive.role, Some(MailboxRole::Archive));
        assert!(archive.synced_now, "Archive is synced in 5b-2");
        assert_eq!(archive.window_class, SyncWindowClass::Folder);
        assert_eq!(
            plan.location_label_ids_for_mailbox("Archive"),
            Vec::<String>::new(),
            "Archive has no location label"
        );

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
        assert_eq!(labels.location_label_id, None);

        // A label-folder child (label_storage == Folders): synced (5b-2 run B
        // contract change — previously !synced_now), carrying `lf:<relative>`.
        let work = find("Labels/Work");
        assert_eq!(work.label_kind, LabelKind::LabelFolder);
        assert!(work.synced_now, "label folders are synced in run B");
        assert_eq!(work.location_label_id.as_deref(), Some("lf:Work"));

        // A user folder: synced (5b-2 run B contract change — previously
        // !synced_now), carrying `folder:<full name>`.
        let acme = find("Clients/Acme");
        assert_eq!(acme.label_kind, LabelKind::UserFolder);
        assert!(acme.synced_now, "user folders are synced in run B");
        assert_eq!(
            acme.location_label_id.as_deref(),
            Some("folder:Clients/Acme")
        );

        // Synced set (INBOX first, then system roles Trash/Spam/Sent/Archive,
        // then user/label folders by name): 5b-2 run B contract change — this
        // list previously ended at "Archive" with no user/label folders.
        let synced: Vec<&str> = plan.synced().map(|e| e.mailbox.as_str()).collect();
        assert_eq!(
            synced,
            vec![
                "INBOX",
                "Trash",
                "Spam",
                "Sent",
                "Archive",
                "Clients/Acme",
                "Labels/Work",
            ]
        );
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
        let catalog = vec![
            mb("INBOX", None),
            mb("Trash", Some("\\Trash")),
            mb("Spam", Some("\\Junk")),
            mb("Sent", Some("\\Sent")),
            mb("Archive", Some("\\Archive")),
        ];
        let plan = build_plan(&catalog, Some("/"), &settings());
        let ids = |name: &str| plan.location_label_ids_for_mailbox(name);
        assert_eq!(ids("INBOX"), vec!["INBOX".to_string()]);
        assert_eq!(ids("Trash"), vec!["TRASH".to_string()]);
        assert_eq!(ids("Spam"), vec!["SPAM".to_string()]);
        assert_eq!(ids("Sent"), vec!["SENT".to_string()]);
        // Archive contributes no system location label.
        assert_eq!(ids("Archive"), Vec::<String>::new());
        // A mailbox absent from the plan contributes nothing.
        assert_eq!(ids("Nope"), Vec::<String>::new());
    }

    /// A user folder yields `folder:<full catalog name>`; a label-folder child
    /// yields `lf:<name relative to the container>`, including a NESTED child.
    /// Dynamic ids with spaces, colons and non-ASCII (already modified-UTF-7
    /// decoded) are carried verbatim (test (i)).
    #[test]
    fn dynamic_labels_use_full_folder_name_and_relative_label_name() {
        let catalog = vec![
            mb("INBOX", None),
            mb("Clients/Acme", None),             // user folder
            mb("Labels/Clients", None),           // label folder child
            mb("Labels/A/B", None),               // nested label folder child
            mb("Projektübersicht: Q3", None),     // user folder, spaces/colon/non-ASCII
            mb("Labels/Zoë's: tag", None),        // label folder, odd chars
        ];
        let plan = build_plan(&catalog, Some("/"), &settings());
        let ids = |name: &str| plan.location_label_ids_for_mailbox(name);
        assert_eq!(ids("Clients/Acme"), vec!["folder:Clients/Acme".to_string()]);
        assert_eq!(ids("Labels/Clients"), vec!["lf:Clients".to_string()]);
        assert_eq!(ids("Labels/A/B"), vec!["lf:A/B".to_string()]);
        assert_eq!(
            ids("Projektübersicht: Q3"),
            vec!["folder:Projektübersicht: Q3".to_string()]
        );
        assert_eq!(ids("Labels/Zoë's: tag"), vec!["lf:Zoë's: tag".to_string()]);
    }

    /// (d) A container child under label_storage Keywords / None is NOT an
    /// `lf:` label — there is no container in play — so it is an ordinary USER
    /// FOLDER (`folder:<full name>`), still synced.
    #[test]
    fn a_container_child_without_folder_storage_is_a_plain_user_folder() {
        for storage in [LabelStorage::Keywords, LabelStorage::None] {
            let mut settings = settings();
            settings.label_storage = storage;
            // Keep label_container set to prove it is the STORAGE mode, not the
            // missing container, that decides this.
            settings.label_container = Some("Labels".into());
            let catalog = vec![mb("INBOX", None), mb("Labels/Clients", None)];
            let plan = build_plan(&catalog, Some("/"), &settings);
            let child = plan.entry_for("Labels/Clients").unwrap();
            assert_eq!(
                child.label_kind,
                LabelKind::UserFolder,
                "a container child under {storage:?} is a user folder, not lf:"
            );
            assert!(child.synced_now, "it is still synced");
            assert_eq!(
                child.location_label_id.as_deref(),
                Some("folder:Labels/Clients"),
                "and carries a folder: label with its full name under {storage:?}"
            );
        }
    }

    /// `dynamic_labels()` lists `lf:` labels first (kind `user`), then
    /// `folder:` labels (kind `folder`), each sorted by id, independent of
    /// whether a message is in them.
    #[test]
    fn dynamic_labels_are_ordered_lf_then_folders() {
        let catalog = vec![
            mb("INBOX", None),
            mb("Zed/Folder", None),       // user folder
            mb("Clients/Acme", None),     // user folder
            mb("Labels/Work", None),      // lf
            mb("Labels/Acme", None),      // lf
        ];
        let plan = build_plan(&catalog, Some("/"), &settings());
        assert_eq!(
            plan.dynamic_labels(),
            vec![
                ("lf:Acme".to_string(), "user"),
                ("lf:Work".to_string(), "user"),
                ("folder:Clients/Acme".to_string(), "folder"),
                ("folder:Zed/Folder".to_string(), "folder"),
            ]
        );
    }
}
