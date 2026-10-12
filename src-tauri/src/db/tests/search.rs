use super::*;
use crate::db::test_support::{database, message};
use crate::db::threads::local_thread_id;
use crate::mime::NormalizedMessage;
use crate::models::{SearchThreadsRequest, ThreadMutation};
use rusqlite::params;

#[test]
fn searches_local_fts_index() {
    let result = database()
        .search_threads(
            &SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(result[0].id, "welcome");
    assert!(result[0].match_snippet.is_some());
}

#[test]
fn search_rows_index_repeated_quoted_history_once() {
    let database = database();
    database
        .upsert_thread("you@example.com", &quoting_thread_messages())
        .unwrap();
    let thread_id = local_thread_id("you@example.com", "quoting");
    let body = search_body(&database, &thread_id);
    assert_eq!(body.matches("nightly feed import").count(), 1, "{body}");
    assert!(body.starts_with(QUOTED_ORIGINAL), "{body}");
    assert!(body.contains("hourly retry"));
    for query in ["nightly", "\"west region accounts\"", "hourly"] {
        assert_eq!(
            search_ids(&database, query),
            vec![thread_id.clone()],
            "{query}"
        );
    }
    // The provider snippet of the reply would run into its quote.
    assert_eq!(
        search_column(&database, "snippet", &thread_id),
        "Fixed now, the hourly retry handles it."
    );
    let found = database
        .search_threads(
            &SearchThreadsRequest {
                query: "nightly".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert!(found[0]
        .match_snippet
        .as_deref()
        .is_some_and(|snippet| snippet.contains("\u{1}nightly\u{2}")));
}

#[test]
fn inbox_preview_shows_the_latest_message_without_its_quote_entity_encoded() {
    let database = database();
    let mut messages = quoting_thread_messages();
    messages[0].body_text =
        format!("Fixed <now> & \"done\" — it's live.\n\nOn Thu, A wrote:\n> {QUOTED_ORIGINAL}");
    messages[0].snippet = "Provider preview that runs into On Thu, A wrote: &gt; Can you".into();
    database
        .upsert_thread("you@example.com", &messages)
        .unwrap();
    let thread_id = local_thread_id("you@example.com", "quoting");
    assert_eq!(
        list_snippet(&database, &thread_id),
        "Fixed &lt;now&gt; &amp; &quot;done&quot; — it&#39;s live."
    );
    let listed = database.list_all_mail(Some("you@example.com")).unwrap();
    assert_eq!(
        listed
            .iter()
            .find(|thread| thread.id == thread_id)
            .unwrap()
            .snippet,
        list_snippet(&database, &thread_id)
    );

    let mut blank = message("blank", "blank-thread", "2026-10-02T09:00:00Z", "");
    blank.snippet = "Provider &amp; preview".into();
    database.upsert_thread("you@example.com", &[blank]).unwrap();
    assert_eq!(
        list_snippet(
            &database,
            &local_thread_id("you@example.com", "blank-thread")
        ),
        "Provider &amp; preview"
    );
}

#[test]
fn inbox_preview_decodes_entity_references_in_plain_text_bodies() {
    let database = database();
    let fixtures = [
        (
            "padding",
            "Your claim &#847; &#847;&zwnj;&nbsp; is ready",
            "Your claim \u{34F} \u{34F}\u{200C}\u{A0} is ready",
        ),
        (
            "mixed",
            "R&amp;D at AT&T &#x2014; &bogus; & &#;",
            "R&amp;D at AT&amp;T \u{2014} &amp;bogus; &amp; &amp;#;",
        ),
    ];
    for (thread, body, preview) in fixtures {
        database
            .upsert_thread(
                "you@example.com",
                &[message(thread, thread, "2026-10-02T09:00:00Z", body)],
            )
            .unwrap();
        assert_eq!(
            list_snippet(&database, &local_thread_id("you@example.com", thread)),
            preview,
            "{thread}"
        );
    }
}

#[test]
fn search_snippet_falls_back_to_the_provider_snippet_for_a_message_without_text() {
    let database = database();
    let mut latest = message("blank", "blank-thread", "2026-10-02T09:00:00Z", "");
    latest.snippet = "Provider preview text".into();
    database
        .upsert_thread("you@example.com", &[latest])
        .unwrap();
    let thread_id = local_thread_id("you@example.com", "blank-thread");
    assert_eq!(
        search_column(&database, "snippet", &thread_id),
        "Provider preview text"
    );
    assert_eq!(search_ids(&database, "preview"), vec![thread_id]);
}

#[test]
fn queued_search_rows_are_rewritten_in_batches_and_fresh_rows_are_dequeued() {
    let database = database();
    database
        .upsert_thread("you@example.com", &quoting_thread_messages())
        .unwrap();
    let thread_id = local_thread_id("you@example.com", "quoting");
    let full = format!("{QUOTED_ORIGINAL} Fixed now, the hourly retry handles it.\n\nOn Thu, A wrote:\n> {QUOTED_ORIGINAL} ");
    {
        // Simulate a row written before v46, queued by the migration, plus
        // a queued thread whose row is gone.
        let connection = database.connection().unwrap();
        connection
            .execute(
                "UPDATE thread_search SET body = ?1, snippet = ?1 WHERE thread_id = ?2",
                params![full, thread_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE threads SET snippet = ?1 WHERE id = ?2",
                params![full, thread_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pending_search_reindex(thread_id) VALUES (?1), ('deleted-thread')",
                [&thread_id],
            )
            .unwrap();
    }
    assert_eq!(database.reindex_next_search_batch(1).unwrap(), 1);
    assert_eq!(database.reindex_next_search_batch(10).unwrap(), 1);
    assert_eq!(database.reindex_next_search_batch(10).unwrap(), 0);
    assert_eq!(
        search_body(&database, &thread_id)
            .matches("nightly feed import")
            .count(),
        1
    );
    assert_eq!(
        search_column(&database, "snippet", &thread_id),
        "Fixed now, the hourly retry handles it."
    );
    assert_eq!(
        list_snippet(&database, &thread_id),
        "Fixed now, the hourly retry handles it."
    );
    assert_eq!(search_ids(&database, "nightly"), vec![thread_id.clone()]);

    database
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO pending_search_reindex(thread_id) VALUES (?1)",
            [&thread_id],
        )
        .unwrap();
    database
        .upsert_thread("you@example.com", &quoting_thread_messages())
        .unwrap();
    assert_eq!(database.reindex_next_search_batch(10).unwrap(), 0);
}

#[test]
fn html_stored_as_plain_text_is_read_and_previewed_as_text() {
    // Rows synced before plain-text parts holding HTML were flattened.
    let fixtures = [
        ("document", "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html\"></head><body><p style=\"color: red\">Hi Parents, we need help Saturday.</p></body></html>", "Hi Parents, we need help Saturday."),
        ("fragment", "<div dir=\"ltr\">Lunch at <b>noon</b> &amp; after?</div>", "Lunch at noon &amp; after?"),
    ];
    let database = database();
    for (thread, body, preview) in fixtures {
        database
            .upsert_thread(
                "you@example.com",
                &[message(thread, thread, "2026-10-02T09:00:00Z", body)],
            )
            .unwrap();
        let thread_id = local_thread_id("you@example.com", thread);
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO pending_search_reindex(thread_id) VALUES (?1)",
                [&thread_id],
            )
            .unwrap();
        while database.reindex_next_search_batch(10).unwrap() > 0 {}
        assert_eq!(list_snippet(&database, &thread_id), preview, "{thread}");
        assert!(
            !search_body(&database, &thread_id).contains('<'),
            "{thread}"
        );
        let detail = database.get_thread(&thread_id).unwrap();
        assert!(!detail.messages[0].body_text.contains('<'), "{thread}");
    }
}

#[test]
fn inbox_previews_and_cached_text_omit_html_styles_after_ingestion_and_upgrade() {
    use crate::mime::{normalize, MimeBody, MimePart, RawMessage};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let database = database();
    let fixtures = [
        ("document", "<html><head data-layout=email><style type=text/css>@media screen and (max-width: 767px) { table tbody { display: block; } }</style></head><body><table><tr><td>Your appointment is reserved.</td></tr></table></body></html>"),
        ("fragment", "<div><style media=screen data-note='a > b'>.card > p { color: red; }</style><p>Your appointment is reserved.</p><script type=text/javascript>alert('hidden')</script></div>"),
    ];
    for (id, html) in fixtures {
        let raw = RawMessage {
            id: id.into(),
            thread_id: id.into(),
            internal_date: "0".into(),
            label_ids: vec!["INBOX".into()],
            snippet: "Provider preview".into(),
            payload: MimePart {
                mime_type: "text/html".into(),
                body: MimeBody {
                    data: Some(URL_SAFE_NO_PAD.encode(html)),
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        let normalized = normalize(&raw).unwrap();
        database
            .upsert_thread("you@example.com", &[normalized])
            .unwrap();
        let thread_id = local_thread_id("you@example.com", id);
        assert_eq!(
            list_snippet(&database, &thread_id),
            "Your appointment is reserved."
        );

        // Simulate a previous release's cached, already flattened CSS text.
        // Exercise both legacy uncompressed and current compressed storage.
        let leaked =
            "@media screen { table tbody { display: block; } } Your appointment is reserved.";
        {
            let connection = database.connection().unwrap();
            if id == "document" {
                connection
                    .execute(
                        "UPDATE messages SET body_text = ?1, body_text_z = NULL WHERE id = ?2",
                        params![leaked, id],
                    )
                    .unwrap();
            } else {
                connection
                    .execute(
                        "UPDATE messages SET body_text = '', body_text_z = ?1 WHERE id = ?2",
                        params![crate::db::messages::compress_body(leaked), id],
                    )
                    .unwrap();
            }
            connection
                .execute(
                    "UPDATE threads SET snippet = ?1 WHERE id = ?2",
                    params![leaked, thread_id],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO pending_search_reindex(thread_id) VALUES (?1)",
                    [&thread_id],
                )
                .unwrap();
        }
        assert_eq!(database.reindex_next_search_batch(1).unwrap(), 1);
        assert_eq!(database.reindex_next_search_batch(1).unwrap(), 0);
        assert_eq!(
            list_snippet(&database, &thread_id),
            "Your appointment is reserved."
        );
        assert_eq!(
            search_column(&database, "snippet", &thread_id),
            "Your appointment is reserved."
        );
        assert_eq!(
            search_body(&database, &thread_id).trim(),
            "Your appointment is reserved."
        );
        let detail = database.get_thread(&thread_id).unwrap();
        assert_eq!(
            detail.messages[0].body_text,
            "Your appointment is reserved."
        );
        assert_eq!(detail.messages[0].body_html, html);
    }
    assert!(search_ids(&database, "media").is_empty());
    assert!(search_ids(&database, "hidden").is_empty());
    assert_eq!(search_ids(&database, "appointment").len(), 2);
}

#[test]
fn cached_text_repair_keeps_the_senders_plain_text_alternative() {
    use crate::mime::{normalize, MimeBody, MimePart, RawMessage};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let plain = "Please use @media screen { p { color: red; } } in the stylesheet.";
    let raw = RawMessage {
        id: "plain-css".into(),
        thread_id: "plain-css".into(),
        internal_date: "0".into(),
        label_ids: vec!["INBOX".into()],
        snippet: String::new(),
        payload: MimePart {
            mime_type: "multipart/alternative".into(),
            parts: [
                ("text/plain", plain),
                (
                    "text/html",
                    "<style type=text/css>p { color: red; }</style><p>Different HTML text</p>",
                ),
            ]
            .into_iter()
            .map(|(kind, body)| MimePart {
                mime_type: kind.into(),
                body: MimeBody {
                    data: Some(URL_SAFE_NO_PAD.encode(body)),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        },
    };
    let database = database();
    database
        .upsert_thread("you@example.com", &[normalize(&raw).unwrap()])
        .unwrap();
    let thread_id = local_thread_id("you@example.com", "plain-css");
    database
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO pending_search_reindex(thread_id) VALUES (?1)",
            [&thread_id],
        )
        .unwrap();
    assert_eq!(database.reindex_next_search_batch(1).unwrap(), 1);
    assert_eq!(list_snippet(&database, &thread_id), plain);
    assert_eq!(
        database.get_thread(&thread_id).unwrap().messages[0].body_text,
        plain
    );
}

#[test]
fn chat_search_matches_any_word_and_skips_the_open_and_trashed_conversations() {
    let database = database();
    let words = |list: &[&str]| list.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    let found = database
        .chat_search_thread_ids(&words(&["keyboard", "nosuchword"]), "none", 10)
        .unwrap();
    assert!(found.contains(&"welcome".to_string()));
    assert!(!database
        .chat_search_thread_ids(&words(&["keyboard"]), "welcome", 10)
        .unwrap()
        .contains(&"welcome".to_string()));
    assert!(database
        .chat_search_thread_ids(&[], "none", 10)
        .unwrap()
        .is_empty());
    assert!(database
        .chat_search_thread_ids(&words(&["keyboard\" OR \"x"]), "none", 10)
        .unwrap()
        .is_empty());
    assert!(database
        .chat_search_thread_ids(&words(&["keyboard"]), "none", 0)
        .unwrap()
        .is_empty());
    database
        .mutate_thread(&ThreadMutation::Trash {
            thread_id: "welcome".into(),
            value: true,
        })
        .unwrap();
    assert!(!database
        .chat_search_thread_ids(&words(&["keyboard"]), "none", 10)
        .unwrap()
        .contains(&"welcome".to_string()));
}

#[test]
fn search_matches_numeric_tokens_in_archived_body_text() {
    let database = database();
    let mut archived = message(
        "numeric-message",
        "numeric-thread",
        "2026-01-01T00:00:00Z",
        "Historical reference 126",
    );
    archived.labels.clear();
    database.upsert_thread("default", &[archived]).unwrap();

    let matches = database
        .search_threads(
            &SearchThreadsRequest {
                query: "126".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            },
            Some("default"),
        )
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].provider_thread_id, "numeric-thread");
}

#[test]
fn phrase_search_requires_contiguous_words() {
    let database = database();
    let matches = database
        .search_threads(
            &SearchThreadsRequest {
                query: "\"keeps your mail\"".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(matches[0].id, "welcome");

    let no_matches = database
        .search_threads(
            &SearchThreadsRequest {
                query: "\"mail your keeps\"".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert!(no_matches.is_empty());
}

#[test]
fn search_excludes_archived_unless_requested() {
    let database = database();
    database
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: "welcome".into(),
            value: true,
        })
        .unwrap();

    let hidden = database
        .search_threads(
            &SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert!(hidden.is_empty());

    let shown = database
        .search_threads(
            &SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            },
            None,
        )
        .unwrap();
    assert_eq!(shown[0].id, "welcome");
}

#[test]
fn search_supports_offset_pagination() {
    let database = database();
    for (id, date) in [
        ("alpha", "2026-01-01T00:00:00Z"),
        ("beta", "2026-01-02T00:00:00Z"),
    ] {
        database
            .upsert_thread(
                "default",
                &[NormalizedMessage {
                    id: format!("{id}-message"),
                    thread_id: id.into(),
                    subject: "Pagination test".into(),
                    from: "sender@example.com".into(),
                    to: vec!["recipient@example.com".into()],
                    date: date.into(),
                    body_html: String::new(),
                    body_text: "unique-pagination-term".into(),
                    snippet: "unique-pagination-term".into(),
                    labels: vec!["INBOX".into()],
                    metadata_json: "{}".into(),
                    unsubscribe: None,
                    attachments: vec![],
                }],
            )
            .unwrap();
    }

    let first_page = database
        .search_threads(
            &SearchThreadsRequest {
                query: "unique-pagination-term".into(),
                limit: Some(1),
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    let second_page = database
        .search_threads(
            &SearchThreadsRequest {
                query: "unique-pagination-term".into(),
                limit: Some(1),
                offset: Some(1),
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(second_page.len(), 1);
    assert_ne!(first_page[0].id, second_page[0].id);
}

#[test]
fn search_results_are_ordered_by_recency_over_relevance() {
    let database = database();
    // The older thread repeats the query term, which FTS5's bm25 rank
    // would normally score as more relevant than a single mention — but
    // recency should still win, since a newer email is more likely to be
    // what the user is looking for.
    database
        .upsert_thread(
            "default",
            &[message(
                "old-message",
                "old-thread",
                "2026-01-01T00:00:00Z",
                "recency-sort-term recency-sort-term recency-sort-term",
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            "default",
            &[message(
                "new-message",
                "new-thread",
                "2026-02-01T00:00:00Z",
                "recency-sort-term",
            )],
        )
        .unwrap();

    let matches = database
        .search_threads(
            &SearchThreadsRequest {
                query: "recency-sort-term".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            Some("default"),
        )
        .unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].provider_thread_id, "new-thread");
    assert_eq!(matches[1].provider_thread_id, "old-thread");
}

fn quoting_thread_messages() -> Vec<NormalizedMessage> {
    // Out of order on purpose: the search row must follow message dates.
    vec![
        message(
            "reply",
            "quoting",
            "2026-10-02T09:00:00Z",
            &format!(
                "Fixed now, the hourly retry handles it.\n\nOn Thu, A wrote:\n> {QUOTED_ORIGINAL}"
            ),
        ),
        message(
            "original",
            "quoting",
            "2026-10-01T09:00:00Z",
            QUOTED_ORIGINAL,
        ),
    ]
}

fn search_column(database: &Database, column: &str, thread_id: &str) -> String {
    database
        .connection()
        .unwrap()
        .query_row(
            &format!("SELECT {column} FROM thread_search WHERE thread_id = ?1"),
            [thread_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn search_body(database: &Database, thread_id: &str) -> String {
    search_column(database, "body", thread_id)
}

fn search_ids(database: &Database, query: &str) -> Vec<String> {
    database
        .search_threads(
            &SearchThreadsRequest {
                query: query.into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap()
        .into_iter()
        .map(|thread| thread.id)
        .collect()
}

fn list_snippet(database: &Database, thread_id: &str) -> String {
    database
        .connection()
        .unwrap()
        .query_row(
            "SELECT snippet FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .unwrap()
}

const QUOTED_ORIGINAL: &str =
    "Can you check whether the nightly feed import still fails for the west region accounts?";

// ---------------------------------------------------------------------------
// Phase 5 run 2a: operator-aware local search.
//
// CHARACTERISATION TESTS — these record TODAY's behaviour BEFORE the operator
// path lands. On the unchanged code every operator query is handed verbatim to
// `fts_query`, so `from:alice` becomes the adjacent-token prefix phrase
// `"from:alice"*` and matches NOTHING, even when a message from alice exists;
// the bare word `alice` DOES match it. When the structured path lands, the
// operator assertions below are updated on purpose (listed as deliberate
// contract changes in the report); the plain-word ones must NOT change.
// ---------------------------------------------------------------------------

/// A thread whose only message is from alice to bob, body "budget review".
fn alice_thread(database: &Database) -> String {
    // Exact-count assertions below: drop the seeded demo threads first.
    crate::db::test_support::clear_seed_threads(database);
    database
        .upsert_thread(
            "default",
            &[NormalizedMessage {
                id: "alice-message".into(),
                thread_id: "alice-thread".into(),
                subject: "Quarterly planning".into(),
                from: "alice@example.com".into(),
                to: vec!["bob@example.com".into()],
                date: "2024-06-15T09:00:00Z".into(),
                body_html: String::new(),
                body_text: "budget review".into(),
                snippet: "budget review".into(),
                labels: vec!["INBOX".into(), "UNREAD".into()],
                metadata_json: "{}".into(),
                unsubscribe: None,
                attachments: vec![],
            }],
        )
        .unwrap();
    local_thread_id("default", "alice-thread")
}

#[test]
fn characterisation_operator_from_matches_the_sender_now() {
    let database = database();
    let thread_id = alice_thread(&database);
    // NEW CONTRACT (2a): `from:alice` now matches the message from alice via
    // the structured path (was empty before run 2a).
    assert_eq!(search_ids(&database, "from:alice"), vec![thread_id.clone()]);
    // UNCHANGED: the bare word still matches the sender indexed in
    // `participants`.
    assert_eq!(search_ids(&database, "alice"), vec![thread_id]);
}

#[test]
fn characterisation_is_unread_subject_and_dates_match_now() {
    let database = database();
    let thread_id = alice_thread(&database);
    // NEW CONTRACT (2a): these operators now resolve locally. The fixture
    // thread is unread (its message has no explicit read flag → unread), dated
    // 2024-06-15, subject "Quarterly planning".
    assert_eq!(search_ids(&database, "is:unread"), vec![thread_id.clone()]);
    assert_eq!(search_ids(&database, "before:2025/01/01"), vec![thread_id.clone()]);
    assert!(search_ids(&database, "before:2024/01/01").is_empty());
    assert_eq!(search_ids(&database, "subject:Quarterly"), vec![thread_id]);
    // UNCHANGED: the bare subject word still matches the indexed subject.
    assert_eq!(search_ids(&database, "Quarterly").len(), 1);
}

// ---------------------------------------------------------------------------
// Phase 5 run 2a: operator-aware local search — behavioural tests.
// ---------------------------------------------------------------------------

use crate::models::MessageAttachment;
use crate::db::test_support::table_rows;

/// A message with explicit sender, recipients, date, labels and attachments,
/// so the operator tests can target each field precisely.
#[allow(clippy::too_many_arguments)]
fn op_message(
    id: &str,
    thread_id: &str,
    subject: &str,
    from: &str,
    to: Vec<&str>,
    date: &str,
    body: &str,
    labels: Vec<&str>,
    attachments: Vec<MessageAttachment>,
) -> NormalizedMessage {
    NormalizedMessage {
        id: id.into(),
        thread_id: thread_id.into(),
        subject: subject.into(),
        from: from.into(),
        to: to.into_iter().map(Into::into).collect(),
        date: date.into(),
        body_html: String::new(),
        body_text: body.into(),
        snippet: body.into(),
        labels: labels.into_iter().map(Into::into).collect(),
        metadata_json: "{}".into(),
        unsubscribe: None,
        attachments,
    }
}

fn attachment() -> MessageAttachment {
    MessageAttachment {
        id: "att-1".into(),
        filename: "report.pdf".into(),
        mime_type: "application/pdf".into(),
        size: 1024,
        content_id: None,
        inline: false,
    }
}

/// Two accounts, several threads, so unified and single-account searches and
/// each operator can be asserted. Returns the (account, thread_id) handles.
fn operator_fixture(database: &Database) {
    // Exact-set assertions below: drop the seeded demo threads first.
    crate::db::test_support::clear_seed_threads(database);
    // Account "work": a thread from alice to bob, unread, with an attachment,
    // dated 2024-06-15, subject "Quarterly budget".
    database
        .upsert_thread(
            "work",
            &[op_message(
                "w1",
                "w-alice",
                "Quarterly budget",
                "alice@example.com",
                vec!["bob@example.com", "carol@example.com"],
                "2024-06-15T12:00:00Z",
                "budget review notes",
                vec!["INBOX", "UNREAD"],
                vec![attachment()],
            )],
        )
        .unwrap();
    // Account "work": a thread from dave, starred, read, no attachment, dated
    // 2024-02-01, subject "Lunch plans".
    database
        .upsert_thread(
            "work",
            &[op_message(
                "w2",
                "w-dave",
                "Lunch plans",
                "dave@example.com",
                vec!["alice@example.com"],
                "2024-02-01T09:00:00Z",
                "where to eat",
                vec!["INBOX", "STARRED"],
                vec![],
            )],
        )
        .unwrap();
    // Account "home": a thread from alice (same display local-part, different
    // account) to erin, read, dated 2023-12-20, subject "Holiday".
    database
        .upsert_thread(
            "home",
            &[op_message(
                "h1",
                "h-alice",
                "Holiday",
                "alice@example.com",
                vec!["erin@example.com"],
                "2023-12-20T18:00:00Z",
                "trip itinerary",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
}

fn ids_scoped(database: &Database, query: &str, account: Option<&str>) -> Vec<String> {
    let mut ids = database
        .search_threads(
            &SearchThreadsRequest {
                query: query.into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            account,
        )
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

fn tid(account: &str, provider_thread: &str) -> String {
    local_thread_id(account, provider_thread)
}

#[test]
fn operator_from_matches_the_sender_across_accounts_and_scoped() {
    let database = database();
    operator_fixture(&database);
    // Unified: both alice-sent threads (one per account).
    assert_eq!(
        ids_scoped(&database, "from:alice", None),
        vec![tid("home", "h-alice"), tid("work", "w-alice")]
    );
    // Single-account scopes to just that account's thread.
    assert_eq!(
        ids_scoped(&database, "from:alice", Some("work")),
        vec![tid("work", "w-alice")]
    );
    // Substring match on the sender address works too.
    assert_eq!(
        ids_scoped(&database, "from:dave@example.com", Some("work")),
        vec![tid("work", "w-dave")]
    );
}

#[test]
fn operator_to_matches_stored_recipients() {
    let database = database();
    operator_fixture(&database);
    // bob and carol are recipients of the work alice thread.
    assert_eq!(
        ids_scoped(&database, "to:bob", None),
        vec![tid("work", "w-alice")]
    );
    assert_eq!(
        ids_scoped(&database, "to:carol", None),
        vec![tid("work", "w-alice")]
    );
    // alice is a recipient of the dave thread.
    assert_eq!(
        ids_scoped(&database, "to:alice", None),
        vec![tid("work", "w-dave")]
    );
    // erin only appears as a recipient in the home account.
    assert_eq!(
        ids_scoped(&database, "to:erin", None),
        vec![tid("home", "h-alice")]
    );
}

#[test]
fn operator_subject_matches_the_thread_subject() {
    let database = database();
    operator_fixture(&database);
    assert_eq!(
        ids_scoped(&database, "subject:budget", None),
        vec![tid("work", "w-alice")]
    );
    // Case-insensitive (ASCII) and substring.
    assert_eq!(
        ids_scoped(&database, "subject:LUNCH", None),
        vec![tid("work", "w-dave")]
    );
}

#[test]
fn operator_dates_compare_by_utc_day() {
    let database = database();
    operator_fixture(&database);
    // before:2024/06/15 excludes the 2024-06-15 thread (strictly before the
    // start of that day) but includes the earlier two.
    assert_eq!(
        ids_scoped(&database, "before:2024/06/15", None),
        vec![tid("home", "h-alice"), tid("work", "w-dave")]
    );
    // after:2024/06/15 includes the 2024-06-15 thread (on or after start of day).
    assert_eq!(
        ids_scoped(&database, "after:2024/06/15", None),
        vec![tid("work", "w-alice")]
    );
    // A tight window via two date terms (ANDed): only the Feb thread.
    assert_eq!(
        ids_scoped(&database, "after:2024/01/01 before:2024/03/01", None),
        vec![tid("work", "w-dave")]
    );
}

#[test]
fn operator_state_and_attachment_filters() {
    let database = database();
    operator_fixture(&database);
    assert_eq!(
        ids_scoped(&database, "is:unread", None),
        vec![tid("work", "w-alice")]
    );
    assert_eq!(
        ids_scoped(&database, "is:starred", None),
        vec![tid("work", "w-dave")]
    );
    assert_eq!(
        ids_scoped(&database, "has:attachment", None),
        vec![tid("work", "w-alice")]
    );
}

#[test]
fn operator_combinations_are_anded_with_free_text() {
    let database = database();
    operator_fixture(&database);
    // from:alice is:unread budget — only the work alice thread is all three.
    assert_eq!(
        ids_scoped(&database, "from:alice is:unread budget", None),
        vec![tid("work", "w-alice")]
    );
    // from:alice is:starred — alice never sent a starred thread: empty.
    assert!(ids_scoped(&database, "from:alice is:starred", None).is_empty());
    // Free text that does not match the body eliminates the row even when the
    // operator matches.
    assert!(ids_scoped(&database, "from:alice zzzznotpresent", None).is_empty());
}

#[test]
fn structured_free_text_uses_fts_and_sets_a_snippet_while_operator_only_does_not() {
    let database = database();
    operator_fixture(&database);
    // With free text, MATCH runs and a snippet is produced.
    let with_text = database
        .search_threads(
            &SearchThreadsRequest {
                query: "from:alice budget".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(with_text.len(), 1);
    assert!(with_text[0].match_snippet.is_some());
    // Operator-only (no free text): no MATCH, so match_snippet is None.
    let operator_only = database
        .search_threads(
            &SearchThreadsRequest {
                query: "from:alice".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert!(operator_only.iter().all(|t| t.match_snippet.is_none()));
}

#[test]
fn structured_escaping_treats_like_metacharacters_literally() {
    let database = database();
    // Two senders: one with a literal '%' in the local-part, one without.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "p1",
                "pct",
                "Promo",
                "sale%off@example.com",
                vec!["x@example.com"],
                "2024-05-01T00:00:00Z",
                "promo body",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            "work",
            &[op_message(
                "p2",
                "plain",
                "Promo2",
                "saleoff@example.com",
                vec!["x@example.com"],
                "2024-05-02T00:00:00Z",
                "promo body",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
    // `from:sale%off` must match ONLY the literal-'%' sender, not act as a
    // wildcard that also matches "saleoff".
    assert_eq!(
        ids_scoped(&database, "from:sale%off", None),
        vec![tid("work", "pct")]
    );
    // Underscore is likewise literal.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "u1",
                "under",
                "Score",
                "a_b@example.com",
                vec!["x@example.com"],
                "2024-05-03T00:00:00Z",
                "x",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
    assert_eq!(
        ids_scoped(&database, "from:a_b", None),
        vec![tid("work", "under")]
    );
    // `axb` must NOT match `a_b` (the underscore is not a single-char wildcard).
    assert!(ids_scoped(&database, "from:axb", None).is_empty());
}

#[test]
fn structured_injection_values_are_bound_not_interpolated_and_table_survives() {
    let database = database();
    operator_fixture(&database);
    let before = table_rows(&database, "threads").len();
    for hostile in [
        "from:'; DROP TABLE threads; --",
        "subject:\"'; DELETE FROM threads; --\"",
        "to:' OR '1'='1",
    ] {
        // Must not error, must not match (no such sender/subject/recipient),
        // and must not alter the table.
        let result = database
            .search_threads(
                &SearchThreadsRequest {
                    query: hostile.into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert!(result.is_empty(), "{hostile:?} should match nothing");
    }
    assert_eq!(
        table_rows(&database, "threads").len(),
        before,
        "threads table must be intact after injection attempts"
    );
}

#[test]
fn structured_over_limit_value_is_rejected_not_truncated_or_legacy() {
    let database = database();
    operator_fixture(&database);
    // A 100 KB from: value is far over MAX_LOCAL_SEARCH_TERM_BYTES. It must be
    // a typed error with the plain message, NOT a silent legacy fallback.
    let huge = "x".repeat(100 * 1024);
    let err = database
        .search_threads(
            &SearchThreadsRequest {
                query: format!("from:{huge}"),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap_err();
    assert_eq!(err.to_string(), "That search is too long.");
    // Table intact.
    assert!(!table_rows(&database, "threads").is_empty());
}

#[test]
fn structured_too_many_terms_is_rejected() {
    let database = database();
    operator_fixture(&database);
    // 17 from: terms (> MAX_LOCAL_SEARCH_TERMS = 16).
    let query = (0..17)
        .map(|i| format!("from:a{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let err = database
        .search_threads(
            &SearchThreadsRequest {
                query,
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap_err();
    assert_eq!(err.to_string(), "That search is too long.");
    // Exactly 16 is accepted (runs, matches nothing here).
    let at = (0..16)
        .map(|i| format!("from:a{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(database
        .search_threads(
            &SearchThreadsRequest {
                query: at,
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .is_ok());
}

#[test]
fn structured_respects_archived_and_trashed_default_exclusion() {
    let database = database();
    operator_fixture(&database);
    // Archive the work alice thread.
    database
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: tid("work", "w-alice"),
            value: true,
        })
        .unwrap();
    // Default: excluded.
    assert!(ids_scoped(&database, "from:alice", Some("work")).is_empty());
    // include_archived: shown again.
    let shown = database
        .search_threads(
            &SearchThreadsRequest {
                query: "from:alice".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            },
            Some("work"),
        )
        .unwrap();
    assert_eq!(shown.len(), 1);
    // Trashed is likewise excluded by default.
    database
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: tid("work", "w-alice"),
            value: false,
        })
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Trash {
            thread_id: tid("work", "w-alice"),
            value: true,
        })
        .unwrap();
    assert!(ids_scoped(&database, "from:alice", Some("work")).is_empty());
}

#[test]
fn structured_pagination_is_stable_and_has_no_duplicate_for_multi_message_match() {
    let database = database();
    // One thread, THREE messages all from alice: an EXISTS (not JOIN) match
    // must return the thread exactly once.
    database
        .upsert_thread(
            "work",
            &[
                op_message(
                    "m1",
                    "multi",
                    "Thread",
                    "alice@example.com",
                    vec!["x@example.com"],
                    "2024-01-01T00:00:00Z",
                    "one",
                    vec!["INBOX"],
                    vec![],
                ),
                op_message(
                    "m2",
                    "multi",
                    "Thread",
                    "alice@example.com",
                    vec!["x@example.com"],
                    "2024-01-02T00:00:00Z",
                    "two",
                    vec!["INBOX"],
                    vec![],
                ),
                op_message(
                    "m3",
                    "multi",
                    "Thread",
                    "alice@example.com",
                    vec!["x@example.com"],
                    "2024-01-03T00:00:00Z",
                    "three",
                    vec!["INBOX"],
                    vec![],
                ),
            ],
        )
        .unwrap();
    // Another alice thread so there are two rows to paginate.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "s1",
                "single",
                "Thread2",
                "alice@example.com",
                vec!["x@example.com"],
                "2024-02-01T00:00:00Z",
                "solo",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
    let all = ids_scoped(&database, "from:alice", Some("work"));
    assert_eq!(all.len(), 2, "multi-message thread counted once: {all:?}");

    let page = |limit, offset| {
        database
            .search_threads(
                &SearchThreadsRequest {
                    query: "from:alice".into(),
                    limit: Some(limit),
                    offset: Some(offset),
                    include_archived: None,
                },
                Some("work"),
            )
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    };
    let first = page(1, 0);
    let second = page(1, 1);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0], second[0], "pages do not overlap");
}

#[test]
fn structured_non_ascii_value_matches_its_own_exact_case() {
    let database = database();
    // Subject with a non-ASCII letter; LIKE is ASCII-case-insensitive only, so
    // the exact-case value matches.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "n1",
                "nonascii",
                "Café meeting",
                "a@example.com",
                vec!["x@example.com"],
                "2024-07-01T00:00:00Z",
                "body",
                vec!["INBOX"],
                vec![],
            )],
        )
        .unwrap();
    assert_eq!(
        ids_scoped(&database, "subject:Café", None),
        vec![tid("work", "nonascii")]
    );
}

#[test]
fn explain_query_plan_for_structured_queries_is_recorded() {
    // Not an assertion on the exact plan (it varies by SQLite build), but a
    // guard that the structured queries PREPARE and EXPLAIN cleanly. The plan
    // text is captured in the run report.
    let database = database();
    operator_fixture(&database);
    let connection = database.connection().unwrap();

    // from: EXISTS subquery, no MATCH — binds a LIKE pattern + limit/offset.
    let from_plan: Vec<String> = connection
        .prepare(
            "EXPLAIN QUERY PLAN \
             SELECT t.id FROM threads t WHERE EXISTS (SELECT 1 FROM messages m \
             WHERE m.thread_id = t.id AND m.sender LIKE ?1 ESCAPE '\\') \
             AND t.archived = 0 AND t.trashed = 0 \
             ORDER BY t.last_received_at DESC, t.id LIMIT ?2 OFFSET ?3",
        )
        .unwrap()
        .query_map(params!["%alice%", 50i64, 0i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!from_plan.is_empty(), "from: EXPLAIN QUERY PLAN produced rows");

    // no-free-text is:unread — pure threads scan, binds only limit/offset.
    let unread_plan: Vec<String> = connection
        .prepare(
            "EXPLAIN QUERY PLAN \
             SELECT t.id FROM threads t WHERE t.unread = 1 \
             AND t.archived = 0 AND t.trashed = 0 \
             ORDER BY t.last_received_at DESC, t.id LIMIT ?1 OFFSET ?2",
        )
        .unwrap()
        .query_map(params![50i64, 0i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!unread_plan.is_empty(), "is:unread EXPLAIN QUERY PLAN produced rows");
}

/// The structured path must highlight the match exactly like the legacy path:
/// the frontend finds the match by the `\u{1}`/`\u{2}` markers FTS5 puts around
/// it, so a snippet without them shows an unhighlighted excerpt.
#[test]
fn structured_free_text_snippet_marks_the_match_like_the_legacy_path() {
    let database = database();
    operator_fixture(&database);
    let request = |query: &str| SearchThreadsRequest {
        query: query.into(),
        limit: None,
        offset: None,
        include_archived: None,
    };
    let legacy = database.search_threads(&request("budget"), None).unwrap();
    let structured = database
        .search_threads(&request("from:alice budget"), None)
        .unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(structured.len(), 1);
    let marked = structured[0].match_snippet.as_deref().unwrap_or_default();
    assert!(
        marked.contains("\u{1}budget\u{2}"),
        "structured snippet lost its match markers: {marked:?}"
    );
    assert_eq!(
        structured[0].match_snippet, legacy[0].match_snippet,
        "same thread and same words must give the same snippet on both paths"
    );
}

/// `messages.sent_at` is not always UTC: a message with no usable internal date
/// falls back to its `Date:` header, which `to_rfc3339` keeps in the sender's
/// offset. Comparing those strings lexicographically against a UTC bound is
/// wrong by the offset, so the day bounds must compare instants.
#[test]
fn structured_date_bounds_compare_instants_across_utc_offsets() {
    let database = database();
    crate::db::test_support::clear_seed_threads(&database);
    // 2024-06-15 01:00 +02:00 is 2024-06-14 23:00 UTC.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "east", "t-east", "East", "a@example.com", vec!["b@example.com"],
                "2024-06-15T01:00:00+02:00", "x", vec!["INBOX"], vec![],
            )],
        )
        .unwrap();
    // 2024-06-15 23:30 -05:00 is 2024-06-16 04:30 UTC.
    database
        .upsert_thread(
            "work",
            &[op_message(
                "west", "t-west", "West", "a@example.com", vec!["b@example.com"],
                "2024-06-15T23:30:00-05:00", "x", vec!["INBOX"], vec![],
            )],
        )
        .unwrap();
    let east = tid("work", "t-east");
    let west = tid("work", "t-west");
    // The east thread is still 14 June in UTC.
    assert_eq!(ids_scoped(&database, "after:2024-06-15", None), vec![west.clone()]);
    assert_eq!(ids_scoped(&database, "before:2024-06-15", None), vec![east.clone()]);
    // The west thread is already 16 June in UTC.
    assert_eq!(ids_scoped(&database, "after:2024-06-16", None), vec![west]);
    assert_eq!(ids_scoped(&database, "before:2024-06-16", None), vec![east]);
}

/// `recipients_json` is a JSON array. `to:` must match the recipient strings,
/// never the brackets, quotes and commas of the encoding around them.
#[test]
fn structured_to_matches_recipients_not_json_punctuation() {
    let database = database();
    operator_fixture(&database);
    for punctuation in ["to:[", "to:]", "to:,", "to:\\"] {
        assert!(
            ids_scoped(&database, punctuation, None).is_empty(),
            "{punctuation} matched the JSON encoding of the recipient list"
        );
    }
    // A real recipient still matches, including one that is not first in the
    // list.
    assert_eq!(
        ids_scoped(&database, "to:carol@example.com", None),
        vec![tid("work", "w-alice")]
    );
}
