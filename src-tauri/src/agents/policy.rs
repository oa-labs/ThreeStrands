//! Central phase-1 bounds. Real provider/token/URL policy arrives in phase 3.
pub(crate) const WORKFLOW: &str = "fixture_research_v1";
pub(crate) const MAX_OBJECTIVE: usize = 4000;
pub(crate) const MAX_ANSWER: usize = 8000;
pub(crate) const MAX_LIST: i64 = 100;
pub(crate) const CONCURRENT_JOBS: i64 = 2;
pub(crate) const LEASE_SECONDS: i64 = 60;
pub(crate) const MAX_DISPATCHES: i64 = 20;
pub(crate) const MAX_ACTIVE_SECONDS: i64 = 900;
pub(crate) const MAX_RETRIES: i64 = 2;
pub(crate) const RETRY_BASE_SECONDS: i64 = 2;
pub(crate) const POLL_MILLIS: u64 = 1000;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scheduler_allowances_match_the_frozen_phase_zero_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/agent-integrations/contract-v1.json"
        ))
        .unwrap();
        for (key, value) in [
            ("concurrentJobs", CONCURRENT_JOBS),
            ("modelCallsPerTask", MAX_DISPATCHES),
            ("activeSecondsPerTask", MAX_ACTIVE_SECONDS),
            ("retriesPerStep", MAX_RETRIES),
            ("retryBaseSeconds", RETRY_BASE_SECONDS),
        ] {
            assert_eq!(contract["limits"][key].as_i64(), Some(value), "{key}");
        }
    }
}
