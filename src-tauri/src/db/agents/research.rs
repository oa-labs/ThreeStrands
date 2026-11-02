//! SQLite owns grants, reservations and protocol checkpoints; no lock crosses I/O.
use super::*;
use crate::agents::research::*;
use serde_json::json;
fn load(c: &Connection, id: &str) -> DbResult<Option<Work>> {
    c.query_row(
        "SELECT record_json FROM agent_research WHERE task_id=?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(decode)
    .transpose()
}
fn work(c: &Connection, id: &str) -> DbResult<Work> {
    load(c, id)?.ok_or(DatabaseError::NotFound("Research work"))
}
fn save(c: &Connection, id: &str, w: &Work) -> DbResult<()> {
    if encode(w)?.len() > MAX_EVIDENCE {
        return Err(DatabaseError::invalid("Evidence allowance exhausted"));
    }
    c.execute("INSERT INTO agent_research(task_id,record_json,expires_at) VALUES (?1,?2,?3) ON CONFLICT(task_id) DO UPDATE SET record_json=excluded.record_json,expires_at=excluded.expires_at",params![id,encode(w)?,if w.next=="expired" {None} else {Some(w.expires_at)}])?;
    Ok(())
}
pub(super) fn summary(c: &Connection, id: &str) -> DbResult<Option<ResearchSummary>> {
    Ok(load(c, id)?.map(|w| ResearchSummary {
        citations: w
            .reports
            .iter()
            .map(|(id, r)| {
                (
                    id.clone(),
                    r.findings.iter().map(|f| f.source_id.clone()).collect(),
                )
            })
            .collect(),
        subjects: w.subjects,
        sources: w
            .sources
            .into_iter()
            .map(|mut s| {
                s.excerpt = s.excerpt.chars().take(MAX_QUOTE).collect();
                s
            })
            .collect(),
        usage: w.usage,
    }))
}
fn config(c: &Connection) -> DbResult<ResearchConfig> {
    let v: Option<String> = c
        .query_row(
            "SELECT value FROM compose_settings WHERE key='agent_research_config'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    v.map(decode).transpose().map(|v| v.unwrap_or_default())
}
fn ready(c: &Connection, search: bool) -> DbResult<()> {
    let cfg = config(c)?;
    if !enabled(c)? || !cfg.enabled {
        return Err(DatabaseError::invalid("research_disabled"));
    }
    let model_verified: bool = c
        .query_row(
            "SELECT value='true' FROM compose_settings WHERE key='agent_model_verified'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !model_verified {
        return Err(DatabaseError::invalid("model_connection_check_required"));
    }
    if !search {
        return Ok(());
    }
    if !cfg.search_enabled {
        return Err(DatabaseError::invalid("search_not_configured"));
    }
    text(&cfg.persistence_basis, MAX_PERSISTENCE_BASIS)
        .map_err(|_| DatabaseError::invalid("persistence_rights_required"))?;
    if !cfg.verified {
        return Err(DatabaseError::invalid("connection_check_required"));
    }
    Ok(())
}
fn current(c: &Connection, l: &Lease, now: i64) -> DbResult<bool> {
    match run(c, &l.run.task_id) {
        Ok(r) => Ok(r.state == RunState::Running
            && r.generation == l.run.generation
            && r.owner == l.run.owner
            && r.lease_until.is_some_and(|t| t > now)
            && ready(c, !work(c, &l.run.task_id)?.discussion).is_ok()),
        Err(DatabaseError::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}
impl Database {
    pub(crate) fn revoke_research_model_check(&self) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "DELETE FROM compose_settings WHERE key='agent_model_verified'",
                [],
            )?;
            Ok(())
        })
    }
    pub(crate) fn research_config(&self) -> DbResult<ResearchConfig> {
        self.with_connection(config)
    }
    pub(crate) fn set_research_config(
        &self,
        cfg: &ResearchConfig,
        verified: bool,
        now: i64,
    ) -> DbResult<()> {
        if cfg.persistence_basis.chars().count() > MAX_PERSISTENCE_BASIS {
            return Err(DatabaseError::invalid("Persistence basis is too long"));
        }
        let mut cfg = cfg.clone();
        cfg.verified = verified;
        self.with_transaction(|c| {
            c.execute("INSERT INTO compose_settings(key,value) VALUES ('agent_research_config',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[encode(&cfg)?])?;
            if verified {c.execute("INSERT INTO compose_settings(key,value) VALUES ('agent_model_verified','true') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[])?;}
            let runs:Vec<AgentRun>=records(c,"SELECT r.record_json FROM agent_runs r JOIN agent_research w ON r.task_id=w.task_id WHERE r.state IN ('queued','running','waiting_for_input') AND ?1 IS NOT NULL","all")?;
            // Any configuration edit invalidates in-flight grants. Resuming is explicit.
            for mut r in runs {charge_time(&mut r,now);invalidate(&mut r,RunState::Paused,Some("configuration_changed"));save_run(c,&r)?;}Ok(())
        })
    }
    pub(crate) fn create_research_task(
        &self,
        request: &ResearchRequest,
        now: i64,
    ) -> DbResult<AgentDetail> {
        bounded(&request.task.objective, MAX_OBJECTIVE)?;
        validate_subjects(&request.subjects).map_err(DatabaseError::invalid)?;
        if !request.consent || request.task.workflow != RESEARCH_WORKFLOW {
            return Err(DatabaseError::invalid(
                "Research disclosure must be accepted",
            ));
        }
        self.with_transaction(|c| {
            ready(c, true)?;
            let d = create_task(c, &request.task, now)?;
            // Phase 3 sends the user's objective only, not mail bodies or addresses.
            // A source link is navigation, never an implicit permission to read mail.
            save(
                c,
                &d.task.id,
                &Work::new(request.subjects.clone(), &d.task.objective, now),
            )?;
            detail(c, &d.task.id)
        })
    }
    pub(crate) fn research_work(&self, l: &Lease, now: i64) -> DbResult<Option<Work>> {
        self.with_connection(|c| {
            if !current(c, l, now)? {
                return Ok(None);
            }
            Ok(Some(work(c, &l.run.task_id)?))
        })
    }
    pub(crate) fn reserve_research(&self, l: &Lease, kind: &str, now: i64) -> DbResult<Work> {
        self.with_transaction(|c| {
            if !current(c, l, now)? {
                return Err(DatabaseError::invalid(
                    "Research grant is no longer current",
                ));
            }
            let mut w = work(c, &l.run.task_id)?;
            if now >= w.expires_at {
                return Err(DatabaseError::invalid("evidence_expired"));
            }
            if w.dispatched_generation == Some(l.run.generation) {
                return Err(DatabaseError::invalid("Dispatch already reserved"));
            }
            match kind {
                "count" => {
                    if w.usage.count_calls >= MAX_MODEL_CALLS * (MAX_RETRIES as u64 + 1) {
                        return Err(DatabaseError::invalid("budget_exhausted"));
                    }
                    w.usage.count_calls += 1;
                }
                "model" => {
                    if w.usage.model_calls >= MAX_MODEL_CALLS
                        || w.counted_input == 0
                        || w.counted_input > MAX_INPUT_CALL
                        || w.usage.reserved_input + MAX_INPUT_CALL > MAX_INPUT_TASK
                        || w.usage.reserved_output + MAX_OUTPUT_CALL > MAX_OUTPUT_TASK
                    {
                        return Err(DatabaseError::invalid("budget_exhausted"));
                    }
                    w.usage.model_calls += 1;
                    w.usage.reserved_input += MAX_INPUT_CALL;
                    w.usage.reserved_output += MAX_OUTPUT_CALL;
                    w.usage.unknown_usage += 1;
                }
                "search" => {
                    if w.usage.search_calls >= MAX_SEARCH_CALLS {
                        return Err(DatabaseError::invalid("budget_exhausted"));
                    }
                    w.usage.search_calls += 1;
                    w.usage.unknown_search_cost += 1;
                }
                "page" => {
                    if w.usage.page_calls >= MAX_PAGE_CALLS {
                        return Err(DatabaseError::invalid("budget_exhausted"));
                    }
                    w.usage.page_calls += 1;
                }
                _ => return Err(DatabaseError::invalid("Unknown dispatch")),
            }
            w.dispatched_generation = Some(l.run.generation);
            // Reservation and dispatch intent commit BEFORE calling a provider.
            save(c, &l.run.task_id, &w)?;
            Ok(w)
        })
    }
    pub(crate) fn expire_research_evidence(&self, now: i64) -> DbResult<()> {
        self.with_transaction(|c| {
            let mut stmt=c.prepare("SELECT task_id,record_json FROM agent_research WHERE expires_at<=?1 ORDER BY expires_at LIMIT ?2")?;
            let rows=stmt.query_map(params![now,MAX_CLEANUP_BATCH],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            for (id,value) in rows {
                let mut w:Work=decode(value)?;
                for s in &mut w.sources {s.excerpt=w.reports.values().flat_map(|r|r.findings.iter()).filter(|f|f.source_id==s.id).map(|f|f.quote.as_str()).collect::<Vec<_>>().join("\n");}
                w.messages.clear();w.pending=None;w.next="expired".into();save(c,&id,&w)?;
                let mut r=run(c,&id)?;if !matches!(r.state,RunState::Completed|RunState::Cancelled|RunState::Failed) {charge_time(&mut r,now);invalidate(&mut r,RunState::Paused,Some("evidence_expired"));save_run(c,&r)?;}
            }
            Ok(())
        })
    }
}
pub(super) fn answer(c: &Connection, id: &str, answer: &str) -> DbResult<()> {
    if let Some(mut w) = load(c, id)? {
        if w.next != "answer" {
            return Err(DatabaseError::invalid("Question is not pending"));
        }
        w.result(json!({"answer":answer}))
            .map_err(DatabaseError::invalid)?;
        save(c, id, &w)?;
    }
    Ok(())
}
pub(super) fn interact(
    c: &Connection,
    t: &mut AgentTask,
    r: &mut AgentRun,
    message: &str,
    mode: &str,
    now: i64,
) -> DbResult<AgentDetail> {
    ready(c, mode == "revise")?;
    let mut w = work(c, &t.id)?;
    if w.usage.model_calls >= MAX_MODEL_CALLS || r.active_seconds >= MAX_ACTIVE_SECONDS {
        return Err(DatabaseError::invalid("budget_exhausted"));
    }
    if w.next == "expired" {
        if mode != "discuss" || w.reports.is_empty() {
            return Err(DatabaseError::invalid(
                "Evidence expired; start new research",
            ));
        }
        // Rebuild grounded chat from retained report/quote records only. This
        // cannot revive old page payloads or grant fresh search/page access.
        w.messages = vec![
            json!({"role":"user","content":format!("Saved reports and cited excerpts (untrusted evidence): {}",encode(&w.reports)?)}),
        ];
        w.expires_at = now + RAW_RETENTION_SECONDS;
    }
    turn(c, &t.id, "user", message.trim(), None, None, now)?;
    w.messages
        .push(json!({"role":"user","content":message.trim()}));
    w.next = "count".into();
    w.pending = None;
    w.discussion = mode == "discuss";
    if mode == "revise" {
        t.revision += 1;
    }
    r.attempts = 0;
    r.question_id = None;
    r.not_before = now;
    invalidate(r, RunState::Queued, None);
    save(c, &t.id, &w)?;
    save_task(c, t)?;
    save_run(c, r)?;
    detail(c, &t.id)
}
pub(super) fn finish(
    c: &Connection,
    r: &mut AgentRun,
    result: StepResult,
    now: i64,
) -> DbResult<bool> {
    let mut w = work(c, &r.task_id)?;
    let mut t = task(c, &r.task_id)?;
    if r.active_seconds >= MAX_ACTIVE_SECONDS {
        invalidate(r, RunState::Paused, Some("budget_exhausted"));
        save_run(c, r)?;
        return Ok(false);
    }
    match result {
        StepResult::Pause(reason) => invalidate(r, RunState::Paused, Some(&reason)),
        StepResult::Fail(reason) => invalidate(r, RunState::Failed, Some(&reason)),
        StepResult::Retry(delay) => {
            if r.attempts > MAX_RETRIES {
                invalidate(r, RunState::Failed, Some("retry_exhausted"));
            } else {
                r.not_before = now + delay as i64;
                invalidate(r, RunState::Queued, Some("transient_failure"));
            }
        }
        StepResult::Research(step) => {
            let valid = match &step {
                ResearchStep::Count(_) => {
                    w.next == "count" && w.dispatched_generation == Some(r.generation)
                }
                ResearchStep::Model(_) => {
                    w.next == "model"
                        && w.dispatched_generation == Some(r.generation)
                        && w.usage.unknown_usage > 0
                }
                ResearchStep::Sources(_, _) => {
                    w.next == "action"
                        && w.pending.as_ref().is_some_and(|a| a.name == "search_web")
                        && w.dispatched_generation == Some(r.generation)
                }
                ResearchStep::Page(_) => {
                    w.next == "action"
                        && w.pending.as_ref().is_some_and(|a| a.name == "read_page")
                        && w.dispatched_generation == Some(r.generation)
                }
                ResearchStep::Question(_) => {
                    w.next == "action" && w.pending.as_ref().is_some_and(|a| a.name == "ask_user")
                }
                ResearchStep::Publish(_) => {
                    w.next == "action"
                        && w.pending
                            .as_ref()
                            .is_some_and(|a| a.name == "publish_artifact")
                }
            };
            if !valid {
                return Err(DatabaseError::invalid("Research checkpoint mismatch"));
            }
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM agent_turns WHERE task_id=?1",
                [&t.id],
                |r| r.get(0),
            )?;
            if count >= MAX_TURNS {
                invalidate(r, RunState::Paused, Some("conversation_limit"));
                save_run(c, r)?;
                return Ok(true);
            }
            match step {
                ResearchStep::Count(n) => {
                    if n == 0 || n > MAX_INPUT_CALL {
                        invalidate(r, RunState::Paused, Some("context_limit"));
                        save_run(c, r)?;
                        return Ok(true);
                    }
                    w.counted_input = n;
                    w.next = "model".into();
                }
                ResearchStep::Model(v) => {
                    let call = parse_model(&v).map_err(DatabaseError::invalid)?;
                    let input = v["usage"]["input_tokens"].as_u64().unwrap();
                    let output = v["usage"]["output_tokens"].as_u64().unwrap();
                    if input > MAX_INPUT_CALL || output > MAX_OUTPUT_CALL {
                        return Err(DatabaseError::invalid("Provider exceeded reservation"));
                    }
                    w.usage.reserved_input -= MAX_INPUT_CALL - input;
                    w.usage.reserved_output -= MAX_OUTPUT_CALL - output;
                    w.usage.input_tokens += input;
                    w.usage.output_tokens += output;
                    w.usage.unknown_usage -= 1;
                    record_activity(c, "anthropic", MODEL, input, output, None, now)?;
                    if let Some(call) = call {
                        validate_call(&w, &call).map_err(DatabaseError::invalid)?;
                        w.messages
                            .push(json!({"role":"assistant","content":v["content"]}));
                        w.pending = Some(call);
                        w.next = "action".into();
                    } else {
                        if !w.discussion {
                            invalidate(r, RunState::Failed, Some("no_report_published"));
                            save_run(c, r)?;
                            save(c, &t.id, &w)?;
                            return Ok(true);
                        }
                        let response = v["content"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        turn(c, &t.id, "assistant", &response, None, None, now)?;
                        w.messages
                            .push(json!({"role":"assistant","content":v["content"]}));
                        r.state = RunState::Completed;
                        t.unread = true;
                        w.next = "done".into();
                    }
                }
                ResearchStep::Sources(s, cost) => {
                    w.sources.extend(s.clone());
                    if let Some(cost) = cost {
                        w.usage.search_cost_usd =
                            Some(w.usage.search_cost_usd.unwrap_or(0.0) + cost);
                        w.usage.unknown_search_cost -= 1;
                    }
                    w.result(json!({"sources":s}))
                        .map_err(DatabaseError::invalid)?;
                    record_activity(c, "exa", "auto", 0, 0, cost, now)?;
                }
                ResearchStep::Page(s) => {
                    w.sources.push(s.clone());
                    w.result(json!({"source":s}))
                        .map_err(DatabaseError::invalid)?;
                }
                ResearchStep::Question(question) => {
                    let id = Uuid::new_v4().to_string();
                    turn(
                        c,
                        &t.id,
                        "assistant",
                        &question,
                        Some(id.clone()),
                        None,
                        now,
                    )?;
                    r.question_id = Some(id);
                    r.state = RunState::WaitingForInput;
                    w.next = "answer".into();
                    t.unread = true;
                }
                ResearchStep::Publish(report) => {
                    validate_report(&report, &w.sources).map_err(DatabaseError::invalid)?;
                    let mut body = format!(
                        "Assessment (agent judgment)\n{}\n\nFindings\n",
                        report.summary
                    );
                    for f in &report.findings {
                        let s = w.sources.iter().find(|s| s.id == f.source_id).unwrap();
                        body.push_str(&format!(
                            "\n{}\nSource: {} ({})\nQuote: {}\n",
                            f.text,
                            s.title,
                            if s.kind == "page" {
                                "retrieved page"
                            } else {
                                "search excerpt"
                            },
                            f.quote.replace('\n', " ")
                        ));
                    }
                    body.push_str("\nUncertainties\n");
                    body.push_str(&report.uncertainties.join("\n"));
                    let version: i64 = c.query_row(
                        "SELECT COALESCE(MAX(version),0)+1 FROM agent_artifacts WHERE task_id=?1",
                        [&t.id],
                        |r| r.get(0),
                    )?;
                    let a = AgentArtifact {
                        id: Uuid::new_v4().to_string(),
                        version,
                        kind: "research_report".into(),
                        body,
                        not_researched: false,
                        created_at: now,
                    };
                    c.execute("INSERT INTO agent_artifacts(id,task_id,version,record_json) VALUES (?1,?2,?3,?4)",params![a.id,t.id,version,encode(&a)?])?;
                    w.reports.insert(a.id.clone(), report);
                    w.result(json!({"published":a.id}))
                        .map_err(DatabaseError::invalid)?;
                    w.next = "done".into();
                    r.state = RunState::Completed;
                    t.unread = true;
                }
            }
            r.checkpoint += 1;
            r.attempts = 0;
            r.reason = None;
            r.not_before = now;
            if r.state == RunState::Running {
                r.state = RunState::Queued;
            }
            r.owner = None;
            r.lease_until = None;
            r.started_at = None;
        }
        _ => return Err(DatabaseError::invalid("Unexpected research step")),
    }
    c.execute(
        "UPDATE agent_steps SET status='committed' WHERE run_id=?1 AND generation=?2",
        params![r.id, r.generation],
    )?;
    save(c, &t.id, &w)?;
    save_task(c, &t)?;
    save_run(c, r)?;
    Ok(true)
}
fn record_activity(
    c: &Connection,
    provider: &str,
    model: &str,
    input: u64,
    output: u64,
    cost: Option<f64>,
    now: i64,
) -> DbResult<()> {
    let at = chrono::DateTime::from_timestamp(now, 0)
        .ok_or_else(|| DatabaseError::invalid("Invalid time"))?;
    // No subject, queries, source titles, objectives or excerpts enter AI activity.
    c.execute("INSERT INTO ai_activity(at,feature,provider,model,subject,message_count,input_tokens,output_tokens) VALUES (?1,'agent_research',?2,?3,NULL,NULL,?4,?5)",params![at.to_rfc3339(),provider,model,input as i64,output as i64])?;
    c.execute("INSERT INTO ai_usage(day,provider,model,requests,input_tokens,output_tokens,reported_cost_requests,reported_cost_usd) VALUES (?1,?2,?3,1,?4,?5,?6,?7) ON CONFLICT(day,provider,model) DO UPDATE SET requests=requests+1,input_tokens=input_tokens+excluded.input_tokens,output_tokens=output_tokens+excluded.output_tokens,reported_cost_requests=reported_cost_requests+excluded.reported_cost_requests,reported_cost_usd=reported_cost_usd+excluded.reported_cost_usd",params![at.format("%Y-%m-%d").to_string(),provider,model,input as i64,output as i64,i64::from(cost.is_some()),cost.unwrap_or(0.0)])?;
    let cutoff = at
        .checked_sub_days(chrono::Days::new(crate::db::ai::AI_USAGE_RETENTION_DAYS))
        .unwrap_or(at);
    c.execute(
        "DELETE FROM ai_usage WHERE day<=?1",
        [cutoff.format("%Y-%m-%d").to_string()],
    )?;
    c.execute("DELETE FROM ai_activity WHERE at<?1 OR id NOT IN (SELECT id FROM ai_activity ORDER BY at DESC,id DESC LIMIT ?2)",params![cutoff.to_rfc3339(),crate::db::ai::MAX_AI_ACTIVITY_ENTRIES])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn setup() -> (Database, String) {
        let db = Database::open_memory();
        db.adopt_mail_account("you@example.com", crate::models::MailProviderKind::Gmail)
            .unwrap();
        db.set_agent_runner_enabled(true, 0).unwrap();
        db.set_research_config(
            &ResearchConfig {
                enabled: true,
                search_enabled: true,
                persistence_basis: "Applicable API agreement".into(),
                verified: false,
            },
            true,
            0,
        )
        .unwrap();
        let d = db
            .create_research_task(
                &ResearchRequest {
                    task: CreateAgentTask {
                        account_id: "you@example.com".into(),
                        objective: "Evaluate Example Summit".into(),
                        source_thread_id: None,
                        linked_task_id: None,
                        workflow: RESEARCH_WORKFLOW.into(),
                    },
                    subjects: vec!["Example Summit".into()],
                    consent: true,
                },
                0,
            )
            .unwrap();
        (db, d.task.id)
    }
    fn step(db: &Database, kind: Option<&str>, result: ResearchStep) {
        let lease = db.claim_agent_run("test", 1).unwrap().unwrap();
        if let Some(kind) = kind {
            db.reserve_research(&lease, kind, 1).unwrap();
        }
        assert!(db
            .finish_agent_step(&lease, StepResult::Research(result), 1)
            .unwrap());
    }
    fn model(db: &Database, id: &str, name: &str, input: Value) {
        step(db, Some("count"), ResearchStep::Count(2000));
        let d = db.get_agent_task(id).unwrap();
        let call_id = format!("call_{}", d.run.checkpoint);
        step(
            db,
            Some("model"),
            ResearchStep::Model(
                json!({"type":"message","role":"assistant","model":MODEL,"content":[{"type":"tool_use","id":call_id,"name":name,"input":input}],"stop_reason":"tool_use","usage":{"input_tokens":1200,"output_tokens":80}}),
            ),
        );
    }
    fn publish(db: &Database, id: &str) -> String {
        model(
            db,
            id,
            "search_web",
            json!({"query":"Example Summit official speakers"}),
        );
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../docs/agent-integrations/fixtures/search.json"
        ))
        .unwrap();
        let (sources, cost) =
            crate::agents::transport::parse_search(&fixture["response"], 1).unwrap();
        let source_id = sources[0].id.clone();
        step(db, Some("search"), ResearchStep::Sources(sources, cost));
        let report = Report {
            summary: "Agent judgment based on limited evidence".into(),
            findings: vec![Finding {
                text: "A speaker list is published".into(),
                source_id,
                quote: "Published speaker list; attendance not established.".into(),
            }],
            uncertainties: vec!["Audience not verified".into()],
        };
        model(
            db,
            id,
            "publish_artifact",
            serde_json::to_value(&report).unwrap(),
        );
        step(db, None, ResearchStep::Publish(report));
        db.get_agent_task(id)
            .unwrap()
            .artifacts
            .last()
            .unwrap()
            .id
            .clone()
    }
    #[test]
    fn recorded_responses_publish_cited_versions_with_cumulative_usage_and_content_free_activity() {
        let (db, id) = setup();
        let a = publish(&db, &id);
        let d = db.get_agent_task(&id).unwrap();
        assert_eq!(d.run.state, RunState::Completed);
        assert!(!d.artifacts[0].not_researched);
        assert_eq!(d.research.as_ref().unwrap().citations[&a].len(), 1);
        assert_eq!(d.research.as_ref().unwrap().usage.model_calls, 2);
        assert_eq!(d.research.as_ref().unwrap().usage.search_calls, 1);
        assert_eq!(d.research.as_ref().unwrap().usage.unknown_usage, 0);
        for entry in db.ai_activity(100).unwrap() {
            assert_eq!(entry.feature, "agent_research");
            assert!(entry.subject.is_none());
        }
        db.interact_agent_task(&id, "Revise for sales", "revise", 1)
            .unwrap();
        let b = publish(&db, &id);
        let d = db.get_agent_task(&id).unwrap();
        assert_ne!(a, b);
        assert_eq!(d.artifacts.len(), 2);
        assert_eq!(d.artifacts[0].version, 1);
        assert_eq!(d.artifacts[1].version, 2);
        assert_eq!(d.research.unwrap().usage.model_calls, 4);
    }
    #[test]
    fn questions_wait_without_dispatch_and_pair_the_answer_before_the_next_model_message() {
        let (db, id) = setup();
        model(
            &db,
            &id,
            "ask_user",
            json!({"question":"What outcome matters?"}),
        );
        step(
            &db,
            None,
            ResearchStep::Question("What outcome matters?".into()),
        );
        let d = db.get_agent_task(&id).unwrap();
        assert_eq!(d.run.state, RunState::WaitingForInput);
        assert!(db.claim_agent_run("test", 2).unwrap().is_none());
        let q = d.run.question_id.unwrap();
        db.answer_agent_question(&id, &q, "Customer introductions", 2)
            .unwrap();
        let w = db.with_connection(|c| work(c, &id)).unwrap();
        assert_eq!(w.messages[2]["content"][0]["type"], "tool_result");
        assert!(w.messages[2]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Customer introductions"));
        assert!(db
            .answer_agent_question(&id, &q, "Other answer", 2)
            .is_err());
    }
    #[test]
    fn reservation_survives_crash_and_late_results_cannot_publish_after_cancellation_or_deletion() {
        let (db, id) = setup();
        step(&db, Some("count"), ResearchStep::Count(2000));
        let l = db.claim_agent_run("worker", 1).unwrap().unwrap();
        db.reserve_research(&l, "model", 1).unwrap();
        assert!(db.reserve_research(&l, "model", 1).is_err());
        let next = db.claim_agent_run("recovery", 62).unwrap().unwrap();
        let w = db.reserve_research(&next, "model", 62).unwrap();
        assert_eq!(w.usage.model_calls, 2);
        assert_eq!(w.usage.unknown_usage, 2);
        assert_eq!(w.usage.reserved_output, 2 * MAX_OUTPUT_CALL);
        db.control_agent_task(&id, "cancel", 62).unwrap();
        assert!(db.reserve_research(&next, "search", 62).is_err());
        assert!(!db
            .finish_agent_step(&next, StepResult::Research(ResearchStep::Count(2000)), 62)
            .unwrap());
        assert!(db.get_agent_task(&id).unwrap().artifacts.is_empty());
        db.delete_agent_task(&id).unwrap();
        assert!(!db
            .finish_agent_step(&l, StepResult::Research(ResearchStep::Count(2000)), 63)
            .unwrap());
        assert!(db.with_connection(|c| load(c, &id)).unwrap().is_none());
    }
    #[test]
    fn limits_grants_expiry_and_grounded_chat_do_not_reset_budget_or_require_search() {
        let (db, id) = setup();
        publish(&db, &id);
        let cfg = db.research_config().unwrap();
        db.set_research_config(
            &ResearchConfig {
                search_enabled: false,
                ..cfg
            },
            false,
            1,
        )
        .unwrap();
        db.interact_agent_task(&id, "Explain the saved result", "discuss", 1)
            .unwrap();
        let w = db.with_connection(|c| work(c, &id)).unwrap();
        assert!(w.discussion);
        assert!(w.request(false).get("tools").is_none());
        step(&db, Some("count"), ResearchStep::Count(2000));
        step(
            &db,
            Some("model"),
            ResearchStep::Model(
                json!({"type":"message","role":"assistant","model":MODEL,"content":[{"type":"text","text":"Based on the saved search excerpt, attendance remains unverified."}],"stop_reason":"end_turn","usage":{"input_tokens":1200,"output_tokens":40}}),
            ),
        );
        assert_eq!(db.get_agent_task(&id).unwrap().artifacts.len(), 1);
        assert!(db
            .interact_agent_task(&id, "Research again", "revise", 1)
            .is_err());
        db.expire_research_evidence(RAW_RETENTION_SECONDS + 2)
            .unwrap();
        let d = db.get_agent_task(&id).unwrap();
        assert_eq!(d.artifacts.len(), 1);
        let w = db.with_connection(|c| work(c, &id)).unwrap();
        assert!(w.messages.is_empty());
        assert_eq!(
            w.sources[0].excerpt,
            "Published speaker list; attendance not established."
        );
        db.interact_agent_task(
            &id,
            "Explain the saved quotes",
            "discuss",
            RAW_RETENTION_SECONDS + 2,
        )
        .unwrap();
        let w = db.with_connection(|c| work(c, &id)).unwrap();
        assert!(w.discussion);
        assert!(w.request(false).get("tools").is_none());
        assert!(w.messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("Published speaker list"));
        assert_eq!(w.usage.model_calls, 3);
    }
    #[test]
    fn exhausted_token_and_call_allowances_fail_before_dispatch() {
        for (field, value) in [
            ("reserved_input", MAX_INPUT_TASK),
            ("reserved_output", MAX_OUTPUT_TASK),
            ("model_calls", MAX_MODEL_CALLS),
        ] {
            let (db, id) = setup();
            step(&db, Some("count"), ResearchStep::Count(2000));
            db.with_transaction(|c| {
                let mut w = work(c, &id)?;
                match field {
                    "reserved_input" => w.usage.reserved_input = value,
                    "reserved_output" => w.usage.reserved_output = value,
                    _ => w.usage.model_calls = value,
                };
                save(c, &id, &w)
            })
            .unwrap();
            let l = db.claim_agent_run("test", 1).unwrap().unwrap();
            assert!(db.reserve_research(&l, "model", 1).is_err());
        }
    }
    #[test]
    fn previous_schema_upgrades_without_changing_fixture_jobs_or_enabling_research() {
        let path = crate::db::test_support::TempDbPath::new();
        let id;
        {
            let db = Database::open(&path.path).unwrap();
            db.adopt_mail_account("you@example.com", crate::models::MailProviderKind::Gmail)
                .unwrap();
            db.set_agent_runner_enabled(true, 0).unwrap();
            id = db
                .create_agent_task(
                    &CreateAgentTask {
                        account_id: "you@example.com".into(),
                        objective: "Existing fixture".into(),
                        workflow: WORKFLOW.into(),
                        source_thread_id: None,
                        linked_task_id: None,
                    },
                    0,
                )
                .unwrap()
                .task
                .id;
            db.connection()
                .unwrap()
                .execute_batch("DROP TABLE agent_research; PRAGMA user_version=95;")
                .unwrap();
        }
        let db = Database::open(&path.path).unwrap();
        assert_eq!(
            db.get_agent_task(&id).unwrap().task.objective,
            "Existing fixture"
        );
        assert!(!db.research_config().unwrap().enabled);
        assert!(db.get_agent_task(&id).unwrap().research.is_none());
    }
    #[test]
    fn search_and_page_limits_allow_the_last_call_but_no_more_and_configuration_revokes_inflight_work(
    ) {
        for (kind, limit) in [("search", MAX_SEARCH_CALLS), ("page", MAX_PAGE_CALLS)] {
            let (db, id) = setup();
            db.with_transaction(|c| {
                let mut w = work(c, &id)?;
                if kind == "search" {
                    w.usage.search_calls = limit - 1;
                } else {
                    w.usage.page_calls = limit - 1;
                }
                save(c, &id, &w)
            })
            .unwrap();
            let l = db.claim_agent_run("test", 1).unwrap().unwrap();
            let w = db.reserve_research(&l, kind, 1).unwrap();
            assert_eq!(
                if kind == "search" {
                    w.usage.search_calls
                } else {
                    w.usage.page_calls
                },
                limit
            );
            db.finish_agent_step(&l, StepResult::Retry(2), 1).unwrap();
            let next = db.claim_agent_run("test", 3).unwrap().unwrap();
            assert!(db.reserve_research(&next, kind, 3).is_err());
            db.set_research_config(&ResearchConfig::default(), false, 3)
                .unwrap();
            assert!(!db.agent_lease_current(&next, 3).unwrap());
            assert!(!db
                .finish_agent_step(&next, StepResult::Fail("late".into()), 3)
                .unwrap());
        }
    }
    #[test]
    fn research_activity_preserves_the_existing_retention_and_content_free_log_contract() {
        let db = Database::open_memory();
        db.with_transaction(|c| {
            for n in 0..=crate::db::ai::MAX_AI_ACTIVITY_ENTRIES {
                record_activity(c, "anthropic", MODEL, 1, 1, None, n)?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            db.ai_activity(1000).unwrap().len(),
            crate::db::ai::MAX_AI_ACTIVITY_ENTRIES as usize
        );
        db.with_transaction(|c| record_activity(c, "exa", "auto", 0, 0, None, 100 * 24 * 60 * 60))
            .unwrap();
        let entries = db.ai_activity(1000).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].subject.is_none());
    }
}
