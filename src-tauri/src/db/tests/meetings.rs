use super::*;
use crate::db::test_support::{database, message};
use crate::models::ScheduleEvent;
use chrono::TimeZone;

const ME: &str = "me@example.com";
const CAL: &str = "work";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap()
}

fn event(
    id: &str,
    series: Option<&str>,
    title: &str,
    start: &str,
    attendees: &[&str],
) -> ScheduleEvent {
    serde_json::from_value(serde_json::json!({
        "id": id, "seriesId": series, "accountId": ME, "calendarId": CAL, "title": title,
        "start": start, "end": start.replace("T15:", "T16:"), "allDay": false,
        "attendees": attendees, "attendeeResponses": {}, "canRespond": false, "canEdit": true
    }))
    .unwrap()
}

fn occurrence(id: &str, series: Option<&str>) -> NoteReference {
    NoteReference {
        version: 1,
        kind: "event".into(),
        id: id.into(),
        account: Some(ME.into()),
        calendar: Some(CAL.into()),
        note: None,
        contact: None,
        series: series.map(Into::into),
    }
}

fn href(id: &str, series: Option<&str>) -> String {
    occurrence(id, series).href().unwrap()
}

/// A database with a mail and calendar account for ME and the events cached.
fn setup(events: &[ScheduleEvent]) -> Database {
    let db = database();
    db.adopt_account(ME).unwrap();
    db.adopt_calendar_account(ME).unwrap();
    db.cache_note_events(ME, &[], "", "", events).unwrap();
    db
}

fn context(db: &Database, reference: &NoteReference) -> MeetingContext {
    db.meeting_context(reference, None, now()).unwrap()
}

fn texts(section: &MeetingSection<MeetingNoteSource>) -> Vec<String> {
    section
        .items
        .iter()
        .map(|source| source.text.clone())
        .collect()
}

fn mail(
    id: &str,
    thread: &str,
    date: &str,
    from: &str,
    to: &[&str],
) -> crate::mime::NormalizedMessage {
    let mut message = message(id, thread, date, "Body");
    message.subject = format!("About {thread}");
    message.from = from.into();
    message.to = to.iter().map(|address| address.to_string()).collect();
    message
}

fn thread_task(db: &Database, account: &str, thread: &str, title: &str) -> String {
    db.create_task(
        &serde_json::from_value(serde_json::json!({
            "accountId": account, "threadId": format!("{account}:{thread}"),
            "subjectSnapshot": "Subject", "title": title, "kind": "action"
        }))
        .unwrap(),
    )
    .unwrap()
    .id
}

#[test]
fn this_meeting_shows_its_own_block_linked_notes_and_references_but_not_neighbors() {
    let db = setup(&[event("evt-1", None, "Sync", "2026-11-16T15:00:00Z", &[])]);
    db.create_note("Sync notes", "Running notes for the sync")
        .unwrap();
    db.save_daily_note(
        "2026-11-16",
        &format!(
            "## Meetings\n\n- [9:00am]({}) for Standup\n  - Private standup detail\n- [3:00pm]({}) for [[Sync notes]]\n  - Decided to ship\n\n## Journal\n\nPersonal text\n",
            href("evt-other", None),
            href("evt-1", None)
        ),
    )
    .unwrap();
    db.create_note(
        "Ideas",
        &format!(
            "Bring up [the sync]({}) with Pete. Also unrelated.",
            href("evt-1", None)
        ),
    )
    .unwrap();

    let result = context(&db, &occurrence("evt-1", None));
    assert_eq!(result.this_meeting.status, SectionStatus::Ready);
    let kinds: Vec<_> = result
        .this_meeting
        .items
        .iter()
        .map(|source| source.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            MeetingSourceKind::Excerpt,
            MeetingSourceKind::Reference,
            MeetingSourceKind::LinkedNote
        ]
    );
    let all = texts(&result.this_meeting).join("\n");
    assert!(all.contains("Decided to ship"));
    assert!(all.contains("Running notes for the sync"));
    assert!(!all.contains("Private standup detail"));
    assert!(!all.contains("Personal text"));
    assert_eq!(
        result.this_meeting.items[0].daily_date.as_deref(),
        Some("2026-11-16")
    );
}

#[test]
fn previous_meetings_are_earlier_occurrences_of_the_exact_series_newest_first() {
    let series = "series-1";
    let db = setup(&[
        event("occ-1", Some(series), "Weekly", "2026-11-02T15:00:00Z", &[]),
        event("occ-2", Some(series), "Weekly", "2026-11-09T15:00:00Z", &[]),
        event("occ-3", Some(series), "Weekly", "2026-11-16T15:00:00Z", &[]),
        event("occ-4", Some(series), "Weekly", "2026-11-23T15:00:00Z", &[]),
        // Same title, unrelated event.
        event("other", None, "Weekly", "2026-11-12T15:00:00Z", &[]),
    ]);
    for (date, id) in [
        ("2026-11-02", "occ-1"),
        ("2026-11-09", "occ-2"),
        ("2026-11-23", "occ-4"),
        ("2026-11-12", "other"),
    ] {
        let series = (id != "other").then_some(series);
        db.save_daily_note(
            date,
            &format!(
                "- [3pm]({}) for Weekly\n  - Notes from {id}\n",
                href(id, series)
            ),
        )
        .unwrap();
    }
    db.create_note(
        "About the series",
        &format!(
            "- [Weekly]({})\n  - Agenda for every week\n",
            href(series, None)
        ),
    )
    .unwrap();

    let result = context(&db, &occurrence("occ-3", Some(series)));
    let previous: Vec<&str> = result
        .previous_meetings
        .items
        .iter()
        .map(|item| item.reference.id.as_str())
        .collect();
    assert_eq!(previous, ["occ-2", "occ-1"]);
    assert!(result.previous_meetings.items[0].sources[0]
        .text
        .contains("Notes from occ-2"));
    assert_eq!(
        texts(&result.series_notes),
        ["- [Weekly](".to_string() + &href(series, None) + ")\n  - Agenda for every week"]
    );
    // The series' own notes and later occurrences are not this meeting's.
    assert!(result.this_meeting.items.is_empty());
    assert_eq!(
        result.event.as_ref().unwrap().series_id.as_deref(),
        Some(series)
    );
}

#[test]
fn an_uncached_occurrence_is_ordered_by_its_daily_note_and_a_same_day_one_is_left_out() {
    let series = "series-2";
    let this = event(
        "occ-now",
        Some(series),
        "Review",
        "2026-11-16T15:00:00Z",
        &[],
    );
    let this_date = instant(&this.start)
        .unwrap()
        .with_timezone(&Local)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let db = setup(&[this]);
    db.save_daily_note(
        "2026-11-01",
        &format!(
            "- [Review]({})\n  - Old notes\n",
            href("occ-old", Some(series))
        ),
    )
    .unwrap();
    db.save_daily_note(
        &this_date,
        &format!(
            "- [Review]({})\n  - Same day\n",
            href("occ-same", Some(series))
        ),
    )
    .unwrap();

    let result = context(&db, &occurrence("occ-now", Some(series)));
    let previous: Vec<_> = result
        .previous_meetings
        .items
        .iter()
        .map(|item| {
            (
                item.reference.id.clone(),
                item.start.clone(),
                item.date.clone(),
            )
        })
        .collect();
    assert_eq!(
        previous,
        [("occ-old".to_string(), None, Some("2026-11-01".to_string()))]
    );
}

#[test]
fn a_meeting_missing_from_the_cache_keeps_its_notes_and_explains_the_rest() {
    let db = setup(&[]);
    db.save_daily_note(
        "2026-11-16",
        &format!(
            "- [3pm]({})\n  - ( ) Send the deck\n",
            href("evt-gone", Some("s"))
        ),
    )
    .unwrap();

    let result = context(&db, &occurrence("evt-gone", Some("s")));
    assert!(result.event.is_none());
    assert_eq!(result.this_meeting.items.len(), 1);
    assert_eq!(result.previous_meetings.status, SectionStatus::Unavailable);
    assert_eq!(result.conversations.status, SectionStatus::Unavailable);
    assert_eq!(result.commitments.items.len(), 1);
    assert!(result.commitments.message.is_some());
}

#[test]
fn conversations_come_from_the_origin_message_and_recent_mail_with_external_attendees_in_scope() {
    let mut meeting = event(
        "evt-1",
        None,
        "Sync",
        "2026-11-16T15:00:00Z",
        &["ann@example.org", "ME@example.com", "bo@example.org"],
    );
    meeting.source_message_id = Some("origin-msg".into());
    let db = setup(&[meeting]);
    let other = "other@example.com";
    db.adopt_account(other).unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "origin-msg",
            "origin",
            "2026-11-10T09:00:00Z",
            "Cy <cy@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "a1",
            "with-ann",
            "2026-11-14T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "b1",
            "with-bo",
            "2026-11-12T09:00:00Z",
            "Bo <bo@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "old",
            "too-old",
            "2026-06-01T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        other,
        &[mail(
            "x1",
            "elsewhere",
            "2026-11-15T09:00:00Z",
            "Ann <ann@example.org>",
            &[other],
        )],
    )
    .unwrap();

    let result = context(&db, &occurrence("evt-1", None));
    assert_eq!(result.mail_scope, [ME]);
    assert_eq!(
        result.event.as_ref().unwrap().attendees,
        ["ann@example.org", "bo@example.org"]
    );
    let threads: Vec<_> = result
        .conversations
        .items
        .iter()
        .map(|item| (item.thread_id.clone(), item.reason))
        .collect();
    assert_eq!(
        threads,
        [
            (format!("{ME}:origin"), ConversationReason::Origin),
            (format!("{ME}:with-ann"), ConversationReason::Attendees),
            (format!("{ME}:with-bo"), ConversationReason::Attendees),
        ]
    );
    assert_eq!(result.conversations.items[1].attendees, ["ann@example.org"]);

    // Another account is searched only when chosen.
    let both = db
        .meeting_context(
            &occurrence("evt-1", None),
            Some(&[ME.to_string(), other.to_string()]),
            now(),
        )
        .unwrap();
    assert!(both
        .conversations
        .items
        .iter()
        .any(|item| item.thread_id == format!("{other}:elsewhere")));
    assert!(db
        .meeting_context(
            &occurrence("evt-1", None),
            Some(&["unknown@example.com".to_string()]),
            now()
        )
        .is_err());
}

#[test]
fn a_calendar_without_a_matching_mail_account_needs_a_scope_choice() {
    let db = database();
    db.adopt_calendar_account(ME).unwrap();
    db.cache_note_events(
        ME,
        &[],
        "",
        "",
        &[event(
            "evt-1",
            None,
            "Sync",
            "2026-11-16T15:00:00Z",
            &["ann@example.org"],
        )],
    )
    .unwrap();
    let result = context(&db, &occurrence("evt-1", None));
    assert!(result.mail_scope.is_empty());
    assert_eq!(result.conversations.status, SectionStatus::ScopeRequired);
    let explicit_empty = db
        .meeting_context(&occurrence("evt-1", None), Some(&[]), now())
        .unwrap();
    assert_eq!(
        explicit_empty.conversations.status,
        SectionStatus::ScopeRequired
    );
}

#[test]
fn commitments_are_open_tasks_in_the_meetings_blocks_and_conversations_with_attendee_tasks_as_candidates(
) {
    let mut meeting = event(
        "evt-1",
        None,
        "Sync",
        "2026-11-16T15:00:00Z",
        &["ann@example.org"],
    );
    meeting.source_message_id = Some("origin-msg".into());
    let db = setup(&[meeting]);
    db.upsert_thread(
        ME,
        &[mail(
            "origin-msg",
            "origin",
            "2026-11-10T09:00:00Z",
            "Cy <cy@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "a1",
            "with-ann",
            "2026-11-14T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    let origin_task = thread_task(&db, ME, "origin", "Reply to Cy");
    let candidate_task = thread_task(&db, ME, "with-ann", "Send Ann the numbers");
    let done = thread_task(&db, ME, "origin", "Already done");
    db.connection()
        .unwrap()
        .execute("UPDATE tasks SET status='completed' WHERE id=?1", [&done])
        .unwrap();
    let linked = db
        .create_task(&serde_json::from_value(serde_json::json!({"accountId": ME, "title": "Linked standalone", "kind": "action"})).unwrap())
        .unwrap();
    let task_link = NoteReference {
        version: 1,
        kind: "task".into(),
        id: linked.id.clone(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    };
    db.save_daily_note(
        "2026-11-16",
        &format!(
            "- [3pm]({})\n  - ( ) Send the deck\n  - (x) Book the room\n  - See [{}]({})\n- ( ) Unrelated chore\n",
            href("evt-1", None),
            linked.title,
            task_link.href().unwrap()
        ),
    )
    .unwrap();

    let result = context(&db, &occurrence("evt-1", None));
    let items: Vec<_> = result
        .commitments
        .items
        .iter()
        .map(|item| (item.title.as_str(), item.origin, item.candidate))
        .collect();
    assert_eq!(
        items,
        [
            ("Send the deck", CommitmentOrigin::MeetingNote, false),
            ("Linked standalone", CommitmentOrigin::MeetingNote, false),
            ("Reply to Cy", CommitmentOrigin::OriginConversation, false),
            (
                "Send Ann the numbers",
                CommitmentOrigin::AttendeeConversation,
                true
            ),
        ]
    );
    assert_eq!(result.commitments.items[2].id, origin_task);
    assert_eq!(result.commitments.items[3].id, candidate_task);
    assert!(result.commitments.items[0]
        .id
        .starts_with("note:daily:2026-11-16:"));
}

#[test]
fn readiness_counts_agree_with_the_full_context() {
    let series = "series-r";
    let mut meeting = event(
        "occ-2",
        Some(series),
        "Weekly",
        "2026-11-16T15:00:00Z",
        &["ann@example.org"],
    );
    meeting.source_message_id = Some("origin-msg".into());
    let db = setup(&[
        event("occ-1", Some(series), "Weekly", "2026-11-09T15:00:00Z", &[]),
        meeting,
    ]);
    db.upsert_thread(
        ME,
        &[mail(
            "origin-msg",
            "origin",
            "2026-11-10T09:00:00Z",
            "Cy <cy@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "a1",
            "with-ann",
            "2026-11-14T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    thread_task(&db, ME, "with-ann", "Candidate");
    db.save_daily_note(
        "2026-11-09",
        &format!("- [3pm]({})\n  - Last week\n", href("occ-1", Some(series))),
    )
    .unwrap();
    db.save_daily_note(
        "2026-11-16",
        &format!("- [3pm]({})\n  - ( ) Prep\n", href("occ-2", Some(series))),
    )
    .unwrap();

    let reference = occurrence("occ-2", Some(series));
    let full = context(&db, &reference);
    let counts = db
        .meeting_readiness(std::slice::from_ref(&reference), None, now())
        .unwrap()
        .remove(0);
    assert!(counts.event_available && counts.has_occurrence_note && !counts.scope_required);
    assert_eq!(
        counts.prior_occurrences_with_notes,
        full.previous_meetings.items.len()
    );
    assert_eq!(counts.prior_occurrences_with_notes, 1);
    assert_eq!(counts.open_commitments, 1);
    assert_eq!(counts.candidate_commitments, 1);
    assert_eq!(
        counts.latest_correspondence_at.as_deref(),
        Some("2026-11-14T09:00:00Z")
    );
    assert!(counts.unavailable_sections.is_empty() && counts.error.is_none());
}

#[test]
fn readiness_is_bounded_and_one_unreadable_meeting_does_not_fail_the_others() {
    let db = setup(&[event("evt-1", None, "Sync", "2026-11-16T15:00:00Z", &[])]);
    db.connection()
        .unwrap()
        .execute("INSERT INTO note_event_cache VALUES (?1,?2,'broken','2026-11-16','2026-11-16','not json')", [ME, CAL])
        .unwrap();
    let results = db
        .meeting_readiness(
            &[occurrence("evt-1", None), occurrence("broken", None)],
            None,
            now(),
        )
        .unwrap();
    assert!(results[0].error.is_none() && results[0].event_available);
    assert!(results[1].error.is_some());

    let at_limit: Vec<_> = (0..policy::MAX_READINESS_EVENTS)
        .map(|n| occurrence(&format!("e{n}"), None))
        .collect();
    assert_eq!(
        db.meeting_readiness(&at_limit, None, now()).unwrap().len(),
        policy::MAX_READINESS_EVENTS
    );
    let mut above = at_limit;
    above.push(occurrence("one-more", None));
    assert!(db.meeting_readiness(&above, None, now()).is_err());
    let not_an_event = NoteReference {
        kind: "task".into(),
        account: None,
        calendar: None,
        ..occurrence("x", None)
    };
    assert!(db.meeting_context(&not_an_event, None, now()).is_err());
}

#[test]
fn long_blocks_and_many_earlier_meetings_say_what_was_left_out() {
    let series = "series-l";
    let mut events = vec![event(
        "now",
        Some(series),
        "Weekly",
        "2026-11-16T15:00:00Z",
        &[],
    )];
    for week in 1..=(policy::MAX_PRIOR_OCCURRENCES + 1) {
        let date = Utc.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap()
            - chrono::Duration::weeks(week as i64);
        events.push(event(
            &format!("w{week}"),
            Some(series),
            "Weekly",
            &date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            &[],
        ));
    }
    let db = setup(&events);
    for week in 1..=(policy::MAX_PRIOR_OCCURRENCES + 1) {
        db.create_note(
            &format!("Week {week}"),
            &format!("- [Weekly]({})\n", href(&format!("w{week}"), Some(series))),
        )
        .unwrap();
    }
    let long = "x".repeat(policy::MAX_MEETING_EXCERPT_BYTES + 10);
    db.save_daily_note(
        "2026-11-16",
        &format!("- [3pm]({}) {long}\n", href("now", Some(series))),
    )
    .unwrap();

    let result = context(&db, &occurrence("now", Some(series)));
    assert_eq!(result.previous_meetings.status, SectionStatus::Limited);
    assert_eq!(
        result.previous_meetings.items.len(),
        policy::MAX_PRIOR_OCCURRENCES
    );
    assert_eq!(result.previous_meetings.omitted, 1);
    assert_eq!(result.previous_meetings.items[0].reference.id, "w1");
    let source = &result.this_meeting.items[0];
    assert!(source.truncated);
    assert_eq!(source.text.len(), policy::MAX_MEETING_EXCERPT_BYTES);
    assert_eq!(result.limits, policy::LIMITS);
}

fn prep_entry(id: &str) -> crate::note_meetings::MeetingEntry {
    crate::note_meetings::MeetingEntry {
        event: occurrence(id, None),
        time: "3:00pm".into(),
        attendees: Vec::new(),
        name: "Sync".into(),
        link_note: false,
    }
}

#[test]
fn reserving_prep_writes_the_meeting_line_and_one_prep_task_and_repeating_reuses_it() {
    let db = setup(&[]);
    let first = db
        .reserve_meeting_prep("2026-11-16", None, &prep_entry("evt-1"))
        .unwrap();
    assert!(first.added && first.added_meeting_line);
    assert_eq!(first.previous_body, "");
    assert!(first.note.body.contains("  - ( ) Prepare for Sync"));
    let task_id = first.task_id.clone().unwrap();
    assert!(db
        .list_note_tasks()
        .unwrap()
        .iter()
        .any(|task| task.id == task_id && !task.checked));

    let again = db
        .reserve_meeting_prep(
            "2026-11-16",
            Some(&first.note.updated_at),
            &prep_entry("evt-1"),
        )
        .unwrap();
    assert!(!again.added && !again.added_meeting_line);
    assert_eq!(again.task_id, Some(task_id));
    assert_eq!(again.note.body, first.note.body);
    assert_eq!(again.note.updated_at, first.note.updated_at);

    // The context counts it as this meeting's open commitment.
    let result = context(&db, &occurrence("evt-1", None));
    assert!(result
        .commitments
        .items
        .iter()
        .any(|item| item.title == "Prepare for Sync"));
}

#[test]
fn reserving_prep_goes_under_an_existing_meeting_line_and_refuses_stale_or_unbounded_notes() {
    let db = setup(&[]);
    let saved = db
        .save_daily_note(
            "2026-11-16",
            &format!(
                "## Meetings\n\n- [3pm]({}) for Sync\n- Other\n",
                href("evt-1", None)
            ),
        )
        .unwrap()
        .unwrap();
    assert!(db
        .reserve_meeting_prep("2026-11-16", Some("stale"), &prep_entry("evt-1"))
        .is_err());
    assert_eq!(db.get_note(&saved.id).unwrap().unwrap().body, saved.body);

    let reserved = db
        .reserve_meeting_prep("2026-11-16", Some(&saved.updated_at), &prep_entry("evt-1"))
        .unwrap();
    assert!(reserved.added && !reserved.added_meeting_line);
    assert!(reserved.note.body.starts_with(&format!(
        "## Meetings\n\n- [3pm]({}) for Sync\n  - ( ) Prepare for Sync",
        href("evt-1", None)
    )));
    assert!(reserved.note.body.contains("\n- Other\n"));

    let prose = db
        .save_daily_note(
            "2026-11-17",
            &format!("Talked about [the sync]({}).\n", href("evt-2", None)),
        )
        .unwrap()
        .unwrap();
    assert!(db
        .reserve_meeting_prep("2026-11-17", Some(&prose.updated_at), &prep_entry("evt-2"))
        .is_err());
    assert_eq!(db.get_note(&prose.id).unwrap().unwrap().body, prose.body);
}

#[test]
fn related_conversations_name_the_meeting_share_a_title_word_or_include_every_attendee() {
    let meeting = event(
        "evt-1",
        None,
        "Benefits Review",
        "2026-11-16T15:00:00Z",
        &["ann@example.org", "bo@example.org"],
    );
    let db = setup(&[meeting]);
    let mut invitation = mail(
        "inv",
        "invitation",
        "2026-11-12T09:00:00Z",
        "Ann <ann@example.org>",
        &[ME],
    );
    invitation.subject = "Invitation: Benefits review @ Mon Nov 16".into();
    db.upsert_thread(ME, &[invitation]).unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "solo",
            "ann-only",
            "2026-11-15T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME],
        )],
    )
    .unwrap();
    let mut topic = mail(
        "topic",
        "enrollment",
        "2026-11-11T09:00:00Z",
        "Bo <bo@example.org>",
        &[ME],
    );
    topic.subject = "Re: benefits enrollment forms".into();
    db.upsert_thread(ME, &[topic]).unwrap();
    db.upsert_thread(
        ME,
        &[mail(
            "both",
            "ann-and-bo",
            "2026-11-13T09:00:00Z",
            "Ann <ann@example.org>",
            &[ME, "bo@example.org"],
        )],
    )
    .unwrap();
    // A generic shared word, such as "review", isn't enough.
    let mut generic = mail(
        "gen",
        "generic",
        "2026-11-14T09:00:00Z",
        "Bo <bo@example.org>",
        &[ME],
    );
    generic.subject = "Code review for the release".into();
    db.upsert_thread(ME, &[generic]).unwrap();

    let result = context(&db, &occurrence("evt-1", None));
    let threads: Vec<_> = result
        .conversations
        .items
        .iter()
        .map(|item| {
            (
                item.thread_id
                    .trim_start_matches(&format!("{ME}:"))
                    .to_string(),
                item.reason,
                item.related,
            )
        })
        .collect();
    assert_eq!(
        threads,
        [
            ("invitation".to_string(), ConversationReason::Title, true),
            (
                "ann-and-bo".to_string(),
                ConversationReason::Attendees,
                true
            ),
            (
                "enrollment".to_string(),
                ConversationReason::Attendees,
                true
            ),
            ("ann-only".to_string(), ConversationReason::Attendees, false),
            ("generic".to_string(), ConversationReason::Attendees, false),
        ]
    );
    assert_eq!(
        result.conversations.items[1].attendees,
        ["ann@example.org", "bo@example.org"]
    );

    // A title too short to be telling matches nothing by subject.
    let short = event("evt-2", None, "1:1", "2026-11-16T15:00:00Z", &[]);
    db.cache_note_events(ME, &[], "", "", &[short]).unwrap();
    assert!(context(&db, &occurrence("evt-2", None))
        .conversations
        .items
        .is_empty());
}

#[test]
fn step_a_carries_over_prior_tasks_promotes_group_threads_and_surfaces_promises_files_and_related_notes(
) {
    let mut prev = event(
        "evt-prev",
        Some("series-mkt"),
        "Marketing Power Hour",
        "2026-11-09T14:00:00Z",
        &[
            "taylor@example.org",
            "hiede@example.org",
            "nick@example.org",
        ],
    );
    let mut curr = event(
        "evt-curr",
        Some("series-mkt"),
        "Marketing Power Hour",
        "2026-11-16T14:00:00Z",
        &[
            "taylor@example.org",
            "hiede@example.org",
            "nick@example.org",
        ],
    );
    curr.description = Some("<p>Agenda: Q4 launch</p><br>-::~:~::~:~:~:~<br>Join Zoom".into());
    prev.description = None;
    let db = setup(&[prev, curr]);

    // Prior occurrence has an open task that should carry forward into Open commitments.
    db.save_daily_note(
        "2026-11-09",
        &format!(
            "- [2pm]({})\n  - ( ) Confirm ad budget with Taylor\n",
            href("evt-prev", Some("series-mkt"))
        ),
    )
    .unwrap();

    // A thread with 2 of 3 group attendees should be promoted to related = true, with snippet + attachment.
    let mut group_msg = mail(
        "m-group",
        "t-group",
        "2026-11-14T10:00:00Z",
        "Taylor <taylor@example.org>",
        &[ME, "hiede@example.org"],
    );
    group_msg.subject = "Q4 campaign creative".into();
    group_msg.body_text = "Here is the updated creative deck for Monday".into();
    group_msg.snippet = "Here is the updated creative deck for Monday".into();
    group_msg.attachments = vec![crate::models::MessageAttachment {
        id: "att-deck".into(),
        filename: "Q4-Creative.pdf".into(),
        mime_type: "application/pdf".into(),
        size: 4096,
        content_id: None,
        inline: false,
    }];
    db.upsert_thread(ME, &[group_msg]).unwrap();

    // An open promise sent by the user to Nick should appear in promises.
    let mut promise_msg = mail(
        "m-promise",
        "t-promise",
        "2026-11-15T09:00:00Z",
        ME,
        &["nick@example.org"],
    );
    promise_msg.subject = "Landing page metrics".into();
    promise_msg.body_text = "I will send the conversion numbers before Monday's meeting.".into();
    db.upsert_thread(ME, &[promise_msg]).unwrap();

    // A standalone note referencing Taylor or sharing a title word should appear in related_notes.
    db.create_note("Marketing Q4 goals", "Focus on partner webinars")
        .unwrap();

    let result = context(&db, &occurrence("evt-curr", Some("series-mkt")));
    assert_eq!(
        result.event.as_ref().unwrap().description.as_deref(),
        Some("Agenda: Q4 launch")
    );
    assert!(result
        .commitments
        .items
        .iter()
        .any(|c| c.title == "Confirm ad budget with Taylor"
            && c.origin == CommitmentOrigin::PreviousMeetingNote
            && !c.candidate));
    let group_conv = result
        .conversations
        .items
        .iter()
        .find(|c| c.thread_id == format!("{ME}:t-group"))
        .expect("group thread included");
    assert!(group_conv.related);
    assert_eq!(
        group_conv.snippet.as_deref(),
        Some("Here is the updated creative deck for Monday")
    );
    assert!(result
        .files
        .items
        .iter()
        .any(|f| f.filename == "Q4-Creative.pdf" && f.source == "mail"));
    assert!(result
        .promises
        .items
        .iter()
        .any(|p| p.thread_id == format!("{ME}:t-promise")
            && p.excerpt.contains("conversion numbers")));
    assert!(result
        .related_notes
        .items
        .iter()
        .any(|n| n.title == "Marketing Q4 goals"));
}

#[test]
fn an_attendee_is_found_in_headers_only_as_a_whole_address() {
    assert!(mentions_address("ann <ann@example.org>", "ann@example.org"));
    assert!(mentions_address(
        "[\"ann@example.org\",\"bo@example.org\"]",
        "bo@example.org"
    ));
    assert!(!mentions_address("joann@example.org", "ann@example.org"));
    assert!(!mentions_address("ann@example.org.uk", "ann@example.org"));
    assert!(!mentions_address("ann@example.organic", "ann@example.org"));
}
