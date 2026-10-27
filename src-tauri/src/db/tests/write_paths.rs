//! Pins the exact rows that synced projection, local saves, and settings
//! import write, column by column, so moving their SQL into the feature
//! modules (`docs/archive/db-layer-plan.md`, Phase 4) cannot change what is stored.
//! Timestamps generated during the test read `<now>` and generated ids read
//! `<uuid>`; every other value is compared exactly.

use super::*;
use crate::db::test_support::database;
use crate::models::MailProviderKind;
use crate::signatures::tests::{defaults, template};
use rusqlite::types::ValueRef;
use serde_json::{json, Value};
use threestrands_sync_protocol::EntityType;

const T0: &str = "2020-01-02T03:04:05+00:00";
const T1: &str = "2021-02-03T04:05:06+00:00";

fn normalize(text: &str, start: &chrono::DateTime<chrono::FixedOffset>) -> String {
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        if time >= *start {
            return "<now>".into();
        }
    }
    if uuid::Uuid::parse_str(text).is_ok() && text.len() == 36 {
        return "<uuid>".into();
    }
    text.to_string()
}

fn dump(db: &Database, tables: &[&str], start: &chrono::DateTime<chrono::FixedOffset>) -> String {
    let connection = db.connection().unwrap();
    let mut out = String::new();
    for table in tables {
        let mut statement = connection.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns: Vec<String> = statement.column_names().iter().map(|name| name.to_string()).collect();
        let mut rows: Vec<String> = statement
            .query_map([], |row| {
                Ok((0..columns.len())
                    .map(|index| {
                        let value = match row.get_ref(index).unwrap() {
                            ValueRef::Null => "NULL".to_string(),
                            ValueRef::Integer(value) => value.to_string(),
                            ValueRef::Real(value) => value.to_string(),
                            ValueRef::Text(bytes) => normalize(&String::from_utf8_lossy(bytes), start),
                            ValueRef::Blob(bytes) => format!("<{} bytes>", bytes.len()),
                        };
                        format!("{}={}", columns[index], value)
                    })
                    .collect::<Vec<_>>()
                    .join(" | "))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        out.push_str(&format!("[{table}]\n"));
        for row in rows {
            out.push_str(&row);
            out.push('\n');
        }
    }
    out
}

fn start() -> chrono::DateTime<chrono::FixedOffset> {
    // Truncate to whole seconds: some writers round timestamps.
    chrono::DateTime::parse_from_rfc3339(&(chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap()
}

fn task(id: &str, title: &str, updated_at: &str) -> Value {
    json!({
        "id": id, "accountId": "me@example.com", "threadId": "thread-1", "sourceMessageId": "message-1",
        "subjectSnapshot": "Subject", "title": title, "notes": "Notes", "kind": "action", "dueKind": "date",
        "dueValue": "2020-03-04", "timeZone": "UTC", "repeatIntervalDays": 7, "status": "open",
        "completionSource": null, "evidenceText": null, "waitAfter": null, "createdAt": T0,
        "updatedAt": updated_at, "completedAt": null, "goalId": null,
    })
}

#[test]
fn synced_projection_writes_exactly_these_rows() {
    let start = start();
    let db = database();
    db.adopt_mail_account("existing@example.com", MailProviderKind::Gmail).unwrap();
    db.set_beta_features_enabled(true).unwrap();
    let project = |entity_type: EntityType, id: &str, payload: Value| db.materialize_entity(entity_type, id, Some(&payload), false).unwrap();
    let delete = |entity_type: EntityType, id: &str| db.materialize_entity(entity_type, id, None, true).unwrap();

    project(EntityType::Task, "task-1", task("task-1", "First", T0));
    project(EntityType::Task, "task-1", task("task-1", "Renamed", T1));
    project(EntityType::Task, "task-2", task("task-2", "Doomed", T0));
    delete(EntityType::Task, "task-2");

    project(EntityType::Snippet, "snippet-1", json!({"id": "snippet-1", "name": "Hi", "body": "Hello", "createdAt": T0}));
    project(EntityType::Snippet, "snippet-1", json!({"id": "snippet-1", "name": "Hey", "body": "Hello again", "createdAt": T1}));
    project(EntityType::Snippet, "snippet-2", json!({"id": "snippet-2", "name": "Bye", "body": "Later", "createdAt": T0}));
    delete(EntityType::Snippet, "snippet-2");

    let split = |name: &str, created_at: &str| json!({"id": "split-1", "name": name, "matchKind": "domain", "matchValue": "example.com", "sortOrder": 3, "createdAt": created_at, "accountId": "me@example.com"});
    project(EntityType::SplitInbox, "split-1", split("News", T0));
    project(EntityType::SplitInbox, "split-1", split("Newsletters", T1));
    project(EntityType::SplitInbox, "split-2", json!({"id": "split-2", "name": "Gone", "matchKind": "domain", "matchValue": "x.example.com", "sortOrder": 1, "createdAt": T0, "accountId": "me@example.com"}));
    delete(EntityType::SplitInbox, "split-2");

    project(EntityType::MailAccount, "existing@example.com", json!({"email": "existing@example.com", "displayName": "Me", "nickname": "Work", "color": "#112233", "provider": "gmail", "sortOrder": 5}));
    project(EntityType::MailAccount, "new@example.com", json!({"email": "new@example.com", "displayName": null, "nickname": null, "color": "#445566", "provider": "imap", "sortOrder": 6}));

    project(EntityType::CalendarAccount, "cal-a@example.com", json!({"email": "cal-a@example.com"}));
    project(EntityType::CalendarAccount, "cal-b@example.com", json!({"email": "cal-b@example.com", "sortOrder": 9}));
    project(EntityType::CalendarAccount, "cal-a@example.com", json!({"email": "cal-a@example.com", "sortOrder": 4}));
    project(EntityType::CalendarAccount, "cal-b@example.com", json!({"email": "cal-b@example.com"}));

    let report = |operation: &str| json!({
        "operationId": operation, "ownerInstallationId": "00000000-0000-4000-8000-000000000001",
        "ownerSyncDeviceId": null, "ownerNameAtCreation": "Laptop", "account": "me@example.com",
        "subject": "Later", "scheduledAt": 1_900_000_000_000_i64, "timeZone": "UTC", "state": "scheduled",
        "blockedReason": null, "reportRevision": 1, "statusChangedAt": 1_800_000_000_000_i64,
    });
    let summary_id = |operation: &str| format!("00000000-0000-4000-8000-000000000001:{operation}");
    project(EntityType::ScheduledSendSummary, &summary_id("00000000-0000-4000-8000-0000000000a1"), json!({"report": report("00000000-0000-4000-8000-0000000000a1")}));
    let mut revised = report("00000000-0000-4000-8000-0000000000a1");
    revised["reportRevision"] = json!(2);
    project(EntityType::ScheduledSendSummary, &summary_id("00000000-0000-4000-8000-0000000000a1"), json!({"report": revised}));
    project(EntityType::ScheduledSendSummary, &summary_id("00000000-0000-4000-8000-0000000000a2"), json!({"report": report("00000000-0000-4000-8000-0000000000a2")}));
    delete(EntityType::ScheduledSendSummary, &summary_id("00000000-0000-4000-8000-0000000000a2"));

    project(EntityType::SenderRule, "rule-1", json!({"id": "rule-1", "accountId": "me@example.com", "matchKind": "address", "matchValue": "spam@example.com", "action": "mute", "createdAt": T0}));
    project(EntityType::SenderRule, "rule-2", json!({"id": "rule-2", "accountId": "me@example.com", "matchKind": "address", "matchValue": "gone@example.com", "action": "mute", "createdAt": T0}));
    delete(EntityType::SenderRule, "rule-2");

    project(EntityType::Preferences, "portable", json!({"fontSize": 14, "deviceName:abc123": "Laptop"}));
    db.update_synced_preferences(json!({"fontSize": 16, "theme": "dark"})).unwrap();
    project(EntityType::Retention, "mail", json!({"days": 30}));

    let actual = dump(
        &db,
        &["tasks", "snippets", "split_inboxes", "accounts", "calendar_accounts", "scheduled_send_summaries", "sender_rules", "synced_preferences", "sync_device_labels", "compose_settings"],
        &start,
    );
    assert_eq!(actual, SYNCED_PROJECTION, "\n--- actual ---\n{actual}");
}

#[test]
fn local_saves_write_exactly_these_rows() {
    let start = start();
    let db = database();
    db.create_snippet("Hi", "Hello").unwrap();
    db.create_split_inbox("News", "domain", "example.com", "me@example.com").unwrap();
    let contact: crate::models::SaveContactRequest = serde_json::from_value(json!({
        "id": "contact-1", "displayName": "Ada", "role": null, "company": "Engines", "location": null, "bio": null,
        "notes": null, "links": ["https://example.com"], "photoData": null, "favorite": true,
        "addresses": ["ada@example.com", "ada@work.example.com"], "birthday": "1990-12-10",
    }))
    .unwrap();
    db.save_contact_profile(&contact).unwrap();
    let group = db.create_contact_group("Friends", &["contact-1".to_string()], &[]).unwrap();
    db.add_contact_group_members(&group.group.id, &[], &["new@example.com".to_string()]).unwrap();
    db.set_retention_days(Some(90)).unwrap();
    db.set_retention_days(None).unwrap();
    db.set_retention_days(Some(365)).unwrap();

    let actual = dump(
        &db,
        &["snippets", "split_inboxes", "contacts", "contact_addresses", "contact_groups", "contact_group_members", "compose_settings"],
        &start,
    );
    assert_eq!(actual, LOCAL_SAVES, "\n--- actual ---\n{actual}");
}

#[test]
fn settings_import_writes_exactly_these_rows() {
    let start = start();
    let db = database();
    db.adopt_mail_account("existing@example.com", MailProviderKind::Gmail).unwrap();
    db.create_snippet("Old", "Replaced by the import").unwrap();
    db.store_signature(&template("old")).unwrap();

    let accounts: Vec<crate::transfer::TransferAccount> = serde_json::from_value(json!([
        {"email": "existing@example.com", "displayName": "Me", "nickname": "Home", "color": "#010203", "provider": "gmail", "sortOrder": 0},
        {"email": "Chosen@example.com", "displayName": null, "color": "#040506", "provider": "gmail", "sortOrder": 1},
        {"email": "elsewhere@example.com", "displayName": "Elsewhere", "color": "#070809", "sortOrder": 2},
    ]))
    .unwrap();
    let not_here = std::collections::HashSet::from(["elsewhere@example.com".to_string()]);
    let splits: Vec<crate::transfer::TransferSplitInbox> = serde_json::from_value(json!([
        {"id": "split-1", "name": "News", "matchKind": "domain", "matchValue": "example.com", "sortOrder": 0, "createdAt": T0, "accountId": "existing@example.com"},
    ]))
    .unwrap();
    let snippets: Vec<crate::transfer::TransferSnippet> = serde_json::from_value(json!([
        {"id": "snippet-1", "name": "Hi", "body": "Hello", "createdAt": T0},
    ]))
    .unwrap();
    let contacts: Vec<crate::transfer::TransferContact> = serde_json::from_value(json!([
        {"id": "contact-1", "displayName": "Ada", "role": "Analyst", "company": null, "location": null, "bio": null, "notes": "n",
         "links": ["https://example.com"], "photoData": null, "favorite": false, "addresses": ["ada@example.com"],
         "birthday": "1990-12-10", "keepInTouch": {"intervalDays": 30, "startedAt": T0, "snoozedUntil": null, "snoozedAt": null, "lastTouchAt": T1}},
    ]))
    .unwrap();
    let groups: Vec<crate::transfer::TransferContactGroup> = serde_json::from_value(json!([
        {"id": "group-1", "name": " Friends ", "memberIds": ["contact-1"], "createdAt": T0},
    ]))
    .unwrap();
    let rules: Vec<crate::transfer::TransferSenderRule> = serde_json::from_value(json!([
        {"id": "rule-1", "accountId": "existing@example.com", "matchKind": "address", "matchValue": "Spam@Example.com", "action": "mute", "createdAt": T0},
    ]))
    .unwrap();
    let library = crate::signatures::SignatureLibrary {
        signatures: vec![template("work")],
        defaults: vec![defaults("existing@example.com", "", Some("work"), None)],
    };
    db.import_transfer_data(&accounts, &not_here, &splits, &snippets, &contacts, Some(&groups), Some(&rules), Some(180), Some(&library))
        .unwrap();

    let actual = dump(
        &db,
        &["accounts", "split_inboxes", "snippets", "contacts", "contact_addresses", "contact_groups", "contact_group_members", "sender_rules", "signatures", "signature_defaults", "compose_settings"],
        &start,
    );
    assert_eq!(actual, SETTINGS_IMPORT, "\n--- actual ---\n{actual}");
}

const SYNCED_PROJECTION: &str = r##"[tasks]
id=task-1 | account_id=me@example.com | thread_id=thread-1 | source_message_id=message-1 | subject_snapshot=Subject | title=Renamed | notes=Notes | kind=action | due_kind=date | due_value=2020-03-04 | time_zone=UTC | repeat_interval_days=7 | status=open | completion_source=NULL | evidence_text=NULL | wait_after=NULL | created_at=2020-01-02T03:04:05+00:00 | updated_at=2021-02-03T04:05:06+00:00 | completed_at=NULL | goal_id=NULL
[snippets]
id=snippet-1 | name=Hey | body=Hello again | created_at=2020-01-02T03:04:05+00:00 | updated_at=<now>
[split_inboxes]
id=split-1 | name=Newsletters | match_kind=domain | match_value=example.com | sort_order=3 | created_at=2020-01-02T03:04:05+00:00 | account_id=me@example.com | updated_at=<now>
[accounts]
email=existing@example.com | display_name=Me | color=#112233 | status=connected | sort_order=5 | connected_at=<now> | last_synced_at=NULL | provider=gmail | nickname=Work | on_this_device=1
email=new@example.com | display_name=NULL | color=#445566 | status=needs_reauth | sort_order=6 | connected_at=<now> | last_synced_at=NULL | provider=imap | nickname=NULL | on_this_device=0
[calendar_accounts]
email=cal-a@example.com | connected_at=<now> | selection_initialized=0 | status=needs_reauth | sort_order=4 | availability_calendar_ids=NULL
email=cal-b@example.com | connected_at=<now> | selection_initialized=0 | status=needs_reauth | sort_order=9 | availability_calendar_ids=NULL
[scheduled_send_summaries]
id=00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-0000000000a1 | report={"account":"me@example.com","blockedReason":null,"operationId":"00000000-0000-4000-8000-0000000000a1","ownerInstallationId":"00000000-0000-4000-8000-000000000001","ownerNameAtCreation":"Laptop","ownerSyncDeviceId":null,"reportRevision":2,"scheduledAt":1900000000000,"state":"scheduled","statusChangedAt":1800000000000,"subject":"Later","timeZone":"UTC"}
[sender_rules]
id=rule-1 | account_id=me@example.com | match_kind=address | match_value=spam@example.com | action=mute | created_at=2020-01-02T03:04:05+00:00 | updated_at=<now>
[synced_preferences]
key=portable | value={"deviceName:abc123":"Laptop","fontSize":16,"theme":"dark"} | updated_at=<now>
[sync_device_labels]
device_id=abc123 | label=Laptop
[compose_settings]
key=retention_days | value=30
"##;
const LOCAL_SAVES: &str = r##"[snippets]
id=<uuid> | name=Hi | body=Hello | created_at=<now> | updated_at=NULL
[split_inboxes]
id=<uuid> | name=News | match_kind=domain | match_value=example.com | sort_order=0 | created_at=<now> | account_id=me@example.com | updated_at=NULL
[contacts]
id=contact-1 | display_name=Ada | role=NULL | company=Engines | location=NULL | bio=NULL | notes=NULL | links_json=["https://example.com"] | photo_data=NULL | favorite=1 | updated_at=<now> | birthday=1990-12-10 | kit_interval_days=NULL | kit_started_at=NULL | kit_snoozed_until=NULL | kit_snoozed_at=NULL | kit_last_touch_at=NULL
id=contact:f0030501023327437b06e5c6f87df7871b8e704ae608d1d0b7b24fdd2a06c716 | display_name=NULL | role=NULL | company=NULL | location=NULL | bio=NULL | notes=NULL | links_json=[] | photo_data=NULL | favorite=0 | updated_at=<now> | birthday=NULL | kit_interval_days=NULL | kit_started_at=NULL | kit_snoozed_until=NULL | kit_snoozed_at=NULL | kit_last_touch_at=NULL
[contact_addresses]
contact_id=contact-1 | email=ada@example.com | position=0
contact_id=contact-1 | email=ada@work.example.com | position=1
contact_id=contact:f0030501023327437b06e5c6f87df7871b8e704ae608d1d0b7b24fdd2a06c716 | email=new@example.com | position=0
[contact_groups]
id=<uuid> | name=Friends | created_at=<now> | updated_at=<now>
[contact_group_members]
group_id=<uuid> | contact_id=contact-1
group_id=<uuid> | contact_id=contact:f0030501023327437b06e5c6f87df7871b8e704ae608d1d0b7b24fdd2a06c716
[compose_settings]
key=retention_days | value=365
"##;
const SETTINGS_IMPORT: &str = r##"[accounts]
email=Chosen@example.com | display_name=NULL | color=#040506 | status=needs_reauth | sort_order=1 | connected_at=<now> | last_synced_at=NULL | provider=gmail | nickname=NULL | on_this_device=1
email=elsewhere@example.com | display_name=Elsewhere | color=#070809 | status=needs_reauth | sort_order=2 | connected_at=<now> | last_synced_at=NULL | provider=gmail | nickname=NULL | on_this_device=0
email=existing@example.com | display_name=Me | color=#010203 | status=connected | sort_order=0 | connected_at=<now> | last_synced_at=NULL | provider=gmail | nickname=Home | on_this_device=1
[split_inboxes]
id=split-1 | name=News | match_kind=domain | match_value=example.com | sort_order=0 | created_at=2020-01-02T03:04:05+00:00 | account_id=existing@example.com | updated_at=NULL
[snippets]
id=snippet-1 | name=Hi | body=Hello | created_at=2020-01-02T03:04:05+00:00 | updated_at=NULL
[contacts]
id=contact-1 | display_name=Ada | role=Analyst | company=NULL | location=NULL | bio=NULL | notes=n | links_json=["https://example.com"] | photo_data=NULL | favorite=0 | updated_at=<now> | birthday=1990-12-10 | kit_interval_days=30 | kit_started_at=2020-01-02T03:04:05+00:00 | kit_snoozed_until=NULL | kit_snoozed_at=NULL | kit_last_touch_at=2021-02-03T04:05:06+00:00
[contact_addresses]
contact_id=contact-1 | email=ada@example.com | position=0
[contact_groups]
id=group-1 | name=Friends | created_at=2020-01-02T03:04:05+00:00 | updated_at=<now>
[contact_group_members]
group_id=group-1 | contact_id=contact-1
[sender_rules]
id=rule-1 | account_id=existing@example.com | match_kind=address | match_value=spam@example.com | action=mute | created_at=2020-01-02T03:04:05+00:00 | updated_at=<now>
[signatures]
id=work | payload={"id":"work","name":"work","html":"<p>work</p>","text":"work"}
[signature_defaults]
id=signature-default:7af1db0afc5853041cb7b4489026ea7cabf3c675051e5d30b6bbd373bc5c7642 | payload={"id":"signature-default:7af1db0afc5853041cb7b4489026ea7cabf3c675051e5d30b6bbd373bc5c7642","account":"existing@example.com","address":"","newSignatureId":"work","replySignatureId":null}
[compose_settings]
key=retention_days | value=180
"##;
