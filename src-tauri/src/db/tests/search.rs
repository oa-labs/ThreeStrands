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
