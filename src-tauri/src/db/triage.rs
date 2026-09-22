//! Local triage observation persistence.

use super::*;

impl Database {
    /// Resolves sender identity from cached mail rather than trusting input.
    pub fn record_triage_event(&self, event: &TriageEvent) -> DbResult<()> {
        match (&event.kind, &event.action) {
            (
                TriageEventKind::Open | TriageEventKind::Close | TriageEventKind::Response,
                Some(_),
            ) => return Err("Open and close triage events cannot have an action".into()),
            (TriageEventKind::Disposition | TriageEventKind::Restore, None) => {
                return Err("Disposition and restore triage events require an action".into())
            }
            _ => {}
        }
        self.with_transaction(|transaction| {
            let Some((account_id, sender_email, sender_domain)) =
                sender_identity_for_thread(transaction, &event.thread_id)?
            else {
                // Nothing was written; committing the read-only transaction
                // is equivalent to the rollback-on-drop this replaced.
                return Ok(());
            };
            let dwell_ms = event.dwell_ms.map(|value| value.clamp(0, 86_400_000));
            transaction.execute(
                "INSERT INTO triage_events(
                    id, account_id, thread_id, sender_email, sender_domain,
                    event_kind, context, action, opened, dwell_ms, scrolled,
                    batch, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    Uuid::new_v4().to_string(),
                    account_id,
                    event.thread_id,
                    sender_email,
                    sender_domain,
                    triage_event_kind_name(&event.kind),
                    triage_context_name(&event.context),
                    event.action.as_ref().map(triage_action_name),
                    event.opened,
                    dwell_ms,
                    event.scrolled,
                    event.batch,
                    Utc::now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }
}
