//! Native request authority: cancellation, concurrency, conservative spending
//! reservations, and a bounded, content-free provenance journal. Device-local.
use super::manifest::{fingerprint, ContextManifest};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

pub const MAX_CONCURRENT: usize = 2;
pub const MAX_PENDING: usize = 8;
pub const QUEUE_SECONDS: u64 = 30;
pub const TOTAL_SECONDS: u64 = 150;
pub const MAX_TRANSIENT_RETRIES: usize = 2;
pub const MAX_RETRY_SECONDS: u64 = 5;
pub const MAX_RECORDS: usize = 500;
pub const RETENTION_DAYS: i64 = 30;
pub const MAX_JOURNAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_CANCELLED: usize = 1000;
const CANCEL_RETENTION_SECONDS: i64 = 300;
const INPUT_FRAMING_TOKENS: usize = 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
}
impl ModelPrice {
    fn valid(&self) -> bool {
        [self.input_per_million, self.output_per_million]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0 && *value <= 1_000_000.0)
    }
    fn cost(&self, input: u64, output: u64) -> f64 {
        (input as f64 * self.input_per_million + output as f64 * self.output_per_million)
            / 1_000_000.0
    }
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestControl {
    pub request_id: String,
    pub price: Option<ModelPrice>,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestPolicy {
    pub daily_budget_usd: Option<f64>,
}
impl RequestPolicy {
    fn validate(&self) -> Result<(), String> {
        if self
            .daily_budget_usd
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1_000_000.0)
        {
            return Err("Enter a positive daily AI budget, or leave it blank".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub attempt_id: String,
    pub request_id: String,
    pub at: String,
    pub feature: String,
    pub provider: String,
    pub model: String,
    pub endpoint_revision: String,
    pub prompt_version: u32,
    pub schema_version: u32,
    pub prompt_revision: String,
    pub schema_revision: Option<String>,
    pub context: ContextManifest,
    pub response_revision: Option<String>,
    pub outcome: String,
    pub request_outcome: String,
    pub charged_usd: f64,
    pub price: Option<ModelPrice>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRequest {
    pub request_id: String,
    pub feature: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSnapshot {
    pub policy: RequestPolicy,
    pub day: String,
    pub charged_usd: f64,
    pub active: Vec<ActiveRequest>,
    pub records: Vec<RequestRecord>,
    pub max_concurrent: usize,
    pub max_pending: usize,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    policy: RequestPolicy,
    day: String,
    charged_usd: f64,
    records: Vec<RequestRecord>,
}
struct State {
    saved: Saved,
    path: Option<PathBuf>,
    unavailable: bool,
    active: HashMap<String, (String, watch::Sender<bool>)>,
    cancelled: HashMap<String, i64>,
}
#[derive(Clone)]
pub struct Runtime {
    state: Arc<Mutex<State>>,
    permits: Arc<Semaphore>,
}
pub static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new());
#[derive(Clone)]
struct Running {
    id: String,
    price: Option<ModelPrice>,
    runtime: Runtime,
    latest: Arc<Mutex<Option<RequestRecord>>>,
}
tokio::task_local! { static RUNNING: Running; }
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
impl State {
    fn prune(&mut self) {
        if self.saved.day != today() {
            self.saved.day = today();
            self.saved.charged_usd = 0.0;
        }
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
        self.saved.records.retain(|record| record.at >= cutoff);
        if self.saved.records.len() > MAX_RECORDS {
            self.saved
                .records
                .drain(..self.saved.records.len() - MAX_RECORDS);
        }
        while serde_json::to_vec(&self.saved).map_or(true, |data| data.len() > MAX_JOURNAL_BYTES)
            && !self.saved.records.is_empty()
        {
            self.saved.records.remove(0);
        }
        let cutoff = chrono::Utc::now().timestamp() - CANCEL_RETENTION_SECONDS;
        self.cancelled.retain(|_, at| *at >= cutoff);
    }
    fn persist(&mut self) -> Result<(), String> {
        self.prune();
        if self.unavailable {
            return Err("AI request controls could not be loaded; no request was sent".into());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let temporary = path.with_extension("tmp");
        let data =
            serde_json::to_vec(&self.saved).map_err(|_| "Could not save AI request controls")?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options
            .open(&temporary)
            .map_err(|_| "Could not save AI request controls")?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Could not save AI request controls")?;
        std::fs::rename(temporary, path)
            .map_err(|_| "Could not save AI request controls".to_string())
    }
}
impl Runtime {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                saved: Saved::default(),
                path: None,
                unavailable: false,
                active: HashMap::new(),
                cancelled: HashMap::new(),
            })),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }
    pub fn initialize(&self, path: PathBuf) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(saved)
                    if saved.policy.validate().is_ok()
                        && saved.charged_usd.is_finite()
                        && saved.charged_usd >= 0.0 =>
                {
                    state.saved = saved
                }
                _ => {
                    state.unavailable = true;
                    return Err(
                        "AI request controls are unreadable; AI requests are disabled".into(),
                    );
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                state.unavailable = true;
                return Err(
                    "AI request controls could not be read; AI requests are disabled".into(),
                );
            }
        }
        state.path = Some(path);
        // Reservations for interrupted requests remain charged after a restart.
        for record in &mut state.saved.records {
            if record.request_outcome == "running" {
                record.request_outcome = "interrupted".into();
            }
        }
        state.prune();
        state.persist().inspect_err(|_| state.unavailable = true)
    }
    pub fn snapshot(&self) -> Result<RequestSnapshot, String> {
        let mut state = self.state.lock().unwrap();
        state.prune();
        if state.unavailable {
            return Err("AI request controls are unavailable".into());
        }
        Ok(RequestSnapshot {
            policy: state.saved.policy.clone(),
            day: state.saved.day.clone(),
            charged_usd: state.saved.charged_usd,
            active: state
                .active
                .iter()
                .map(|(id, (feature, _))| ActiveRequest {
                    request_id: id.clone(),
                    feature: feature.clone(),
                })
                .collect(),
            records: state.saved.records.iter().rev().cloned().collect(),
            max_concurrent: MAX_CONCURRENT,
            max_pending: MAX_PENDING,
        })
    }
    pub fn configure(&self, policy: RequestPolicy) -> Result<(), String> {
        policy.validate()?;
        let mut state = self.state.lock().unwrap();
        let old = std::mem::replace(&mut state.saved.policy, policy);
        if let Err(error) = state.persist() {
            state.saved.policy = old;
            return Err(error);
        }
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        uuid::Uuid::parse_str(id).map_err(|_| "Invalid AI request ID")?;
        let mut state = self.state.lock().unwrap();
        state.prune();
        if let Some((_, cancel)) = state.active.get(id) {
            cancel.send_replace(true);
        } else {
            if state.cancelled.len() == MAX_CANCELLED {
                return Err("Too many pending cancellations".into());
            }
            state
                .cancelled
                .insert(id.into(), chrono::Utc::now().timestamp());
        }
        Ok(())
    }
    pub async fn run<T>(
        &self,
        feature: &str,
        control: Option<RequestControl>,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.run_recorded(feature, control, Box::pin(future))
            .await
            .map(|(value, _)| value)
    }
    pub async fn run_recorded<T>(
        &self,
        feature: &str,
        control: Option<RequestControl>,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<(T, Option<RequestRecord>), String> {
        let control = control.unwrap_or_else(|| RequestControl {
            request_id: uuid::Uuid::new_v4().to_string(),
            price: None,
        });
        uuid::Uuid::parse_str(&control.request_id).map_err(|_| "Invalid AI request ID")?;
        if control.price.as_ref().is_some_and(|price| !price.valid()) {
            return Err("Invalid AI model prices".into());
        }
        let (cancel, mut cancelled) = watch::channel(false);
        {
            let mut state = self.state.lock().unwrap();
            state.prune();
            if state.unavailable {
                return Err("AI request controls are unavailable; no request was sent".into());
            }
            if state.cancelled.contains_key(&control.request_id) {
                return Err("AI request cancelled".into());
            }
            if state.active.len() >= MAX_PENDING {
                return Err("Too many AI requests; wait for one to finish".into());
            }
            if state.active.contains_key(&control.request_id) {
                return Err("This AI request is already running".into());
            }
            state
                .active
                .insert(control.request_id.clone(), (feature.into(), cancel));
        }
        let latest = Arc::new(Mutex::new(None));
        let mut guard = RequestGuard {
            runtime: self.clone(),
            id: control.request_id.clone(),
            outcome: "cancelled",
        };
        let operation = async {
            let _permit = tokio::time::timeout(
                Duration::from_secs(QUEUE_SECONDS),
                self.permits.clone().acquire_owned(),
            )
            .await
            .map_err(|_| "AI request queue is busy; try again".to_string())?
            .map_err(|_| "AI request queue is unavailable".to_string())?;
            RUNNING
                .scope(
                    Running {
                        id: control.request_id,
                        price: control.price,
                        runtime: self.clone(),
                        latest: latest.clone(),
                    },
                    async {
                        tokio::time::timeout(Duration::from_secs(TOTAL_SECONDS), future)
                            .await
                            .map_err(|_| "AI request timed out".to_string())?
                    },
                )
                .await
        };
        let result = tokio::select! {
            biased;
            _ = async { while !*cancelled.borrow_and_update() { if cancelled.changed().await.is_err() { break; } } } => Err("AI request cancelled".to_string()),
            result = operation => result,
        };
        guard.outcome = if result.is_ok() {
            "succeeded"
        } else if result
            .as_ref()
            .err()
            .is_some_and(|error| error == "AI request cancelled")
        {
            "cancelled"
        } else {
            "failed"
        };
        if let Some(record) = latest.lock().unwrap().as_mut() {
            record.request_outcome = guard.outcome.into();
        }
        result.map(|value| (value, latest.lock().unwrap().clone()))
    }
    pub fn reserve(
        &self,
        mut record: RequestRecord,
        input_bytes: usize,
        output_tokens: usize,
    ) -> Result<Reservation, String> {
        let running = RUNNING
            .try_with(Clone::clone)
            .map_err(|_| "AI request has no native lifecycle")?;
        let mut state = self.state.lock().unwrap();
        state.prune();
        // UTF-8 bytes are a conservative estimate for supported text tokenizers;
        // framing/schema overhead and the full output allowance are reserved too.
        let estimate = running.price.as_ref().map(|price| {
            price.cost(
                input_bytes.saturating_add(INPUT_FRAMING_TOKENS) as u64,
                output_tokens as u64,
            )
        });
        if state.saved.policy.daily_budget_usd.is_some() && estimate.is_none() {
            return Err(
                "Set this model's input and output prices before using the daily AI budget".into(),
            );
        }
        let reserved = estimate.unwrap_or(0.0);
        if state
            .saved
            .policy
            .daily_budget_usd
            .is_some_and(|cap| state.saved.charged_usd + reserved > cap)
        {
            return Err("This request would exceed today's estimated AI budget".into());
        }
        record.request_id = running.id;
        record.price = running.price;
        record.charged_usd = reserved;
        let previous = state.saved.clone();
        state.saved.charged_usd += reserved;
        state.saved.records.push(record.clone());
        state.prune();
        if let Err(error) = state.persist() {
            state.saved = previous;
            return Err(error);
        }
        *running.latest.lock().unwrap() = Some(record.clone());
        Ok(Reservation {
            runtime: self.clone(),
            latest: running.latest.clone(),
            request_id: record.request_id,
            attempt_id: record.attempt_id,
            reserved,
            day: state.saved.day.clone(),
            price: record.price,
        })
    }
}
struct RequestGuard {
    runtime: Runtime,
    id: String,
    outcome: &'static str,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let mut state = self.runtime.state.lock().unwrap();
        state.active.remove(&self.id);
        for record in &mut state.saved.records {
            if record.request_id == self.id {
                record.request_outcome = self.outcome.into();
            }
        }
        if state.persist().is_err() {
            state.unavailable = true;
        }
    }
}
pub struct Reservation {
    runtime: Runtime,
    latest: Arc<Mutex<Option<RequestRecord>>>,
    request_id: String,
    attempt_id: String,
    reserved: f64,
    day: String,
    price: Option<ModelPrice>,
}
impl Reservation {
    pub fn failed(self, outcome: &str) -> Result<(), String> {
        let mut state = self.runtime.state.lock().unwrap();
        if let Some(record) = state
            .saved
            .records
            .iter_mut()
            .rev()
            .find(|record| record.attempt_id == self.attempt_id)
        {
            record.outcome = outcome.into();
        }
        if let Some(record) = self.latest.lock().unwrap().as_mut() {
            record.outcome = outcome.into();
        }
        state.persist().inspect_err(|_| state.unavailable = true)
    }
    pub fn finish(
        self,
        body: &str,
        input: u64,
        output: u64,
        reported_cost: Option<f64>,
        has_usage: bool,
        outcome: &str,
    ) -> Result<(), String> {
        let actual = reported_cost
            .or_else(|| {
                if has_usage {
                    self.price.as_ref().map(|price| price.cost(input, output))
                } else {
                    None
                }
            })
            .unwrap_or(self.reserved);
        let mut state = self.runtime.state.lock().unwrap();
        if state.saved.day == self.day {
            state.saved.charged_usd = (state.saved.charged_usd - self.reserved).max(0.0) + actual;
        }
        if let Some(record) = state.saved.records.iter_mut().rev().find(|record| {
            record.request_id == self.request_id && record.attempt_id == self.attempt_id
        }) {
            record.response_revision = Some(fingerprint(body.as_bytes()));
            record.outcome = outcome.into();
            record.charged_usd = actual;
        }
        if let Some(record) = self.latest.lock().unwrap().as_mut() {
            record.response_revision = Some(fingerprint(body.as_bytes()));
            record.outcome = outcome.into();
            record.charged_usd = actual;
        }
        state.persist().inspect_err(|_| state.unavailable = true)
    }
}
pub fn running() -> bool {
    RUNNING.try_with(|_| ()).is_ok()
}

pub fn reserve_current(
    record: RequestRecord,
    input_bytes: usize,
    output_tokens: usize,
) -> Result<Reservation, String> {
    RUNNING
        .try_with(|running| running.runtime.reserve(record, input_bytes, output_tokens))
        .map_err(|_| "AI request has no native lifecycle")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::manifest::{PreparedContext, SourceScope};
    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ai-requests-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn control(price: Option<ModelPrice>) -> RequestControl {
        RequestControl {
            request_id: uuid::Uuid::new_v4().to_string(),
            price,
        }
    }
    fn record() -> RequestRecord {
        RequestRecord {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            request_id: String::new(),
            at: chrono::Utc::now().to_rfc3339(),
            feature: "brief".into(),
            provider: "custom".into(),
            model: "test".into(),
            endpoint_revision: fingerprint(b"test endpoint"),
            prompt_version: 1,
            schema_version: 1,
            prompt_revision: fingerprint(b"system"),
            schema_revision: Some(fingerprint(b"schema")),
            context: PreparedContext::new(
                r#"{"messages":[{"sourceMessageId":"m1","bodyText":"PRIVATE_MAIL_TEXT"}]}"#,
                SourceScope::default(),
            )
            .unwrap()
            .manifest,
            response_revision: None,
            outcome: "dispatched".into(),
            request_outcome: "running".into(),
            charged_usd: 0.0,
            price: None,
        }
    }
    #[tokio::test]
    async fn bounded_queue_and_cancellation_cover_running_waiting_and_not_yet_registered_work() {
        let runtime = Runtime::new();
        let mut tasks = Vec::new();
        let controls: Vec<_> = (0..MAX_PENDING).map(|_| control(None)).collect();
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for config in &controls {
            let runtime = runtime.clone();
            let config = config.clone();
            let started = started.clone();
            tasks.push(tokio::spawn(async move {
                runtime
                    .run("brief", Some(config), async {
                        started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::future::pending::<Result<(), String>>().await
                    })
                    .await
            }));
        }
        for _ in 0..50 {
            if runtime.snapshot().unwrap().active.len() == MAX_PENDING {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(runtime.snapshot().unwrap().active.len(), MAX_PENDING);
        assert_eq!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            MAX_CONCURRENT
        );
        let excess = runtime
            .run("brief", None, async { Ok(()) })
            .await
            .unwrap_err();
        assert!(excess.contains("Too many"));
        runtime
            .cancel(&controls[MAX_PENDING - 1].request_id)
            .unwrap();
        for config in &controls {
            runtime.cancel(&config.request_id).unwrap();
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap_err(), "AI request cancelled");
        }
        assert!(runtime.snapshot().unwrap().active.is_empty());
        let config = control(None);
        runtime.cancel(&config.request_id).unwrap();
        assert_eq!(
            runtime
                .run("brief", Some(config), async {
                    panic!("cancelled work ran");
                    #[allow(unreachable_code)]
                    Ok::<_, String>(())
                })
                .await
                .unwrap_err(),
            "AI request cancelled"
        );
        assert_eq!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            MAX_CONCURRENT
        );
    }
    #[tokio::test(start_paused = true)]
    async fn native_deadlines_release_the_slot_and_do_not_leave_orphan_work() {
        let runtime = Runtime::new();
        let result = runtime
            .run("brief", None, async {
                tokio::time::sleep(Duration::from_secs(TOTAL_SECONDS + 1)).await;
                Ok(())
            })
            .await;
        assert_eq!(result.unwrap_err(), "AI request timed out");
        let permits = runtime
            .permits
            .clone()
            .acquire_many_owned(MAX_CONCURRENT as u32)
            .await
            .unwrap();
        let result = runtime.run("brief", None, async { Ok(()) }).await;
        assert!(result.unwrap_err().contains("queue is busy"));
        drop(permits);
        assert!(runtime.snapshot().unwrap().active.is_empty());
        assert!(runtime.run("brief", None, async { Ok(()) }).await.is_ok());
    }
    #[tokio::test]
    async fn reservations_are_atomic_at_the_exact_budget_boundary_and_require_prices() {
        let price = ModelPrice {
            input_per_million: 1.0,
            output_per_million: 1.0,
        };
        let exact = price.cost((INPUT_FRAMING_TOKENS + 10) as u64, 20);
        for cap in [exact - 0.0000001, exact, exact + 0.0000001] {
            let runtime = Runtime::new();
            runtime
                .configure(RequestPolicy {
                    daily_budget_usd: Some(cap),
                })
                .unwrap();
            let result = runtime
                .run("brief", Some(control(Some(price.clone()))), async {
                    runtime.reserve(record(), 10, 20).map(|_| ())
                })
                .await;
            assert_eq!(result.is_ok(), cap >= exact);
        }
        let runtime = Runtime::new();
        runtime
            .configure(RequestPolicy {
                daily_budget_usd: Some(1.0),
            })
            .unwrap();
        let error = runtime
            .run("brief", None, async {
                runtime.reserve(record(), 10, 20).map(|_| ())
            })
            .await
            .unwrap_err();
        assert!(error.contains("prices"));
        assert_eq!(runtime.snapshot().unwrap().charged_usd, 0.0);
    }
    #[tokio::test]
    async fn interrupted_reservations_survive_restart_and_successful_usage_settles_them_without_text(
    ) {
        let directory = TestDirectory::new();
        let path = directory.path().join("requests.json");
        let runtime = Runtime::new();
        runtime.initialize(path.clone()).unwrap();
        runtime
            .configure(RequestPolicy {
                daily_budget_usd: Some(1.0),
            })
            .unwrap();
        let price = ModelPrice {
            input_per_million: 1.0,
            output_per_million: 1.0,
        };
        runtime
            .run("brief", Some(control(Some(price.clone()))), async {
                let _reservation = runtime.reserve(record(), 10, 20)?;
                Err::<(), _>("unknown billing".into())
            })
            .await
            .unwrap_err();
        let reserved = runtime.snapshot().unwrap().charged_usd;
        let restored = Runtime::new();
        restored.initialize(path.clone()).unwrap();
        assert_eq!(restored.snapshot().unwrap().charged_usd, reserved);
        restored
            .run("brief", Some(control(Some(price))), async {
                let reservation = restored.reserve(record(), 10, 20)?;
                reservation.finish("PRIVATE_RESPONSE", 3, 4, None, true, "received")
            })
            .await
            .unwrap();
        let snapshot = restored.snapshot().unwrap();
        assert!((snapshot.charged_usd - (reserved + 0.000007)).abs() < 1e-12);
        assert_eq!(snapshot.records[0].request_outcome, "succeeded");
        assert_eq!(snapshot.records[1].request_outcome, "failed");
        let json = std::fs::read_to_string(path).unwrap();
        assert!(!json.contains("PRIVATE_MAIL_TEXT"));
        assert!(!json.contains("PRIVATE_RESPONSE"));
        assert!(snapshot.records[0].response_revision.is_some());
    }
    #[tokio::test]
    async fn result_provenance_survives_journal_eviction_before_completion() {
        let runtime = Runtime::new();
        let (_, provenance) = runtime
            .run_recorded("brief", None, async {
                let reservation = runtime.reserve(record(), 10, 20)?;
                runtime.state.lock().unwrap().saved.records.clear();
                reservation.finish("response", 1, 2, None, true, "received")?;
                Ok(())
            })
            .await
            .unwrap();
        let provenance = provenance.unwrap();
        assert_eq!(provenance.request_outcome, "succeeded");
        assert_eq!(provenance.outcome, "received");
        assert_eq!(provenance.response_revision, Some(fingerprint(b"response")));
    }

    #[test]
    fn journal_count_byte_and_age_bounds_do_not_erase_spending_reservations() {
        let runtime = Runtime::new();
        let mut state = runtime.state.lock().unwrap();
        state.saved.day = today();
        state.saved.charged_usd = 0.5;
        state.saved.records = vec![record(); MAX_RECORDS + 1];
        state.prune();
        assert_eq!(state.saved.records.len(), MAX_RECORDS);
        let mut large = record();
        large.context.sources = vec![large.context.sources[0].clone(); 2048];
        for source in &mut large.context.sources {
            source.path = "x".repeat(900);
        }
        state.saved.records = vec![large; 8];
        state.prune();
        assert!(serde_json::to_vec(&state.saved).unwrap().len() <= MAX_JOURNAL_BYTES);
        assert_eq!(state.saved.charged_usd, 0.5);
        for record in &mut state.saved.records {
            record.at = "2000-01-01T00:00:00Z".into();
        }
        state.prune();
        assert!(state.saved.records.is_empty());
        assert_eq!(state.saved.charged_usd, 0.5);
        state.saved.day = "2000-01-01".into();
        state.prune();
        assert_eq!(state.saved.charged_usd, 0.0);
    }

    #[test]
    fn corrupt_or_unwritable_control_storage_fails_closed_and_policy_changes_do_not_reset_spend() {
        let directory = TestDirectory::new();
        let path = directory.path().join("requests.json");
        std::fs::write(&path, "broken").unwrap();
        let runtime = Runtime::new();
        assert!(runtime.initialize(path).is_err());
        assert!(runtime.snapshot().is_err());
        let unwritable = Runtime::new();
        assert!(unwritable
            .initialize(directory.path().join("missing").join("controls.json"))
            .is_err());
        assert!(unwritable.snapshot().is_err());
        let runtime = Runtime::new();
        {
            let mut state = runtime.state.lock().unwrap();
            state.saved.day = today();
            state.saved.charged_usd = 0.5;
        }
        runtime
            .configure(RequestPolicy {
                daily_budget_usd: Some(1.0),
            })
            .unwrap();
        assert_eq!(runtime.state.lock().unwrap().saved.charged_usd, 0.5);
        for value in [-1.0, 0.0, f64::NAN, f64::INFINITY] {
            assert!(runtime
                .configure(RequestPolicy {
                    daily_budget_usd: Some(value)
                })
                .is_err());
        }
    }
}
