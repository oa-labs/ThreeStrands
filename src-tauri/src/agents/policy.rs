//! Central bounds for local demonstration and read-only research.
pub(crate) const WORKFLOW: &str = "fixture_research_v1";
pub(crate) const MAX_OBJECTIVE: usize = 4000;
pub(crate) const MAX_TURNS: i64 = 100;
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

pub(crate) const RESEARCH_WORKFLOW: &str = "web_research_v1";
pub(crate) const MAX_RESEARCH_DISPATCHES: i64 = 120;
pub(crate) const MAX_MODEL_CALLS: u64 = 20;
pub(crate) const MAX_SEARCH_CALLS: u64 = 20;
pub(crate) const MAX_PAGE_CALLS: u64 = 30;
pub(crate) const MAX_INPUT_CALL: u64 = 24000;
pub(crate) const MAX_OUTPUT_CALL: u64 = 4096;
pub(crate) const MAX_INPUT_TASK: u64 = 200000;
pub(crate) const MAX_OUTPUT_TASK: u64 = 40000;
pub(crate) const MAX_QUERY: usize = 400;
pub(crate) const MAX_RESPONSE: usize = 2097152;
pub(crate) const MAX_EXTRACTED: usize = 30000;
pub(crate) const MAX_EVIDENCE: usize = 5242880;
pub(crate) const MAX_REDIRECTS: usize = 3;
pub(crate) const CONNECT_SECONDS: u64 = 10;
pub(crate) const REQUEST_SECONDS: u64 = 60;
pub(crate) const RAW_RETENTION_SECONDS: i64 = 7*24*60*60;
pub(crate) const MAX_RETRY_DELAY: u64 = 300;
pub(crate) const MAX_SUBJECTS: usize = 5;
pub(crate) const MAX_SUBJECT: usize = 200;
pub(crate) const MAX_REPORT: usize = 12000;
pub(crate) const MAX_FINDING: usize = 2000;
pub(crate) const MAX_FINDINGS: usize = 30;
pub(crate) const MAX_QUOTE: usize = 1000;
pub(crate) const MAX_CALL_ID: usize = 200;
pub(crate) const MAX_URL: usize = 2048;
pub(crate) const MAX_TITLE: usize = 500;
pub(crate) const SEARCH_RESULTS: usize = 5;
pub(crate) const MAX_PROTOCOL_BYTES: usize = 100000;
pub(crate) const MAX_SECRET: usize = 1000;
pub(crate) const SEARCH_INTERVAL_SECONDS: u64 = 1;
pub(crate) const CANCELLATION_MILLIS: u64 = 200;
pub(crate) const REQUEST_GRACE_SECONDS: u64 = 2;
pub(crate) const MAX_DNS_ANSWERS: usize = 32;
pub(crate) const MAX_CLEANUP_BATCH: i64 = 10;

/// Public, non-secret contract returned to the UI; native parity tests bind
/// execution limits to these frozen values rather than duplicating UI numbers.
pub(crate) fn client_contract()->serde_json::Value {
    serde_json::from_str(include_str!("../../../docs/agent-integrations/contract-v1.json")).expect("embedded research contract is valid JSON")
}
