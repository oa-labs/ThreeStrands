use chrono::{Days, NaiveDate};
use rusqlite::{params, OptionalExtension};

use super::{Database, DbResult};
use crate::models::{ActionAnalysis, ActionProposal, AiUsageDay};

/// Daily usage rows older than this are pruned when new usage is recorded.
pub(crate) const AI_USAGE_RETENTION_DAYS: u64 = 90;

impl Database {
    /// The saved suggestions for a thread, if they were produced for its
    /// current newest message.
    pub fn thread_analysis(
        &self,
        thread_id: &str,
        last_message_at: &str,
    ) -> DbResult<Option<ActionAnalysis>> {
        let saved: Option<String> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT analysis_json FROM ai_thread_analyses WHERE thread_id = ?1 AND last_message_at = ?2",
                    params![thread_id, last_message_at],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        // A row this build cannot read is treated as missing, so analysis
        // simply runs again instead of failing.
        Ok(saved.and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Replaces the thread's saved suggestions with those for its current
    /// newest message.
    pub fn save_thread_analysis(
        &self,
        thread_id: &str,
        last_message_at: &str,
        analysis: &ActionAnalysis,
        generated_at: &str,
    ) -> DbResult<()> {
        let json = serde_json::to_string(analysis).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO ai_thread_analyses(thread_id, last_message_at, analysis_json, generated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(thread_id) DO UPDATE SET
                    last_message_at = excluded.last_message_at,
                    analysis_json = excluded.analysis_json,
                    generated_at = excluded.generated_at",
                params![thread_id, last_message_at, json, generated_at],
            )?;
            Ok(())
        })
    }

    /// Removes one handled suggestion (discarded, or turned into a task or
    /// event) from the thread's saved suggestions, so reopening the thread
    /// doesn't offer it again. Only the saved set for `last_message_at` is
    /// changed. Returns false when nothing matched, as for a suggestion that
    /// came from chat and was never saved.
    pub fn remove_thread_suggestion(
        &self,
        thread_id: &str,
        last_message_at: &str,
        proposal: &ActionProposal,
    ) -> DbResult<bool> {
        let target = serde_json::to_value(proposal).map_err(|error| error.to_string())?;
        self.with_transaction(|transaction| {
            let saved: Option<String> = transaction
                .query_row(
                    "SELECT analysis_json FROM ai_thread_analyses WHERE thread_id = ?1 AND last_message_at = ?2",
                    params![thread_id, last_message_at],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(mut analysis) = saved.and_then(|json| serde_json::from_str::<ActionAnalysis>(&json).ok()) else {
                return Ok(false);
            };
            // Compare normalized JSON so defaults filled in on either side still match.
            let Some(position) = analysis
                .proposals
                .iter()
                .position(|candidate| serde_json::to_value(candidate).ok().as_ref() == Some(&target))
            else {
                return Ok(false);
            };
            analysis.proposals.remove(position);
            let json = serde_json::to_string(&analysis).map_err(|error| error.to_string())?;
            transaction.execute(
                "UPDATE ai_thread_analyses SET analysis_json = ?3 WHERE thread_id = ?1 AND last_message_at = ?2",
                params![thread_id, last_message_at, json],
            )?;
            Ok(true)
        })
    }

    /// Adds one completed provider request to the day's usage and prunes
    /// rows older than the retention window. `day` is a local `YYYY-MM-DD`.
    pub fn record_ai_usage(
        &self,
        day: &str,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    ) -> DbResult<()> {
        let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|error| error.to_string())?;
        let cutoff = date
            .checked_sub_days(Days::new(AI_USAGE_RETENTION_DAYS))
            .unwrap_or(date)
            .format("%Y-%m-%d")
            .to_string();
        let cost = cost_usd.filter(|cost| cost.is_finite() && *cost >= 0.0);
        let to_i64 = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO ai_usage(day, provider, model, requests, input_tokens, output_tokens, reported_cost_requests, reported_cost_usd)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7)
                 ON CONFLICT(day, provider, model) DO UPDATE SET
                    requests = requests + 1,
                    input_tokens = input_tokens + excluded.input_tokens,
                    output_tokens = output_tokens + excluded.output_tokens,
                    reported_cost_requests = reported_cost_requests + excluded.reported_cost_requests,
                    reported_cost_usd = reported_cost_usd + excluded.reported_cost_usd",
                params![
                    day,
                    provider,
                    model,
                    to_i64(input_tokens),
                    to_i64(output_tokens),
                    i64::from(cost.is_some()),
                    cost.unwrap_or(0.0),
                ],
            )?;
            connection.execute("DELETE FROM ai_usage WHERE day <= ?1", [cutoff])?;
            Ok(())
        })
    }

    /// The conversations that best match any of the search words, most
    /// relevant first, excluding the open conversation and trashed mail.
    /// Terms must already be plain words (see `ai::chat_search_terms`).
    pub fn chat_search_thread_ids(
        &self,
        terms: &[String],
        exclude_thread_id: &str,
        limit: usize,
    ) -> DbResult<Vec<String>> {
        let words: Vec<&String> = terms
            .iter()
            .filter(|term| !term.is_empty() && term.chars().all(char::is_alphanumeric))
            .collect();
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query = words
            .iter()
            .map(|word| format!("\"{word}\"*"))
            .collect::<Vec<_>>()
            .join(" OR ");
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT t.id FROM thread_search s JOIN threads t ON t.id = s.thread_id
                 WHERE thread_search MATCH ?1 AND t.trashed = 0 AND t.id <> ?2
                 ORDER BY rank, t.last_received_at DESC
                 LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![
                    query,
                    exclude_thread_id,
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| row.get(0),
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Usage rows from `since_day` (inclusive), newest first.
    pub fn ai_usage_since(&self, since_day: &str) -> DbResult<Vec<AiUsageDay>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT day, provider, model, requests, input_tokens, output_tokens, reported_cost_requests, reported_cost_usd
                 FROM ai_usage WHERE day >= ?1 ORDER BY day DESC, provider, model",
            )?;
            let rows = statement.query_map([since_day], |row| {
                Ok(AiUsageDay {
                    day: row.get(0)?,
                    provider: row.get(1)?,
                    model: row.get(2)?,
                    requests: row.get(3)?,
                    input_tokens: row.get(4)?,
                    output_tokens: row.get(5)?,
                    reported_cost_requests: row.get(6)?,
                    reported_cost_usd: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use crate::models::{ActionAnalysis, ActionProposal};

    fn database_with_thread() -> Database {
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO threads(
                    id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json,
                    trashed, account_id, summary, summary_generated_at,
                    has_attachments, last_received_at
                 ) VALUES ('account:thread', 'thread', 'Planning', 'Please reply', '[]',
                           '2026-09-19T10:00:00Z', 0, 0, 0, '[]', 0, 'account@example.com',
                           NULL, NULL, 0, '2026-09-19T10:00:00Z')",
                [],
            )
            .unwrap();
        database
    }

    fn analysis(hidden_count: usize) -> ActionAnalysis {
        let proposal: ActionProposal = serde_json::from_value(serde_json::json!({
            "type": "task", "kind": "action", "title": "Send the deck", "notes": null,
            "dueKind": "none", "dueValue": null, "timeZone": null, "repeatIntervalDays": null,
            "confidence": 0.9, "evidence": {"sourceMessageId": "m1", "excerpt": "Send the deck"},
        }))
        .unwrap();
        ActionAnalysis {
            proposals: vec![proposal],
            hidden_count,
        }
    }

    #[test]
    fn thread_analysis_is_reused_only_for_the_same_newest_message() {
        let database = database_with_thread();
        assert!(database
            .thread_analysis("account:thread", "2026-09-19T10:00:00Z")
            .unwrap()
            .is_none());

        database
            .save_thread_analysis(
                "account:thread",
                "2026-09-19T10:00:00Z",
                &analysis(1),
                "2026-09-19T11:00:00Z",
            )
            .unwrap();
        let saved = database
            .thread_analysis("account:thread", "2026-09-19T10:00:00Z")
            .unwrap()
            .unwrap();
        assert_eq!((saved.proposals.len(), saved.hidden_count), (1, 1));
        assert!(database
            .thread_analysis("account:thread", "2026-09-20T10:00:00Z")
            .unwrap()
            .is_none());

        database
            .save_thread_analysis(
                "account:thread",
                "2026-09-20T10:00:00Z",
                &analysis(0),
                "2026-09-20T11:00:00Z",
            )
            .unwrap();
        assert!(database
            .thread_analysis("account:thread", "2026-09-19T10:00:00Z")
            .unwrap()
            .is_none());
        assert_eq!(
            database
                .thread_analysis("account:thread", "2026-09-20T10:00:00Z")
                .unwrap()
                .unwrap()
                .hidden_count,
            0
        );
    }

    fn task(title: &str) -> ActionProposal {
        serde_json::from_value(serde_json::json!({
            "type": "task", "kind": "action", "title": title, "notes": null,
            "dueKind": "none", "dueValue": null, "timeZone": null, "repeatIntervalDays": null,
            "confidence": 0.9, "evidence": {"sourceMessageId": "m1", "excerpt": title},
        }))
        .unwrap()
    }

    fn saved_titles(database: &Database, revision: &str) -> Option<Vec<String>> {
        database.thread_analysis("account:thread", revision).unwrap().map(|analysis| {
            analysis
                .proposals
                .into_iter()
                .map(|proposal| match proposal {
                    ActionProposal::Task(task) => task.title,
                    ActionProposal::Meeting(meeting) => meeting.title,
                })
                .collect()
        })
    }

    #[test]
    fn removing_a_handled_suggestion_keeps_the_rest_for_the_same_revision() {
        let database = database_with_thread();
        let revision = "2026-09-19T10:00:00Z";
        database
            .save_thread_analysis(
                "account:thread",
                revision,
                &ActionAnalysis { proposals: vec![task("Send the deck"), task("Book the room")], hidden_count: 2 },
                "2026-09-19T11:00:00Z",
            )
            .unwrap();

        assert!(database.remove_thread_suggestion("account:thread", revision, &task("Send the deck")).unwrap());
        assert_eq!(saved_titles(&database, revision), Some(vec!["Book the room".to_string()]));
        assert_eq!(database.thread_analysis("account:thread", revision).unwrap().unwrap().hidden_count, 2);

        // A suggestion that was never saved, such as one from chat, changes nothing.
        assert!(!database.remove_thread_suggestion("account:thread", revision, &task("From chat")).unwrap());
        assert_eq!(saved_titles(&database, revision), Some(vec!["Book the room".to_string()]));
    }

    #[test]
    fn removing_a_suggestion_ignores_other_revisions_and_missing_rows() {
        let database = database_with_thread();
        assert!(!database
            .remove_thread_suggestion("account:thread", "2026-09-19T10:00:00Z", &task("Send the deck"))
            .unwrap());

        database
            .save_thread_analysis("account:thread", "2026-09-20T10:00:00Z", &analysis(0), "2026-09-20T11:00:00Z")
            .unwrap();
        // A removal for the older revision leaves the newer saved set alone.
        assert!(!database
            .remove_thread_suggestion("account:thread", "2026-09-19T10:00:00Z", &task("Send the deck"))
            .unwrap());
        assert_eq!(saved_titles(&database, "2026-09-20T10:00:00Z"), Some(vec!["Send the deck".to_string()]));
    }

    #[test]
    fn thread_analysis_is_removed_with_its_thread_and_ignores_unreadable_rows() {
        let database = database_with_thread();
        database
            .save_thread_analysis(
                "account:thread",
                "2026-09-19T10:00:00Z",
                &analysis(0),
                "2026-09-19T11:00:00Z",
            )
            .unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "UPDATE ai_thread_analyses SET analysis_json = '{\"proposals\":[],\"tool\":1}'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(database
            .thread_analysis("account:thread", "2026-09-19T10:00:00Z")
            .unwrap()
            .is_none());

        database
            .connection()
            .unwrap()
            .execute("DELETE FROM threads WHERE id = 'account:thread'", [])
            .unwrap();
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM ai_thread_analyses", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn usage_accumulates_per_day_and_model_and_keeps_reported_cost_separate() {
        let database = Database::open_memory();
        database
            .record_ai_usage("2026-09-29", "openai", "gpt-4o", 1_000, 200, None)
            .unwrap();
        database
            .record_ai_usage("2026-09-29", "openai", "gpt-4o", 500, 100, None)
            .unwrap();
        database
            .record_ai_usage(
                "2026-09-29",
                "openrouter",
                "openai/gpt-4o",
                300,
                50,
                Some(0.0125),
            )
            .unwrap();
        database
            .record_ai_usage(
                "2026-09-29",
                "openrouter",
                "openai/gpt-4o",
                10,
                5,
                Some(f64::NAN),
            )
            .unwrap();
        database
            .record_ai_usage("2026-09-28", "openai", "gpt-4o", 10, 1, None)
            .unwrap();

        let today = database.ai_usage_since("2026-09-29").unwrap();
        assert_eq!(today.len(), 2);
        let openai = today.iter().find(|row| row.provider == "openai").unwrap();
        assert_eq!(
            (
                openai.requests,
                openai.input_tokens,
                openai.output_tokens,
                openai.reported_cost_requests
            ),
            (2, 1_500, 300, 0)
        );
        let openrouter = today
            .iter()
            .find(|row| row.provider == "openrouter")
            .unwrap();
        assert_eq!(
            (openrouter.requests, openrouter.reported_cost_requests),
            (2, 1)
        );
        assert!((openrouter.reported_cost_usd - 0.0125).abs() < 1e-9);
        assert_eq!(database.ai_usage_since("2026-09-28").unwrap().len(), 3);
    }

    #[test]
    fn usage_older_than_the_retention_window_is_pruned() {
        let database = Database::open_memory();
        let day = |offset: u64| {
            chrono::NaiveDate::from_ymd_opt(2026, 9, 29)
                .unwrap()
                .checked_sub_days(chrono::Days::new(offset))
                .unwrap()
                .format("%Y-%m-%d")
                .to_string()
        };
        for offset in [
            super::AI_USAGE_RETENTION_DAYS - 1,
            super::AI_USAGE_RETENTION_DAYS,
            super::AI_USAGE_RETENTION_DAYS + 1,
        ] {
            database
                .record_ai_usage(&day(offset), "openai", "gpt-4o", 1, 1, None)
                .unwrap();
        }
        database
            .record_ai_usage(&day(0), "openai", "gpt-4o", 1, 1, None)
            .unwrap();
        let days = database
            .ai_usage_since("2000-01-01")
            .unwrap()
            .into_iter()
            .map(|row| row.day)
            .collect::<Vec<_>>();
        assert_eq!(days, vec![day(0), day(super::AI_USAGE_RETENTION_DAYS - 1)]);
        assert!(database
            .record_ai_usage("not-a-day", "openai", "gpt-4o", 1, 1, None)
            .is_err());
    }
}
