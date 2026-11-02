#[cfg(test)]
use super::fixture_step;
use super::{policy::*, Lease, StepResult};
use crate::db::Database;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Notify;

#[async_trait::async_trait]
pub(crate) trait Executor: Send + Sync {
    async fn execute(&self, lease: &Lease) -> Result<StepResult, ()>;
}
#[cfg(test)]
struct Fixture;
#[cfg(test)]
#[async_trait::async_trait]
impl Executor for Fixture {
    async fn execute(&self, lease: &Lease) -> Result<StepResult, ()> {
        Ok(fixture_step(lease.run.checkpoint))
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
    pub(crate) fn start(&self) {
        for _ in 0..CONCURRENT_JOBS {
            let worker = self.clone();
            tauri::async_runtime::spawn(async move {
                while !worker.stopped.load(Ordering::SeqCst) {
                    match worker
                        .tick(
                            &super::service::Production {
                                database: worker.database.clone(),
                            },
                            || chrono::Utc::now().timestamp(),
                        )
                        .await
                    {
                        Ok(true) => continue,
                        Err(error) => log::warn!("Delegated work stopped a step: {error}"),
                        Ok(false) => {}
                    }
                    tokio::select! { _=worker.wake.notified()=>{}, _=tokio::time::sleep(std::time::Duration::from_millis(POLL_MILLIS))=>{} }
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
            db.release_agent_owner(&owner, chrono::Utc::now().timestamp())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
    /// Injectable clock and executor keep recovery/cancellation tests independent
    /// of wall time and paid services. No database lock crosses executor.await.
    pub(crate) async fn tick(
        &self,
        executor: &dyn Executor,
        clock: impl Fn() -> i64,
    ) -> Result<bool, String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let db = self.database.clone();
        let owner = self.owner.clone();
        let now = clock();
        let cleanup = db.clone();
        tokio::task::spawn_blocking(move || cleanup.expire_research_evidence(now))
            .await
            .map_err(|_| "Research cleanup interrupted".to_string())?
            .map_err(|e| e.to_string())?;
        let lease = tokio::task::spawn_blocking(move || db.claim_agent_run(&owner, now))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let Some(lease) = lease else {
            return Ok(false);
        };
        // Recheck after the blocking claim so shutdown cannot start a new tool.
        if self.stopped.load(Ordering::SeqCst) {
            self.shutdown().await?;
            return Ok(false);
        }
        let interrupted = self.wake.notified();
        tokio::pin!(interrupted);
        interrupted.as_mut().enable();
        if self.stopped.load(Ordering::SeqCst) {
            self.shutdown().await?;
            return Ok(false);
        }
        let db_check = self.database.clone();
        let lease_check = lease.clone();
        let wait_started = tokio::time::Instant::now();
        let remaining = (lease.run.lease_until.unwrap_or(now) - now).max(0) as u64;
        let check_current = async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(CANCELLATION_MILLIS)).await;
                let db = db_check.clone();
                let l = lease_check.clone();
                let elapsed = wait_started.elapsed().as_secs();
                let valid = tokio::task::spawn_blocking(move || {
                    db.agent_lease_current(&l, now).unwrap_or(false) && elapsed < remaining
                })
                .await
                .unwrap_or(false);
                if !valid {
                    break;
                }
            }
        };
        let result = tokio::select! {
            _=check_current=>return Ok(false),
            result=executor.execute(&lease)=>result,
            _=interrupted=> { self.shutdown().await?;return Ok(false); }
        };
        if self.stopped.load(Ordering::SeqCst) {
            self.shutdown().await?;
            return Ok(false);
        }
        let result = result.unwrap_or(StepResult::TransientFailure);
        let db = self.database.clone();
        let now = clock();
        tokio::task::spawn_blocking(move || {
            let finished = db.finish_agent_step(&lease, result, now);
            if finished.is_err()
                && db
                    .agent_workflow(&lease.run.task_id)
                    .is_ok_and(|w| w == RESEARCH_WORKFLOW)
            {
                return db.finish_agent_step(
                    &lease,
                    StepResult::Fail("response_invalid".into()),
                    now,
                );
            }
            finished
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::RunState;
    use crate::{agents::CreateAgentTask, models::MailProviderKind};
    struct Blocked(Arc<Notify>, Arc<Notify>);
    #[async_trait::async_trait]
    impl Executor for Blocked {
        async fn execute(&self, _: &Lease) -> Result<StepResult, ()> {
            self.0.notify_one();
            self.1.notified().await;
            Ok(StepResult::Prepared)
        }
    }
    #[tokio::test]
    async fn cancellation_during_executor_await_cannot_publish_and_does_not_lock_database() {
        let db = Arc::new(Database::open_memory());
        db.adopt_mail_account("you@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.set_agent_runner_enabled(true, 0).unwrap();
        let d = db
            .create_agent_task(
                &CreateAgentTask {
                    account_id: "you@example.com".into(),
                    objective: "Fixture".into(),
                    workflow: WORKFLOW.into(),
                    source_thread_id: None,
                    linked_task_id: None,
                },
                0,
            )
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runner = Runner::new(db.clone());
        let r = runner.clone();
        let tool = Blocked(started.clone(), release.clone());
        let task = tokio::spawn(async move { r.tick(&tool, || 1).await });
        started.notified().await;
        db.control_agent_task(&d.task.id, "cancel", 1).unwrap();
        release.notify_one();
        assert!(!task.await.unwrap().unwrap());
        let d = db.get_agent_task(&d.task.id).unwrap();
        assert_eq!(d.run.state, RunState::Cancelled);
        assert!(d.artifacts.is_empty());
        assert_eq!(d.turns.len(), 1);
    }
    #[tokio::test]
    async fn shutdown_releases_work_and_invalidates_inflight_generation() {
        let db = Arc::new(Database::open_memory());
        db.adopt_mail_account("you@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.set_agent_runner_enabled(true, 0).unwrap();
        let d = db
            .create_agent_task(
                &CreateAgentTask {
                    account_id: "you@example.com".into(),
                    objective: "Fixture".into(),
                    workflow: WORKFLOW.into(),
                    source_thread_id: None,
                    linked_task_id: None,
                },
                0,
            )
            .unwrap();
        let runner = Runner::new(db.clone());
        let old = db.claim_agent_run(&runner.owner, 0).unwrap().unwrap();
        runner.shutdown().await.unwrap();
        assert!(!db.finish_agent_step(&old, StepResult::Prepared, 1).unwrap());
        assert_eq!(
            db.get_agent_task(&d.task.id).unwrap().run.state,
            RunState::Queued
        );
        assert!(!runner.tick(&Fixture, || 1).await.unwrap());
    }
}
