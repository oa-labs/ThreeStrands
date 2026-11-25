//! List projections exclude message bodies, signatures and forwarded originals.
//! SQLite constructs each summary before it crosses the database boundary.
use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::correspondence::{DraftSummary, OutboxSummary, SentCopyStatus};
use rusqlite::{functions::FunctionFlags, Connection};

const PREVIEW_CHARS: usize = 200;

pub(super) fn register_functions(connection: &Connection) -> rusqlite::Result<()> {
    connection.create_scalar_function(
        "draft_preview",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let value = context.get_raw(0).as_str()?;
            let mut result = String::new();
            for word in value.split_whitespace() {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.extend(word.chars().take(PREVIEW_CHARS - result.chars().count()));
                if result.chars().count() >= PREVIEW_CHARS {
                    break;
                }
            }
            Ok(result)
        },
    )
}

// Explicit allowlist: future body fields cannot silently enter list responses.
const SUMMARY: &str = "json_object(
 'id',json_extract(payload,'$.id'), 'revision',json_extract(payload,'$.revision'),
 'account',json_extract(payload,'$.account'), 'from',json_extract(payload,'$.from'),
 'mode',json_extract(payload,'$.mode'), 'sourceId',json_extract(payload,'$.sourceId'),
 'threadId',json_extract(payload,'$.threadId'), 'replyId',json_extract(payload,'$.replyId'),
 'to',json_extract(payload,'$.to'), 'cc',json_extract(payload,'$.cc'),
 'bcc',json_extract(payload,'$.bcc'), 'subject',json_extract(payload,'$.subject'),
 'updatedAt',json_extract(payload,'$.updatedAt'),
 'followUpTaskId',json_extract(payload,'$.followUpTaskId'),
 'completesTaskId',json_extract(payload,'$.completesTaskId'),
 'preview',draft_preview(json_extract(payload,'$.body')),
 'hasAttachments',json(CASE WHEN EXISTS(SELECT 1 FROM json_each(json_extract(payload,'$.attachments')) a
 WHERE COALESCE(json_extract(a.value,'$.inline'),0)=0) THEN 'true' ELSE 'false' END))";

impl Database {
    pub fn draft_summaries(&self) -> DbResult<Vec<DraftSummary>> {
        self.with_connection(|c| {
            let mut q = c.prepare(&format!("SELECT {SUMMARY} FROM drafts ORDER BY rowid DESC"))?;
            let rows = q.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| serde_json::from_str(&r?).map_err(serialization_error))
                .collect()
        })
    }
    pub fn outbox_summaries(&self) -> DbResult<Vec<OutboxSummary>> {
        self.with_connection(|c| {
            let mut q = c.prepare(&format!("SELECT id,{SUMMARY},state,deadline,error,provider_id,schedule_json,sent_copy_state,sent_copy_error FROM outbox_messages ORDER BY rowid DESC"))?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?)))?;
            rows.map(|row| {
                let (id,draft,state,deadline,error,provider_id,schedule,sent_copy,sent_error) = row?;
                Ok(OutboxSummary {
                    id, draft: serde_json::from_str(&draft).map_err(serialization_error)?, state,deadline,error,provider_id,
                    schedule: schedule.map(|s| serde_json::from_str(&s)).transpose().map_err(serialization_error)?,
                    sent_copy: sent_copy.map(|state| SentCopyStatus { state, error: sent_error }),
                })
            }).collect()
        })
    }
    pub fn outbox_item(&self, id: &str) -> DbResult<crate::correspondence::OutboxItem> {
        // Reuse the canonical row decoder, restricted to this one send.
        self.read_outbox(Some(id))?
            .pop()
            .ok_or(DatabaseError::NotFound("Outbox item"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projects_metadata_without_retaining_bodies_or_forwarded_originals() {
        let db = Database::open_memory();
        db.set_compose_identity("you@example.com").unwrap();
        db.adopt_account("you@example.com").unwrap();
        let mut draft = db
            .create_draft("new", None, "you@example.com", &[])
            .unwrap();
        draft.to = "Jane <jane@example.com>".into();
        draft.body = format!("  first\n\twords {}", "x".repeat(100_000));
        draft.body_html = "<p>SECRET_HTML</p>".into();
        draft.forwarded_content = Some(crate::correspondence::ForwardedContent {
            html: "FORWARDED_HTML".repeat(1000),
            text: "FORWARDED_TEXT".repeat(1000),
        });
        let draft = db.save_draft(draft).unwrap();
        let summaries = db.draft_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].preview.chars().count(), PREVIEW_CHARS);
        assert!(summaries[0].preview.starts_with("first words "));
        let encoded = serde_json::to_string(&summaries).unwrap();
        for field in [
            "body",
            "bodyHtml",
            "forwardedContent",
            "forwardedEdit",
            "signature",
            "attachments",
            "references",
        ] {
            assert!(!encoded.contains(&format!("\"{field}\":")));
        }
        assert!(!encoded.contains("SECRET_HTML"));
        assert!(!encoded.contains("FORWARDED_"));
        assert!(encoded.len() < 2000);
        assert_eq!(db.draft(&draft.id).unwrap().body, draft.body);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let item = db
            .queue(&draft.id, draft.revision, false, &[], None, &root)
            .unwrap();
        assert!(db.draft_summaries().unwrap().is_empty());
        let summaries = db.outbox_summaries().unwrap();
        assert_eq!(summaries[0].id, item.id);
        assert_eq!(summaries[0].state, "undo_pending");
        assert!(serde_json::to_string(&summaries).unwrap().len() < 2500);
        assert_eq!(
            db.outbox_item(&item.id).unwrap().draft.body_html,
            draft.body_html
        );
        assert!(db.outbox_item("missing").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_is_bounded_after_whitespace_cleanup_and_handles_unicode() {
        let db = Database::open_memory();
        for length in [PREVIEW_CHARS - 1, PREVIEW_CHARS, PREVIEW_CHARS + 1] {
            let input = format!("\n\t {} \n", "界".repeat(length));
            let preview: String = db
                .with_connection(|c| {
                    Ok(c.query_row("SELECT draft_preview(?1)", [&input], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(preview, "界".repeat(length.min(PREVIEW_CHARS)));
        }
        let preview: String = db
            .with_connection(|c| {
                Ok(c.query_row("SELECT draft_preview(?1)", [" \n\t"], |r| r.get(0))?)
            })
            .unwrap();
        assert!(preview.is_empty());
    }
}
