//! Derived note-task and entity-link indexes. Mutations reuse the note write
//! transaction, conflict guard and sync budget, never a second task entity.
use super::{Database, DatabaseError, DbResult};
use crate::{
    models::{Note, NoteBacklink},
    note_entities::{self, EntityLink, NoteItem, NoteReference},
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub items: Vec<NoteItem>,
    pub links: Vec<EntityLink>,
    pub warning: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTask {
    pub id: String,
    pub marker_id: String,
    pub note_id: String,
    pub note_title: String,
    pub title: String,
    pub checked: bool,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub conflicted: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteCapture {
    pub note: Note,
    pub added: bool,
    pub previous_body: String,
}

pub(crate) fn index(tx: &Transaction<'_>, note: &Note) -> DbResult<()> {
    // Keep completion observation across unchanged projections on this device.
    let previous = {
        let mut q = tx.prepare("SELECT marker_id,completed_at FROM note_tasks WHERE note_id=?1")?;
        let rows = q
            .query_map([&note.id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        rows
    };
    tx.execute("DELETE FROM note_tasks WHERE note_id=?1", [&note.id])?;
    tx.execute("DELETE FROM note_entity_links WHERE note_id=?1", [&note.id])?;
    tx.execute(
        "DELETE FROM note_index_warnings WHERE note_id=?1",
        [&note.id],
    )?;
    let parsed = match note_entities::parse(&note.id, &note.body) {
        Ok(parsed) => parsed,
        Err(error) => {
            tx.execute(
                "INSERT INTO note_index_warnings VALUES (?1,?2)",
                params![note.id, error],
            )?;
            return Ok(());
        }
    };
    for item in parsed.items.iter().filter(|item| item.task) {
        let completed = item.checked.then(|| {
            previous
                .get(&item.id)
                .cloned()
                .flatten()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
        });
        tx.execute("INSERT INTO note_tasks(note_id,marker_id,title,checked,completed_at) VALUES (?1,?2,?3,?4,?5)",params![note.id,item.id,item.title,item.checked,completed])?;
    }
    for (position, link) in parsed.links.iter().enumerate() {
        tx.execute("INSERT INTO note_entity_links(note_id,position,href,context,start) VALUES (?1,?2,?3,?4,?5)",params![note.id,position as i64,link.href,link.context,link.start as i64])?;
    }
    Ok(())
}
/// Follow only verified, account-scoped provider aliases. A cycle or a
/// chain beyond the established provider bound is unavailable, never guessed.
fn conversation_id(c: &rusqlite::Connection, account: &str, id: &str) -> DbResult<Option<String>> {
    let mut current = id.to_string();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        if !seen.insert(current.clone()) {
            return Ok(None);
        }
        let next: Option<String> = c
            .query_row(
                "SELECT new_id FROM imap_thread_aliases WHERE account_id=?1 AND old_id=?2",
                params![account, current],
                |r| r.get(0),
            )
            .optional()?;
        match next {
            Some(next) => current = next,
            None => return Ok(Some(current)),
        }
    }
    Ok(None)
}

impl Database {
    pub fn note_document(&self, id: &str) -> DbResult<NoteDocument> {
        let note = self.get_note(id)?.ok_or(DatabaseError::NotFound("Note"))?;
        Ok(match note_entities::parse(id, &note.body) {
            Ok(p) => NoteDocument {
                items: p.items,
                links: p.links,
                warning: None,
            },
            Err(e) => NoteDocument {
                items: vec![],
                links: vec![],
                warning: Some(e),
            },
        })
    }
    pub fn list_note_tasks(&self) -> DbResult<Vec<NoteTask>> {
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT t.marker_id,t.note_id,n.title,t.title,t.checked,n.updated_at,t.completed_at,
                EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=n.id GROUP BY field HAVING count(DISTINCT value)>1)
                FROM note_tasks t JOIN notes n ON n.id=t.note_id WHERE n.trashed=0 ORDER BY t.checked,t.completed_at DESC,t.note_id,t.marker_id")?;
            let rows = q.query_map([],|r|{let marker:String=r.get(0)?;let note:String=r.get(1)?; Ok(NoteTask{id:format!("note:{note}:{marker}"),marker_id:marker,note_id:note,note_title:r.get(2)?,title:r.get(3)?,checked:r.get(4)?,updated_at:r.get(5)?,completed_at:r.get(6)?,conflicted:r.get(7)?})})?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        })
    }
    pub fn patch_note_item(
        &self,
        id: &str,
        marker: &str,
        expected: &str,
        checked: Option<bool>,
        task: Option<bool>,
    ) -> DbResult<Note> {
        self.with_transaction(|tx| {
            let note = super::notes::get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
            if !super::notes::is_current(Some(&note), Some(expected)) {
                return Err(DatabaseError::invalid(
                    "This note changed. Open it to resolve your changes.",
                ));
            }
            let conflicted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)", [id], |r|r.get(0))?;
            if conflicted { return Err(DatabaseError::invalid("Resolve this note's conflict before changing its tasks.")); }
            let body = note_entities::patch(id, &note.body, marker, checked, task)
                .map_err(DatabaseError::invalid)?;
            let title = note.title.clone();
            super::notes::edit(tx, note, title, &body)
        })
    }
    pub fn note_backlinks(&self, reference: &NoteReference) -> DbResult<Vec<NoteBacklink>> {
        reference.href().map_err(DatabaseError::invalid)?;
        self.with_connection(|c| {
            let mut references=vec![reference.clone()];
            if reference.kind=="conversation" {
                let account=reference.account.as_deref().unwrap();
                let target=conversation_id(c,account,&reference.id)?;
                if let Some(target)=target {
                    let mut q=c.prepare("SELECT old_id FROM imap_thread_aliases WHERE account_id=?1")?;
                    for old in q.query_map([account],|r|r.get::<_,String>(0))? {
                        let old=old?;
                        if conversation_id(c,account,&old)?.as_ref()==Some(&target) {
                            references.push(NoteReference{id:old,..reference.clone()});
                        }
                    }
                }
            }
            let mut rows=std::collections::BTreeMap::new();
            for reference in references {
                let mut q=c.prepare("SELECT n.id,n.title,n.daily_date,l.context FROM note_entity_links l JOIN notes n ON n.id=l.note_id WHERE l.href=?1 AND n.trashed=0 GROUP BY n.id ORDER BY n.title,n.id")?;
                for row in q.query_map([reference.href().map_err(DatabaseError::invalid)?],|r|Ok(NoteBacklink{reference:Some(reference.clone()),note_id:r.get(0)?,title:r.get(1)?,daily_date:r.get(2)?,context:r.get(3)?}))? {
                    let row=row?;rows.entry(row.note_id.clone()).or_insert(row);
                }
            }
            Ok(rows.into_values().collect())
        })
    }
    /// Account+provider identity is portable; never resolve by subject or headers.
    pub fn resolve_note_conversation(&self, reference: &NoteReference) -> DbResult<Option<String>> {
        if !reference.validate() || reference.kind != "conversation" {
            return Err(DatabaseError::invalid("Invalid conversation reference"));
        }
        self.with_connection(|c| {
            let Some(id) =
                conversation_id(c, reference.account.as_deref().unwrap(), &reference.id)?
            else {
                return Ok(None);
            };
            Ok(c.query_row(
                "SELECT id FROM threads WHERE account_id=?1 AND provider_thread_id=?2",
                params![reference.account, id],
                |r| r.get(0),
            )
            .optional()?)
        })
    }
    pub fn capture_note_conversation(
        &self,
        date: &str,
        thread: &str,
        expected: Option<&str>,
    ) -> DbResult<NoteCapture> {
        if !crate::notes::is_daily_date(date) {
            return Err(DatabaseError::invalid("Invalid date"));
        }
        self.with_transaction(|tx| {
            let (account, provider, title): (String, String, String) = tx.query_row(
                "SELECT account_id,provider_thread_id,subject FROM threads WHERE id=?1",
                [thread], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let reference = NoteReference {
                version: 1, kind: "conversation".into(), id: provider,
                account: Some(account), calendar: None, note: None,
            };
            let href = reference.href().map_err(DatabaseError::invalid)?;
            let id = crate::notes::daily_note_id(date);
            let note = super::notes::get(tx, &id)?;
            if !super::notes::is_current(note.as_ref(), expected) {
                return Err(DatabaseError::invalid("Today's note changed. Open it before adding this conversation."));
            }
            let conflicted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)",
                [&id], |r| r.get(0),
            )?;
            if conflicted { return Err(DatabaseError::invalid("Resolve today's note conflict before adding this conversation.")); }
            let previous_body = note.as_ref().map(|n| n.body.clone()).unwrap_or_default();
            let parsed = note_entities::parse(&id, &previous_body).map_err(DatabaseError::invalid)?;
            for link in &parsed.links {
                let same_conversation = link.reference.kind == "conversation"
                    && link.reference.account == reference.account
                    && conversation_id(tx, reference.account.as_deref().unwrap(), &link.reference.id)?.as_ref() == Some(&reference.id);
                if link.reference == reference || same_conversation {
                    return Ok(NoteCapture { note: note.unwrap(), added: false, previous_body });
                }
            }
            if parsed.unclosed_fence {
                return Err(DatabaseError::invalid("Today's note ends inside a code block. Open it before adding this conversation."));
            }
            let title = if title.trim().is_empty() { "(No subject)" } else { title.trim() };
            let mut label = String::new();
            for ch in title.chars() {
                if ch.is_ascii_punctuation() { label.push('\\'); }
                label.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
            }
            let separator = if previous_body.is_empty() || previous_body.ends_with('\n') { "" } else { "\n" };
            let body = format!("{previous_body}{separator}- [{label}]({href})\n");
            let saved = super::notes::save_daily_in(tx, date, &body)?.ok_or(DatabaseError::NotFound("Note"))?;
            Ok(NoteCapture { note: saved, added: true, previous_body })
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteLinkChoice {
    pub label: String,
    pub detail: String,
    pub reference: NoteReference,
}
impl Database {
    pub fn cache_note_events(
        &self,
        account: &str,
        calendars: &[String],
        min: &str,
        max: &str,
        events: &[crate::models::ScheduleEvent],
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            for calendar in calendars { tx.execute("DELETE FROM note_event_cache WHERE account=?1 AND calendar=?2 AND julianday(start)<julianday(?4) AND julianday(end)>julianday(?3)",params![account,calendar,min,max])?; }
            for event in events {
                tx.execute("INSERT INTO note_event_cache VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(account,calendar,id) DO UPDATE SET start=excluded.start,end=excluded.end,body=excluded.body",params![event.account_id,event.calendar_id,event.id,event.start,event.end,serde_json::to_string(event).map_err(super::serialization_error)?])?;
            }
            Ok(())
        })
    }
    pub fn resolve_note_event(
        &self,
        reference: &NoteReference,
    ) -> DbResult<Option<crate::models::ScheduleEvent>> {
        if !reference.validate() || reference.kind != "event" {
            return Err(DatabaseError::invalid("Invalid event reference"));
        }
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT body FROM note_event_cache WHERE account=?1 AND calendar=?2 AND EXISTS(SELECT 1 FROM calendar_accounts WHERE email=?1 AND status='connected') ORDER BY start")?;
            let rows=q.query_map(params![reference.account,reference.calendar],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            for body in rows { let event:crate::models::ScheduleEvent=serde_json::from_str(&body).map_err(super::serialization_error)?; if event.id==reference.id || event.series_id.as_ref()==Some(&reference.id) { return Ok(Some(event)); } }
            Ok(None)
        })
    }
    pub fn note_link_choices(&self, kind: &str, query: &str) -> DbResult<Vec<NoteLinkChoice>> {
        self.with_connection(|c| {
            let mut choices=Vec::new();
            match kind {
                "conversation" => {
                    let mut q=c.prepare("SELECT subject,account_id,provider_thread_id FROM threads WHERE instr(lower(subject),lower(?1))>0 ORDER BY last_message_at DESC LIMIT 50")?;
                    for r in q.query_map([query],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
                        let (label,account,id)=r?;
                        choices.push(NoteLinkChoice{label:if label.is_empty(){"(No subject)".into()}else{label},detail:account.clone(),reference:NoteReference{version:1,kind:kind.into(),id,account:Some(account),calendar:None,note:None}});
                    }
                }
                "task" => {
                    let mut q=c.prepare("SELECT title,account_id,id FROM tasks WHERE instr(lower(title),lower(?1))>0 ORDER BY updated_at DESC LIMIT 50")?;
                    for r in q.query_map([query],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
                        let (label,detail,id)=r?; choices.push(NoteLinkChoice{label,detail,reference:NoteReference{version:1,kind:kind.into(),id,account:None,calendar:None,note:None}});
                    }
                    let mut q=c.prepare("SELECT t.title,n.title,t.marker_id,n.id FROM note_tasks t JOIN notes n ON n.id=t.note_id WHERE n.trashed=0 AND instr(lower(t.title),lower(?1))>0 LIMIT 50")?;
                    for r in q.query_map([query],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
                        let (label,detail,id,note)=r?; choices.push(NoteLinkChoice{label,detail:format!("Note · {detail}"),reference:NoteReference{version:1,kind:kind.into(),id,account:None,calendar:None,note:Some(note)}});
                    }
                }
                "event" => {
                    let mut q=c.prepare("SELECT body FROM note_event_cache WHERE EXISTS(SELECT 1 FROM calendar_accounts WHERE email=account AND status='connected') ORDER BY start")?;
                    for r in q.query_map([],|r|r.get::<_,String>(0))? {
                        let event:crate::models::ScheduleEvent=serde_json::from_str(&r?).map_err(super::serialization_error)?;
                        if !event.title.to_lowercase().contains(&query.to_lowercase()){continue;}
                        let reference=NoteReference{version:1,kind:kind.into(),id:event.id.clone(),account:Some(event.account_id.clone()),calendar:Some(event.calendar_id.clone()),note:None};
                        let detail=format!("{} · {} · {}",event.account_id,event.calendar_id,event.start);
                        choices.push(NoteLinkChoice{label:event.title.clone(),detail:format!("{detail}{}",if event.series_id.is_some(){" · This occurrence"}else{""}),reference:reference.clone()});
                        if let Some(series)=event.series_id { if !choices.iter().any(|c|c.reference.id==series && c.reference.account==reference.account && c.reference.calendar==reference.calendar) {choices.push(NoteLinkChoice{label:event.title,detail:format!("{detail} · Entire series"),reference:NoteReference{id:series,..reference}});} }
                    }
                }
                _ => return Err(DatabaseError::invalid("Unknown note link kind")),
            }
            choices.truncate(50);
            Ok(choices)
        })
    }
}

impl Database {
    pub fn forget_note_event(&self, account: &str, calendar: &str, id: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "DELETE FROM note_event_cache WHERE account=?1 AND calendar=?2 AND id=?3",
                params![account, calendar, id],
            )?;
            Ok(())
        })
    }
    pub fn note_task_warnings(&self) -> DbResult<Vec<(String, String, String)>> {
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT n.id,n.title,w.warning FROM note_index_warnings w JOIN notes n ON n.id=w.note_id WHERE n.trashed=0 ORDER BY n.id")?;
            let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        })
    }
}
