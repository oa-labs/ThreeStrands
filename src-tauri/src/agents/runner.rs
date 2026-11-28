//! Durable background runner for standing Inbox Assistants.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use super::analyzer::analyze_candidate;
use super::policy::*;
use super::{AiRunConfig, InboxAgentRun};
use crate::ai::{get_key, AiProvider};
use crate::db::Database;

#[derive(Clone, Debug)]
pub(crate) struct RunExecutionReport {
    pub scanned: i64,
    pub proposed: i64,
}

#[async_trait::async_trait]
pub(crate) trait Executor: Send + Sync {
    async fn execute(
        &self,
        db: &Database,
        run: &InboxAgentRun,
        ai: Option<&AiRunConfig>,
        owner: &str,
        now: i64,
        stopped: &AtomicBool,
    ) -> Result<RunExecutionReport, String>;
}

pub(crate) struct ProductionExecutor;

#[async_trait::async_trait]
impl Executor for ProductionExecutor {
    async fn execute(
        &self,
        db: &Database,
        run: &InboxAgentRun,
        ai: Option<&AiRunConfig>,
        owner: &str,
        now: i64,
        stopped: &AtomicBool,
    ) -> Result<RunExecutionReport, String> {
        let mut config = db
            .get_inbox_agent_config(run.agent_id)
            .map_err(|e| e.to_string())?;
        if run.account_id.is_some() {
            config.account_id = run.account_id.clone();
        }
        let batch = db
            .with_inbox_agent_candidates(&config, now)
            .map_err(|e| e.to_string())?;

        let mut scanned = 0i64;
        let mut proposed = 0i64;

        for det in batch.deterministic {
            if stopped.load(Ordering::SeqCst) {
                return Err("Execution interrupted".into());
            }
            scanned += 1;
            if db
                .save_inbox_agent_proposal(
                    run.agent_id,
                    &det.account_id,
                    &det.thread_id,
                    &det.thread_revision,
                    det.kind,
                    &det.headline,
                    &det.reason,
                    &det.payload,
                    &det.evidence,
                    now,
                )
                .map_err(|e| e.to_string())?
            {
                proposed += 1;
            }
        }

        if batch.ai_candidates.is_empty() {
            return Ok(RunExecutionReport { scanned, proposed });
        }

        let Some(ai_cfg) = ai.filter(|cfg| {
            !matches!(cfg.provider, AiProvider::None) && !cfg.model.trim().is_empty()
        }) else {
            return Err(
                "Configure an AI provider and model in Settings → AI Provider to scan emails."
                    .into(),
            );
        };
        let api_key = get_key()
            .map_err(|e| format!("Could not read AI API key: {e}"))?
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                "Save an AI API key in Settings → AI Provider before running Inbox Assistants."
                    .to_string()
            })?;

        let current_date = DateTime::<Utc>::from_timestamp(now, 0)
            .unwrap_or_else(Utc::now)
            .format("%Y-%m-%d")
            .to_string();

        for candidate in &batch.ai_candidates {
            if stopped.load(Ordering::SeqCst) {
                return Err("Execution interrupted".into());
            }
            let step_now = Utc::now().timestamp();
            let still_active = db
                .renew_inbox_agent_run_lease(&run.id, owner, step_now)
                .map_err(|e| e.to_string())?;
            if !still_active {
                return Err("Run was paused or cancelled".into());
            }
            scanned += 1;
            match analyze_candidate(run.agent_id, candidate, ai_cfg, &api_key, &current_date).await
            {
                Ok(Some(analyzed)) => {
                    let saved = db
                        .save_inbox_agent_proposal(
                            run.agent_id,
                            &candidate.account_id,
                            &candidate.thread_id,
                            &candidate.last_message_at,
                            analyzed.kind,
                            &analyzed.headline,
                            &analyzed.reason,
                            &analyzed.payload,
                            &analyzed.evidence,
                            step_now,
                        )
                        .map_err(|e| e.to_string())?;
                    if saved {
                        proposed += 1;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn!(
                        target: "inbox_assistants",
                        "assistant {} failed on thread {}: {error}",
                        run.agent_id.as_str(),
                        candidate.thread_id
                    );
                    if proposed == 0 && scanned == 1 {
                        return Err(error);
                    }
                }
            }
        }

        Ok(RunExecutionReport { scanned, proposed })
    }
}

#[derive(Clone)]
pub(crate) struct Runner {
    database: Arc<Database>,
    owner: String,
    stopped: Arc<AtomicBool>,
    wake: Arc<Notify>,
}

impl Runner {
    pub(crate) fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            owner: uuid::Uuid::new_v4().to_string(),
            stopped: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn start(&self) {
        for _ in 0..CONCURRENT_JOBS {
            let worker = self.clone();
            tauri::async_runtime::spawn(async move {
                while !worker.stopped.load(Ordering::SeqCst) {
                    match worker
                        .tick(&ProductionExecutor, || Utc::now().timestamp())
                        .await
                    {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(error) => log::warn!("Inbox assistant runner step error: {error}"),
                    }
                    tokio::select! {
                        _ = worker.wake.notified() => {},
                        _ = tokio::time::sleep(std::time::Duration::from_millis(POLL_MILLIS)) => {}
                    }
                }
            });
        }
    }

    pub(crate) fn request_stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        self.request_stop();
        let db = self.database.clone();
        let owner = self.owner.clone();
        tokio::task::spawn_blocking(move || {
            db.release_inbox_agent_owner(&owner, Utc::now().timestamp())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }

    pub(crate) async fn tick(
        &self,
        executor: &dyn Executor,
        clock: impl Fn() -> i64,
    ) -> Result<bool, String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let now = clock();
        let db = self.database.clone();
        tokio::task::spawn_blocking(move || db.enqueue_due_scheduled_inbox_agents(now))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

        let db = self.database.clone();
        let owner = self.owner.clone();
        let claimed = tokio::task::spawn_blocking(move || db.claim_inbox_agent_run(&owner, now))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

        let Some((run, ai)) = claimed else {
            return Ok(false);
        };

        if self.stopped.load(Ordering::SeqCst) {
            self.shutdown().await?;
            return Ok(false);
        }

        let exec_result = executor
            .execute(
                &self.database,
                &run,
                ai.as_ref(),
                &self.owner,
                now,
                &self.stopped,
            )
            .await;

        let db = self.database.clone();
        let owner = self.owner.clone();
        let finish_now = clock();
        let run_id = run.id.clone();
        tokio::task::spawn_blocking(move || {
            db.finish_inbox_agent_run(&run_id, &owner, exec_result, finish_now)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentId, ProposalKind, ProposalPayload};
    use crate::models::MailProviderKind;

    struct MockExecutor;

    #[async_trait::async_trait]
    impl Executor for MockExecutor {
        async fn execute(
            &self,
            db: &Database,
            run: &InboxAgentRun,
            _ai: Option<&AiRunConfig>,
            _owner: &str,
            now: i64,
            _stopped: &AtomicBool,
        ) -> Result<RunExecutionReport, String> {
            db.save_inbox_agent_proposal(
                run.agent_id,
                "me@example.com",
                "t1",
                "2026-11-01T00:00:00Z",
                ProposalKind::ArchiveAsExpired,
                "Expired offer",
                "Expired Nov 5",
                &ProposalPayload {
                    expired_date: Some("2026-11-05".into()),
                    ..Default::default()
                },
                &[],
                now,
            )
            .unwrap();
            Ok(RunExecutionReport {
                scanned: 1,
                proposed: 1,
            })
        }
    }

    #[tokio::test]
    async fn runner_claims_executes_and_completes_queued_runs() {
        let db = Arc::new(Database::open_memory());
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.test_connection()
            .unwrap()
            .execute(
                "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json, last_message_at, labels_json, account_id, last_received_at)
                 VALUES ('t1', 'pt1', 'Promo', 'Ends Nov 5', '[\"news@shop.example\"]', '2026-11-01T00:00:00Z', '[]', 'me@example.com', '2026-11-01T00:00:00Z')",
                [],
            )
            .unwrap();
        db.enqueue_inbox_agent_run(AgentId::ResolvedAndExpired, None, None, 1000)
            .unwrap();

        let runner = Runner::new(db.clone());
        let worked = runner.tick(&MockExecutor, || 1001).await.unwrap();
        assert!(worked);

        let snapshot = db.inbox_agents_snapshot(None).unwrap();
        assert!(snapshot.active_runs.is_empty());
        assert_eq!(snapshot.proposals.len(), 1);
        assert_eq!(snapshot.proposals[0].headline, "Expired offer");
        let cfg = snapshot
            .agents
            .iter()
            .find(|a| a.agent_id == AgentId::ResolvedAndExpired)
            .unwrap();
        assert_eq!(cfg.last_run_status.as_deref(), Some("completed"));
        assert_eq!(cfg.last_run_proposed, 1);
    }

    #[tokio::test]
    async fn sender_hygiene_runs_and_saves_proposals_without_an_ai_provider() {
        let db = Arc::new(Database::open_memory());
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        let conn = db.test_connection().unwrap();
        for index in 0..3 {
            conn.execute(
                "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json, last_message_at, labels_json, account_id, last_received_at, unread)
                 VALUES (?1, ?1, 'Deals', 'Sale', '[\"deals@shop.example\"]', '2026-11-01T00:00:00Z', '[\"INBOX\"]', 'me@example.com', '2026-11-01T00:00:00Z', 1)",
                [format!("h{index}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages(id, thread_id, sender, recipients_json, sent_at, body_html, body_text, unread)
                 VALUES (?1, ?2, 'deals@shop.example', '[\"me@example.com\"]', '2026-11-01T00:00:00Z', '', 'Sale', 1)",
                [format!("hm{index}"), format!("h{index}")],
            )
            .unwrap();
        }
        drop(conn);
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-25T10:00:00Z")
            .unwrap()
            .timestamp();
        // No AI configuration is passed or saved.
        db.enqueue_inbox_agent_run(AgentId::SenderHygiene, None, None, now)
            .unwrap();
        let runner = Runner::new(db.clone());
        assert!(runner.tick(&ProductionExecutor, || now).await.unwrap());

        let snapshot = db.inbox_agents_snapshot(None).unwrap();
        assert_eq!(snapshot.proposals.len(), 1);
        assert_eq!(snapshot.proposals[0].kind, ProposalKind::SenderHygiene);
        let cfg = snapshot
            .agents
            .iter()
            .find(|a| a.agent_id == AgentId::SenderHygiene)
            .unwrap();
        assert_eq!(cfg.last_run_status.as_deref(), Some("completed"));
        assert_eq!(cfg.last_run_proposed, 1);
    }
}
