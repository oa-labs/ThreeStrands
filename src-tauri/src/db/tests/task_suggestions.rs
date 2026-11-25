use super::*;
use crate::correspondence::OutboxItem;
use crate::db::test_support::database;
use crate::models::CompletedByEmail;
use crate::note_entities::NoteReference;

const DAY: &str = "2026-11-16";
/// Local midnight of `DAY` for a user at UTC+10.
const EAST_DAY_START: &str = "2026-11-15T14:00:00Z";
const AT: i64 = 1_900_000_000_000;

fn setup() -> Database {
    let db = database();
    db.adopt_account("me@example.com").unwrap();
    db.connection()
        .unwrap()
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    db
}

fn thread(db: &Database, provider_id: &str) -> String {
    let id = format!("me@example.com:{provider_id}");
    db.connection()
        .unwrap()
        .execute(
            "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json,
                last_message_at, unread, starred, archived, labels_json, trashed, account_id,
                has_attachments, last_received_at)
             VALUES (?1, ?2, 'Subject', '', '[]', '2026-11-10T10:00:00Z', 0, 0, 0, '[]', 0,
                'me@example.com', 0, '2026-11-10T10:00:00Z')",
            [&id, provider_id],
        )
        .unwrap();
    id
}

/// A conversation in which `email` took part.
fn interaction(db: &Database, thread_id: &str, email: &str) {
    db.connection()
        .unwrap()
        .execute(
            "INSERT INTO contact_interactions(message_id, thread_id, account_id, email, display_name, direction, sent_at)
             VALUES (?1, ?2, 'me@example.com', ?3, NULL, 'received', '2026-11-10T10:00:00Z')",
            [&format!("m-{thread_id}-{email}"), thread_id, email],
        )
        .unwrap();
}

fn task(db: &Database, title: &str, thread_id: Option<&str>, due: Option<(&str, &str)>) -> String {
    let (due_kind, due_value) = due.unwrap_or(("none", ""));
    db.create_task(
        &serde_json::from_value(serde_json::json!({
            "accountId": "me@example.com",
            "title": title,
            "kind": "action",
            "threadId": thread_id,
            "subjectSnapshot": thread_id.map(|_| "Subject"),
            "dueKind": due_kind,
            "dueValue": (due_kind != "none").then_some(due_value),
            "timeZone": (due_kind == "datetime").then_some("Australia/Brisbane"),
        }))
        .unwrap(),
    )
    .unwrap()
    .id
}

fn plan(db: &Database, ids: &[&str]) {
    let lines: String = ids
        .iter()
        .map(|id| {
            let reference = NoteReference {
                version: 1,
                kind: "task".into(),
                id: (*id).into(),
                account: None,
                calendar: None,
                note: None,
                contact: None,
                series: None,
            };
            format!("- [t]({})\n", reference.href().unwrap())
        })
        .collect();
    db.save_daily_note(DAY, &format!("## Plan\n\n{lines}"))
        .unwrap();
}

struct Mail<'a> {
    to: &'a str,
    cc: &'a str,
    bcc: &'a str,
    thread: Option<&'a str>,
    state: &'a str,
}

impl Default for Mail<'_> {
    fn default() -> Self {
        Self {
            to: "",
            cc: "",
            bcc: "",
            thread: None,
            state: "ready",
        }
    }
}

fn queue(db: &Database, mail: Mail<'_>) -> String {
    let mut draft = db.create_draft("new", None, "me@example.com", &[]).unwrap();
    draft.to = mail.to.into();
    draft.cc = mail.cc.into();
    draft.bcc = mail.bcc.into();
    draft.subject = "Hello".into();
    let mut draft = db.save_draft(draft).unwrap();
    // A reply's conversation comes from its source message, not from a save.
    draft.thread_id = mail.thread.map(Into::into);
    let id = format!("send-{}", draft.id);
    let revision = draft.revision;
    db.insert_queued_send(
        &OutboxItem {
            id: id.clone(),
            draft,
            state: mail.state.into(),
            deadline: 0,
            error: None,
            provider_id: None,
            last_attempt_at: None,
            archive_on_send: false,
            schedule: None,
            sent_copy: None,
        },
        revision,
        b"raw",
    )
    .unwrap();
    id
}

fn suggest(db: &Database, outbox: &str) -> Option<String> {
    db.suggest_task_for_outbox(outbox, DAY, Some(EAST_DAY_START))
        .unwrap()
        .map(|suggestion| suggestion.task_id)
}

mod matcher {
    use super::*;

    #[test]
    fn a_reply_to_the_tasks_conversation_matches() {
        let db = setup();
        let conversation = thread(&db, "t1");
        let linked = task(&db, "Answer the budget question", Some(&conversation), None);
        task(&db, "Unrelated", None, None);
        let id = queue(
            &db,
            Mail {
                to: "pat@example.org",
                thread: Some("t1"),
                ..Default::default()
            },
        );
        let suggestion = db
            .suggest_task_for_outbox(&id, DAY, Some(EAST_DAY_START))
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.task_id, linked);
        assert_eq!(suggestion.title, "Answer the budget question");
    }

    #[test]
    fn a_planned_task_naming_a_recipients_first_name_matches() {
        let db = setup();
        let planned = task(&db, "Send Priya the contract", None, None);
        plan(&db, &[&planned]);
        let id = queue(
            &db,
            Mail {
                to: "Priya Raman <p.raman@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(planned));
    }

    #[test]
    fn a_saved_contacts_name_is_used_when_the_draft_has_only_an_address() {
        let db = setup();
        let planned = task(&db, "Call Priya about rates", None, None);
        plan(&db, &[&planned]);
        db.save_contact_profile(&crate::models::SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Priya Raman".into()),
            role: None,
            company: None,
            phone: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["p.raman@example.org".into()],
            time_zone: None,
        })
        .unwrap();
        let id = queue(
            &db,
            Mail {
                to: "p.raman@example.org",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(planned));
    }

    #[test]
    fn a_task_due_today_in_a_conversation_with_the_recipient_matches() {
        let db = setup();
        let conversation = thread(&db, "t2");
        interaction(&db, &conversation, "lee@example.org");
        let due = task(
            &db,
            "Review the draft",
            Some(&conversation),
            Some(("date", DAY)),
        );
        let overdue_elsewhere = task(&db, "Other thing", None, Some(("date", "2026-11-01")));
        let _ = overdue_elsewhere;
        let id = queue(
            &db,
            Mail {
                to: "lee@example.org",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(due));
    }

    #[test]
    fn an_overdue_task_counts_and_a_task_due_later_does_not() {
        let db = setup();
        let conversation = thread(&db, "t3");
        interaction(&db, &conversation, "lee@example.org");
        let overdue = task(
            &db,
            "Review the draft",
            Some(&conversation),
            Some(("date", "2026-11-10")),
        );
        let later = thread(&db, "t4");
        interaction(&db, &later, "lee@example.org");
        task(
            &db,
            "Review the plan",
            Some(&later),
            Some(("date", "2026-11-17")),
        );
        let id = queue(
            &db,
            Mail {
                to: "lee@example.org",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(overdue));
    }

    #[test]
    fn two_candidates_suggest_nothing() {
        let db = setup();
        let first = task(&db, "Send Priya the contract", None, None);
        let second = task(&db, "Ask Priya about dates", None, None);
        plan(&db, &[&first, &second]);
        let id = queue(
            &db,
            Mail {
                to: "Priya <priya@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None);
    }

    #[test]
    fn a_task_that_only_names_the_recipient_but_is_neither_planned_nor_due_is_ignored() {
        let db = setup();
        task(&db, "Send Priya the contract", None, None);
        let id = queue(
            &db,
            Mail {
                to: "Priya <priya@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None);
    }

    #[test]
    fn a_planned_task_unrelated_to_the_recipients_is_ignored() {
        let db = setup();
        let planned = task(&db, "Pay the invoice", None, None);
        plan(&db, &[&planned]);
        let id = queue(
            &db,
            Mail {
                to: "Priya <priya@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None);
    }

    #[test]
    fn finished_and_note_tasks_are_not_candidates() {
        let db = setup();
        let done = task(&db, "Send Priya the contract", None, None);
        let cancelled = task(&db, "Call Priya", None, None);
        let note = db
            .create_note("Projects", "- ( ) Email Priya the deck\n")
            .unwrap();
        let marker = db.list_note_tasks().unwrap()[0].marker_id.clone();
        let note_task = NoteReference {
            version: 1,
            kind: "task".into(),
            id: marker,
            account: None,
            calendar: None,
            note: Some(note.id),
            contact: None,
            series: None,
        };
        db.save_daily_note(
            DAY,
            &format!(
                "## Plan\n\n- [a]({})\n- [b]({})\n- [c]({})\n",
                task_href(&done),
                task_href(&cancelled),
                note_task.href().unwrap()
            ),
        )
        .unwrap();
        db.set_task_status(&done, "completed", "user").unwrap();
        db.set_task_status(&cancelled, "cancelled", "user").unwrap();
        let id = queue(
            &db,
            Mail {
                to: "Priya <priya@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None);
    }

    fn task_href(id: &str) -> String {
        NoteReference {
            version: 1,
            kind: "task".into(),
            id: id.into(),
            account: None,
            calendar: None,
            note: None,
            contact: None,
            series: None,
        }
        .href()
        .unwrap()
    }

    #[test]
    fn the_accounts_own_addresses_are_not_recipients() {
        let db = setup();
        let planned = task(&db, "Email me@example.com the notes", None, None);
        plan(&db, &[&planned]);
        let own = queue(
            &db,
            Mail {
                to: "Me <me@example.com>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &own), None);
        let other = queue(
            &db,
            Mail {
                to: "other@example.org",
                cc: "me@example.com",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &other), None);
    }

    #[test]
    fn bcc_recipients_are_ignored() {
        let db = setup();
        let planned = task(&db, "Email Priya the contract", None, None);
        plan(&db, &[&planned]);
        let id = queue(
            &db,
            Mail {
                to: "lee@example.org",
                bcc: "Priya <priya@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None);
    }

    #[test]
    fn a_first_name_below_the_minimum_is_ignored() {
        let db = setup();
        let planned = task(&db, "Send Al the contract", None, None);
        plan(&db, &[&planned]);
        let id = queue(
            &db,
            Mail {
                to: "Al Green <al.green@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), None, "two letters is below the minimum");
        let id = queue(
            &db,
            Mail {
                to: "Alf Green <alf.green@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(
            suggest(&db, &id),
            None,
            "three letters is long enough, but this title doesn't name Alf"
        );
        let named = task(&db, "Send Alf the contract", None, None);
        plan(&db, &[&planned, &named]);
        assert_eq!(
            suggest(&db, &id),
            Some(named),
            "exactly the minimum matches"
        );
    }

    #[test]
    fn names_match_whole_words_only() {
        let db = setup();
        let planned = task(&db, "Send Dana the contract", None, None);
        plan(&db, &[&planned]);
        let dan = queue(
            &db,
            Mail {
                to: "Dan Smith <dan@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &dan), None);
        let dana = queue(
            &db,
            Mail {
                to: "dana@example.org",
                cc: "Dana Cole <dana@example.org>",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &dana), Some(planned));
    }

    #[test]
    fn a_task_title_can_name_the_address() {
        let db = setup();
        let planned = task(&db, "Reply to kim@example.org about the lease", None, None);
        plan(&db, &[&planned]);
        let id = queue(
            &db,
            Mail {
                to: "kim@example.org",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(planned));
    }

    #[test]
    fn due_today_follows_the_local_day_east_of_utc() {
        let db = setup();
        let conversation = thread(&db, "t5");
        interaction(&db, &conversation, "lee@example.org");
        // 23:30 on the 16th at UTC+10, and 00:30 on the 17th.
        let late_today = task(
            &db,
            "Review the draft",
            Some(&conversation),
            Some(("datetime", "2026-11-16T13:30:00Z")),
        );
        let id = queue(
            &db,
            Mail {
                to: "lee@example.org",
                ..Default::default()
            },
        );
        assert_eq!(suggest(&db, &id), Some(late_today.clone()));
        db.set_task_status(&late_today, "completed", "user")
            .unwrap();
        task(
            &db,
            "Review the plan",
            Some(&conversation),
            Some(("datetime", "2026-11-16T14:30:00Z")),
        );
        assert_eq!(
            suggest(&db, &id),
            None,
            "tomorrow in the user's day, though still today in UTC"
        );
    }

    #[test]
    fn a_linked_draft_and_a_missing_message_suggest_nothing() {
        let db = setup();
        let conversation = thread(&db, "t6");
        task(&db, "Answer", Some(&conversation), None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                thread: Some("t6"),
                ..Default::default()
            },
        );
        let other = task(&db, "Other", None, None);
        db.link_task_to_outbox(&id, &other, AT).unwrap();
        assert_eq!(suggest(&db, &id), None);
        assert_eq!(suggest(&db, "missing"), None);
        assert!(db.suggest_task_for_outbox(&id, "not-a-date", None).is_err());
    }
}

mod link {
    use super::*;

    fn status(db: &Database, id: &str) -> String {
        db.list_tasks(None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
            .status
    }

    fn set_state(db: &Database, outbox: &str, state: &str) {
        db.connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state=?2 WHERE id=?1",
                [outbox, state],
            )
            .unwrap();
    }

    #[test]
    fn a_queued_message_in_any_live_state_takes_the_link() {
        for state in ["undo_pending", "ready", "scheduled", "sending", "uncertain"] {
            let db = setup();
            let one = task(&db, "Send the contract", None, None);
            let id = queue(
                &db,
                Mail {
                    to: "x@example.org",
                    state,
                    ..Default::default()
                },
            );
            assert_eq!(
                db.link_task_to_outbox(&id, &one, AT).unwrap(),
                LinkedSend::Queued,
                "{state}"
            );
            assert_eq!(
                status(&db, &one),
                "open",
                "{state}: nothing finishes before the send"
            );
            let draft = db
                .outbox()
                .unwrap()
                .into_iter()
                .find(|i| i.id == id)
                .unwrap()
                .draft;
            assert_eq!(
                draft.completes_task_id.as_deref(),
                Some(one.as_str()),
                "{state}"
            );
        }
    }

    #[test]
    fn the_task_finishes_when_the_linked_message_leaves() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        db.link_task_to_outbox(&id, &one, AT).unwrap();
        assert_eq!(db.claim_due_send(&id, AT, 0, None, "").unwrap(), 1);
        db.record_send_accepted(&id, "provider-9", None, "", AT)
            .unwrap();
        let done = db
            .list_tasks(None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == one)
            .unwrap();
        assert_eq!(done.status, "completed");
        assert_eq!(done.completion_source.as_deref(), Some("user"));
        assert_eq!(
            done.completed_by_email.unwrap().message_id.as_deref(),
            Some("provider-9")
        );
    }

    #[test]
    fn an_already_sent_message_completes_the_task_with_its_record() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        assert_eq!(db.claim_due_send(&id, AT, 0, None, "").unwrap(), 1);
        db.record_send_accepted(&id, "provider-3", None, "", AT)
            .unwrap();
        assert_eq!(
            db.link_task_to_outbox(&id, &one, AT + 7).unwrap(),
            LinkedSend::Sent
        );
        let done = db
            .list_tasks(None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == one)
            .unwrap();
        assert_eq!(done.status, "completed");
        assert_eq!(
            done.completed_by_email,
            Some(CompletedByEmail {
                account_id: "me@example.com".into(),
                subject: "Hello".into(),
                sent_at: AT + 7,
                message_id: Some("provider-3".into()),
                thread_id: None,
                recipients: vec!["x@example.org".into()],
            })
        );
    }

    #[test]
    fn failed_canceled_and_missing_messages_are_refused() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        for state in ["failed", "canceled", "unverifiable"] {
            let id = queue(
                &db,
                Mail {
                    to: "x@example.org",
                    ..Default::default()
                },
            );
            set_state(&db, &id, state);
            let error = db.link_task_to_outbox(&id, &one, AT).unwrap_err();
            assert_eq!(error.to_string(), "That message wasn't sent", "{state}");
        }
        assert_eq!(
            db.link_task_to_outbox("missing", &one, AT)
                .unwrap_err()
                .to_string(),
            "That message wasn't sent"
        );
        assert_eq!(status(&db, &one), "open");
    }

    #[test]
    fn a_message_that_already_has_a_link_is_refused_and_keeps_it() {
        let db = setup();
        let first = task(&db, "First", None, None);
        let second = task(&db, "Second", None, None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        db.link_task_to_outbox(&id, &first, AT).unwrap();
        assert!(db.link_task_to_outbox(&id, &second, AT).is_err());
        let draft = db
            .outbox()
            .unwrap()
            .into_iter()
            .find(|i| i.id == id)
            .unwrap()
            .draft;
        assert_eq!(draft.completes_task_id.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn a_finished_or_missing_task_is_refused() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        db.set_task_status(&one, "completed", "user").unwrap();
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        assert!(db.link_task_to_outbox(&id, &one, AT).is_err());
        assert!(db.link_task_to_outbox(&id, "missing", AT).is_err());
    }

    #[test]
    fn linking_changes_only_the_stored_draft_not_the_message_that_is_sent() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        let before = db.outbox_raw(&id).unwrap();
        db.link_task_to_outbox(&id, &one, AT).unwrap();
        assert_eq!(db.outbox_raw(&id).unwrap(), before);
    }

    #[test]
    fn an_undone_send_after_linking_finishes_nothing() {
        let db = setup();
        let one = task(&db, "Send the contract", None, None);
        let id = queue(
            &db,
            Mail {
                to: "x@example.org",
                ..Default::default()
            },
        );
        db.link_task_to_outbox(&id, &one, AT).unwrap();
        db.cancel_send(&id, false).unwrap();
        assert_eq!(status(&db, &one), "open");
    }
}
