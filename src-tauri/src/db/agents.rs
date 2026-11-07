//! Atomic local agent state. No provider calls or personal-task mutations.
use super::{Database, DatabaseError, DbResult};
use crate::agents::{policy::*, *};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;
mod research;

fn encode<T: Serialize>(v: &T) -> DbResult<String> {
    serde_json::to_string(v).map_err(|e| DatabaseError::Serialization(e.to_string()))
}
fn decode<T: DeserializeOwned>(v: String) -> DbResult<T> {
    serde_json::from_str(&v).map_err(|e| DatabaseError::corrupt(e.to_string()))
}
fn bounded(value: &str, max: usize) -> DbResult<()> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(DatabaseError::invalid("Agent text is empty or too long"));
    }
    Ok(())
}
fn enabled(c: &Connection) -> DbResult<bool> {
    Ok(c.query_row(
        "SELECT value='true' FROM compose_settings WHERE key='agent_runner_enabled'",
        [],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(false))
}
fn run(c: &Connection, task_id: &str) -> DbResult<AgentRun> {
    let value: String = c
        .query_row(
            "SELECT record_json FROM agent_runs WHERE task_id=?1 ORDER BY rowid DESC LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(DatabaseError::NotFound("Delegated task"))?;
    decode(value)
}
fn save_run(c: &Connection, r: &AgentRun) -> DbResult<()> {
    c.execute("UPDATE agent_runs SET state=?2,generation=?3,lease_until=?4,not_before=?5,record_json=?6 WHERE id=?1",
        params![r.id,r.state.key(),r.generation,r.lease_until,r.not_before,encode(r)?])?;
    Ok(())
}
fn task(c: &Connection, id: &str) -> DbResult<AgentTask> {
    let (json, source, linked): (String, Option<String>, Option<String>) = c
        .query_row(
            "SELECT record_json,source_thread_id,linked_task_id FROM agent_tasks WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(DatabaseError::NotFound("Delegated task"))?;
    let mut value: AgentTask = decode(json)?;
    value.source_thread_id = source;
    value.linked_task_id = linked;
    Ok(value)
}
fn save_task(c: &Connection, t: &AgentTask) -> DbResult<()> {
    c.execute(
        "UPDATE agent_tasks SET record_json=?2 WHERE id=?1",
        params![t.id, encode(t)?],
    )?;
    Ok(())
}
fn records<T: DeserializeOwned>(c: &Connection, sql: &str, id: &str) -> DbResult<Vec<T>> {
    let mut s = c.prepare(sql)?;
    let rows = s
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(decode).collect()
}
fn detail(c: &Connection, id: &str) -> DbResult<AgentDetail> {
    Ok(AgentDetail {
        research: research::summary(c, id)?,
        task: task(c, id)?,
        run: run(c, id)?,
        turns: records(
            c,
            "SELECT record_json FROM agent_turns WHERE task_id=?1 ORDER BY id",
            id,
        )?,
        artifacts: records(
            c,
            "SELECT record_json FROM agent_artifacts WHERE task_id=?1 ORDER BY version",
            id,
        )?,
    })
}
fn turn(
    c: &Connection,
    task_id: &str,
    role: &str,
    text: &str,
    question: Option<String>,
    answer: Option<String>,
    now: i64,
) -> DbResult<()> {
    let t = AgentTurn {
        id: Uuid::new_v4().to_string(),
        role: role.into(),
        text: text.into(),
        question_id: question,
        answer_to: answer.clone(),
        created_at: now,
    };
    c.execute(
        "INSERT INTO agent_turns(task_id,answer_to,record_json) VALUES (?1,?2,?3)",
        params![task_id, answer, encode(&t)?],
    )?;
    Ok(())
}
fn invalidate(r: &mut AgentRun, state: RunState, reason: Option<&str>) {
    r.state = state;
    r.reason = reason.map(str::to_string);
    r.generation += 1;
    r.owner = None;
    r.lease_until = None;
    r.started_at = None;
}
fn dispatch_limit(c: &Connection, id: &str) -> DbResult<i64> {
    Ok(if task(c, id)?.workflow == RESEARCH_WORKFLOW {
        MAX_RESEARCH_DISPATCHES
    } else {
        MAX_DISPATCHES
    })
}
fn charge_time(r: &mut AgentRun, now: i64) {
    if let Some(start) = r.started_at {
        let reserved = (r.lease_until.unwrap_or(start + LEASE_SECONDS) - start).max(0);
        r.active_seconds += (now - start).clamp(0, reserved);
    }
}

fn create_task(c: &Connection, request: &CreateAgentTask, now: i64) -> DbResult<AgentDetail> {
    if !enabled(c)? {
        return Err(DatabaseError::invalid("Delegated work is disabled"));
    }
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE email=?1 AND on_this_device=1)",
        [&request.account_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(DatabaseError::invalid(
            "Account is not available on this device",
        ));
    }
    if let Some(id) = &request.source_thread_id {
        let account: Option<String> = c
            .query_row("SELECT account_id FROM threads WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        if account.as_deref() != Some(&request.account_id) {
            return Err(DatabaseError::invalid(
                "Source conversation must belong to the account",
            ));
        }
    }
    if let Some(id) = &request.linked_task_id {
        let account: Option<String> = c
            .query_row("SELECT account_id FROM tasks WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        if account.as_deref() != Some(&request.account_id) {
            return Err(DatabaseError::invalid(
                "Linked task must belong to the account",
            ));
        }
    }
    let t = AgentTask {
        id: Uuid::new_v4().to_string(),
        account_id: request.account_id.clone(),
        objective: request.objective.trim().into(),
        source_thread_id: request.source_thread_id.clone(),
        linked_task_id: request.linked_task_id.clone(),
        workflow: request.workflow.clone(),
        revision: 1,
        created_at: now,
        archived: false,
        unread: false,
    };
    c.execute("INSERT INTO agent_tasks(id,account_id,source_thread_id,linked_task_id,record_json) VALUES (?1,?2,?3,?4,?5)",params![t.id,t.account_id,t.source_thread_id,t.linked_task_id,encode(&t)?])?;
    let r = AgentRun {
        id: Uuid::new_v4().to_string(),
        task_id: t.id.clone(),
        state: RunState::Queued,
        reason: None,
        generation: 0,
        checkpoint: 0,
        owner: None,
        lease_until: None,
        started_at: None,
        not_before: now,
        dispatches: 0,
        attempts: 0,
        active_seconds: 0,
        question_id: None,
    };
    c.execute("INSERT INTO agent_runs(id,task_id,state,generation,not_before,record_json) VALUES (?1,?2,'queued',0,?3,?4)",params![r.id,t.id,now,encode(&r)?])?;
    turn(c, &t.id, "user", &t.objective, None, None, now)?;
    detail(c, &t.id)
}

impl Database {
    pub(crate) fn agent_workflow(&self, id: &str) -> DbResult<String> {
        self.with_connection(|c| Ok(task(c, id)?.workflow))
    }
    pub(crate) fn agent_lease_current(&self, l: &Lease, now: i64) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(run(c, &l.run.task_id).is_ok_and(|r| {
                r.state == RunState::Running
                    && r.generation == l.run.generation
                    && r.owner == l.run.owner
                    && r.lease_until.is_some_and(|end| end > now)
            }))
        })
    }
    pub(crate) fn agent_runner_enabled(&self) -> DbResult<bool> {
        self.with_connection(enabled)
    }
    pub(crate) fn set_agent_runner_enabled(&self, value: bool, now: i64) -> DbResult<()> {
        self.with_transaction(|c| {
            c.execute("INSERT INTO compose_settings(key,value) VALUES ('agent_runner_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if value {"true"} else {"false"}])?;
            if !value {
                let runs: Vec<AgentRun>=records(c,"SELECT record_json FROM agent_runs WHERE state IN ('queued','running') AND ?1 IS NOT NULL", "all")?;
                for mut r in runs { charge_time(&mut r,now); invalidate(&mut r,RunState::Paused,Some("disabled"));save_run(c,&r)?; }
            }
            Ok(())
        })
    }
    pub(crate) fn create_agent_task(
        &self,
        request: &CreateAgentTask,
        now: i64,
    ) -> DbResult<AgentDetail> {
        bounded(&request.objective, MAX_OBJECTIVE)?;
        if request.workflow != WORKFLOW {
            return Err(DatabaseError::invalid(
                "Only the fixture workflow is available",
            ));
        }
        self.with_transaction(|c| create_task(c, request, now))
    }
    pub(crate) fn get_agent_task(&self, id: &str) -> DbResult<AgentDetail> {
        self.with_connection(|c| detail(c, id))
    }
    pub(crate) fn list_agent_tasks(
        &self,
        account: Option<&str>,
        include_archived: bool,
    ) -> DbResult<Vec<AgentDetail>> {
        self.with_connection(|c| {
            let mut s=c.prepare("SELECT id FROM agent_tasks WHERE (?1 IS NULL OR account_id=?1) AND (?2 OR json_extract(record_json,'$.archived')=0) ORDER BY rowid DESC LIMIT ?3")?;
            let ids=s.query_map(params![account,include_archived,MAX_LIST],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let mut out=Vec::new();
            for id in ids { let d=detail(c,&id)?;if include_archived || !d.task.archived { out.push(d); } if out.len()>=MAX_LIST as usize { break; } }
            Ok(out)
        })
    }
    /// Claims one step, recovering expired leases first. Every dispatch has a
    /// durable intent and consumes allowance, even if the process crashes.
    pub(crate) fn claim_agent_run(&self, owner: &str, now: i64) -> DbResult<Option<Lease>> {
        self.with_transaction(|c| {
            if !enabled(c)? { return Ok(None); }
            let expired:Vec<AgentRun>=records(c,"SELECT record_json FROM agent_runs WHERE state='running' AND lease_until<=CAST(?1 AS INTEGER)",&now.to_string())?;
            for mut r in expired {
                charge_time(&mut r,now);
                c.execute("UPDATE agent_steps SET status='ambiguous' WHERE run_id=?1 AND generation=?2 AND status='started'",params![r.id,r.generation])?;
                invalidate(&mut r,RunState::Queued,Some("interrupted")); save_run(c,&r)?;
            }
            let active:i64=c.query_row("SELECT COUNT(*) FROM agent_runs WHERE state='running'",[],|r|r.get(0))?;
            if active>=CONCURRENT_JOBS { return Ok(None); }
            loop {
                let id:Option<String>=c.query_row("SELECT task_id FROM agent_runs WHERE state='queued' AND not_before<=?1 ORDER BY not_before,rowid LIMIT 1",[now],|r|r.get(0)).optional()?;
                let Some(id)=id else { return Ok(None); };
                let mut r=run(c,&id)?;
                if r.dispatches>=dispatch_limit(c,&r.task_id)? || r.active_seconds>=MAX_ACTIVE_SECONDS {
                    invalidate(&mut r,RunState::Paused,Some("budget_exhausted"));save_run(c,&r)?;continue;
                }
                if r.attempts>MAX_RETRIES { invalidate(&mut r,RunState::Failed,Some("retry_exhausted"));save_run(c,&r)?;continue; }
                r.state=RunState::Running;r.reason=None;r.generation+=1;r.owner=Some(owner.into());r.lease_until=Some(now+LEASE_SECONDS.min(MAX_ACTIVE_SECONDS-r.active_seconds));r.started_at=Some(now);r.dispatches+=1;r.attempts+=1;
                save_run(c,&r)?;
                c.execute("INSERT INTO agent_steps(id,run_id,checkpoint,generation,status) VALUES (?1,?2,?3,?4,'started')",params![format!("{}:{}",r.id,r.generation),r.id,r.checkpoint,r.generation])?;
                return Ok(Some(Lease { run:r }));
            }
        })
    }
    pub(crate) fn finish_agent_step(
        &self,
        lease: &Lease,
        result: StepResult,
        now: i64,
    ) -> DbResult<bool> {
        self.with_transaction(|c| {
            let mut r=match run(c,&lease.run.task_id) { Ok(r)=>r, Err(DatabaseError::NotFound(_))=>return Ok(false), Err(e)=>return Err(e) };
            if r.id!=lease.run.id || r.checkpoint!=lease.run.checkpoint || r.state!=RunState::Running || r.generation!=lease.run.generation || r.owner!=lease.run.owner || r.lease_until.is_none_or(|end|end<=now) || !enabled(c)? { return Ok(false); }
            charge_time(&mut r,now);
            if task(c,&r.task_id)?.workflow==RESEARCH_WORKFLOW {return research::finish(c,&mut r,result,now);}
            if r.active_seconds>=MAX_ACTIVE_SECONDS { invalidate(&mut r,RunState::Paused,Some("budget_exhausted"));save_run(c,&r)?;return Ok(false); }
            let expected=match r.checkpoint { 0=>matches!(result,StepResult::Prepared),1=>matches!(result,StepResult::EvidenceCollected),2=>matches!(result,StepResult::Publish) && r.question_id.is_none(),_=>false };
            if !expected && !matches!(result,StepResult::TransientFailure) { return Err(DatabaseError::invalid("Unexpected agent checkpoint result")); }
            c.execute("UPDATE agent_steps SET status='committed',result_json=?3 WHERE run_id=?1 AND generation=?2",params![r.id,r.generation,encode(&result)?])?;
            if matches!(result,StepResult::TransientFailure) {
                let exhausted=r.attempts>MAX_RETRIES;
                r.not_before=now+RETRY_BASE_SECONDS*2_i64.pow((r.attempts-1) as u32);
                invalidate(&mut r,if exhausted {RunState::Failed} else {RunState::Queued},Some(if exhausted {"retry_exhausted"} else {"transient_failure"}));
            } else {
                let mut t=task(c,&r.task_id)?;
                match result {
                    StepResult::Prepared=> {
                        let q=Uuid::new_v4().to_string();
                        turn(c,&t.id,"assistant","Fixture question: what outcome matters most?",Some(q.clone()),None,now)?;
                        r.question_id=Some(q);r.state=RunState::Queued;t.unread=true;
                    },
                    StepResult::EvidenceCollected=> {
                        turn(c,&t.id,"assistant","Fixture evidence step completed. No external research was performed.",None,None,now)?;
                        r.state=if r.question_id.is_some() {RunState::WaitingForInput} else {RunState::Queued};
                    },
                    StepResult::Publish=> {
                        let version:i64=c.query_row("SELECT COALESCE(MAX(version),0)+1 FROM agent_artifacts WHERE task_id=?1",[&t.id],|row|row.get(0))?;
                        let artifact=AgentArtifact { id:Uuid::new_v4().to_string(),version,kind:"fixture_report".into(),body:"Fixture report. No external research was performed. Use this result only to verify delegated-work execution.".into(),not_researched:true,created_at:now };
                        let mut artifact=artifact;
                        if version>1 {
                            let turns:Vec<AgentTurn>=records(c,"SELECT record_json FROM agent_turns WHERE task_id=?1 ORDER BY id DESC LIMIT 1",&t.id)?;
                            if let Some(request)=turns.first() { artifact.body.push_str("\n\nRevision request: ");artifact.body.push_str(&request.text); }
                        }
                        c.execute("INSERT INTO agent_artifacts(id,task_id,version,record_json) VALUES (?1,?2,?3,?4)",params![artifact.id,t.id,version,encode(&artifact)?])?;
                        r.state=RunState::Completed;t.unread=true;
                    },
                    _=>unreachable!(),
                }
                save_task(c,&t)?;
                r.checkpoint+=1;r.attempts=0;r.not_before=now;r.reason=None;r.owner=None;r.lease_until=None;r.started_at=None;
            }
            save_run(c,&r)?; Ok(true)
        })
    }
    pub(crate) fn answer_agent_question(
        &self,
        id: &str,
        question_id: &str,
        answer: &str,
        now: i64,
    ) -> DbResult<AgentDetail> {
        bounded(answer, MAX_ANSWER)?;
        self.with_transaction(|c| {
            let mut r = run(c, id)?;
            let previous: Option<String> = c
                .query_row(
                    "SELECT record_json FROM agent_turns WHERE task_id=?1 AND answer_to=?2",
                    params![id, question_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(previous) = previous {
                let t: AgentTurn = decode(previous)?;
                if t.text == answer.trim() {
                    return detail(c, id);
                }
                return Err(DatabaseError::invalid("Question already answered"));
            }
            if r.question_id.as_deref() != Some(question_id)
                || matches!(
                    r.state,
                    RunState::Completed | RunState::Cancelled | RunState::Failed
                )
            {
                return Err(DatabaseError::invalid("Question is not pending"));
            }
            turn(
                c,
                id,
                "user",
                answer.trim(),
                None,
                Some(question_id.into()),
                now,
            )?;
            research::answer(c, id, answer)?;
            r.question_id = None;
            // A user may answer during the independent evidence step. Leave its
            // lease intact; its commit checks whether more input is still needed.
            if r.state == RunState::WaitingForInput {
                invalidate(
                    &mut r,
                    if enabled(c)? {
                        RunState::Queued
                    } else {
                        RunState::Paused
                    },
                    if enabled(c)? { None } else { Some("disabled") },
                );
                r.not_before = now;
            }
            save_run(c, &r)?;
            detail(c, id)
        })
    }
    pub(crate) fn control_agent_task(
        &self,
        id: &str,
        action: &str,
        now: i64,
    ) -> DbResult<AgentDetail> {
        self.with_transaction(|c| {
            let mut t = task(c, id)?;
            let mut r = run(c, id)?;
            match action {
                "archive" => t.archived = true,
                "unarchive" => t.archived = false,
                "mark_read" => t.unread = false,
                "pause"
                    if matches!(
                        r.state,
                        RunState::Queued | RunState::Running | RunState::WaitingForInput
                    ) =>
                {
                    charge_time(&mut r, now);
                    invalidate(&mut r, RunState::Paused, Some("user"));
                }
                "cancel" if !matches!(r.state, RunState::Completed | RunState::Cancelled) => {
                    charge_time(&mut r, now);
                    invalidate(&mut r, RunState::Cancelled, Some("user"));
                }
                "resume" | "retry"
                    if (action == "resume" && r.state == RunState::Paused)
                        || (action == "retry" && r.state == RunState::Failed) =>
                {
                    if !enabled(c)? {
                        return Err(DatabaseError::invalid("Delegated work is disabled"));
                    }
                    if r.dispatches >= dispatch_limit(c, id)?
                        || r.active_seconds >= MAX_ACTIVE_SECONDS
                    {
                        return Err(DatabaseError::invalid("Agent allowance exhausted"));
                    }
                    let next = if r.question_id.is_some()
                        && (r.checkpoint == 2 || t.workflow == RESEARCH_WORKFLOW)
                    {
                        RunState::WaitingForInput
                    } else {
                        RunState::Queued
                    };
                    invalidate(&mut r, next, None);
                    r.not_before = now;
                    if action == "retry" {
                        r.attempts = 0;
                    }
                }
                _ => return Err(DatabaseError::invalid("Invalid delegated-task transition")),
            }
            save_task(c, &t)?;
            save_run(c, &r)?;
            detail(c, id)
        })
    }
    pub(crate) fn mark_agent_task_read(
        &self,
        id: &str,
        generation: i64,
        turn_count: usize,
        artifact_count: usize,
    ) -> DbResult<AgentDetail> {
        self.with_transaction(|c| {
            let mut d = detail(c, id)?;
            if d.run.generation == generation
                && d.turns.len() == turn_count
                && d.artifacts.len() == artifact_count
            {
                d.task.unread = false;
                save_task(c, &d.task)?;
            }
            detail(c, id)
        })
    }
    pub(crate) fn interact_agent_task(
        &self,
        id: &str,
        message: &str,
        mode: &str,
        now: i64,
    ) -> DbResult<AgentDetail> {
        bounded(message, MAX_ANSWER)?;
        if !matches!(mode, "discuss" | "revise") {
            return Err(DatabaseError::invalid("Unknown interaction"));
        }
        self.with_transaction(|c| {
            let count:i64=c.query_row("SELECT COUNT(*) FROM agent_turns WHERE task_id=?1",[id],|r|r.get(0))?;
            if count+2>MAX_TURNS {return Err(DatabaseError::invalid("Conversation limit reached"));}
            let mut r=run(c,id)?;
            if r.state!=RunState::Completed {return Err(DatabaseError::invalid("Wait for the current work to finish"));}
            let mut t=task(c,id)?;
            if t.workflow==RESEARCH_WORKFLOW {return research::interact(c,&mut t,&mut r,message,mode,now);}
            if mode=="revise" {
                if !enabled(c)? {return Err(DatabaseError::invalid("Delegated work is disabled"));}
                if r.dispatches>=MAX_DISPATCHES || r.active_seconds>=MAX_ACTIVE_SECONDS {return Err(DatabaseError::invalid("Agent allowance exhausted"));}
                t.revision+=1;r.checkpoint=2;r.attempts=0;r.question_id=None;r.not_before=now;
                invalidate(&mut r,RunState::Queued,None);
            }
            turn(c,id,"user",message.trim(),None,None,now)?;
            if mode=="discuss" {turn(c,id,"assistant","This is a demonstration response based on the saved fixture report. No external research was performed. Request a report revision to save your new criteria in a separate version.",None,None,now)?;t.unread=true;}
            save_task(c,&t)?;save_run(c,&r)?;detail(c,id)
        })
    }
    pub(crate) fn link_agent_task(&self, id: &str, linked: Option<&str>) -> DbResult<AgentDetail> {
        self.with_transaction(|c| {
            let mut t = task(c, id)?;
            if let Some(link) = linked {
                let account: Option<String> = c
                    .query_row("SELECT account_id FROM tasks WHERE id=?1", [link], |r| {
                        r.get(0)
                    })
                    .optional()?;
                if account.as_deref() != Some(t.account_id.as_str()) {
                    return Err(DatabaseError::invalid(
                        "Linked task must belong to the account",
                    ));
                }
            }
            t.linked_task_id = linked.map(str::to_owned);
            c.execute(
                "UPDATE agent_tasks SET linked_task_id=?2,record_json=?3 WHERE id=?1",
                params![id, linked, encode(&t)?],
            )?;
            detail(c, id)
        })
    }
    pub(crate) fn delete_agent_task(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|c| {
            c.execute("DELETE FROM agent_tasks WHERE id=?1", [id])?;
            Ok(())
        })
    }
    pub(crate) fn release_agent_owner(&self, owner: &str, now: i64) -> DbResult<()> {
        self.with_transaction(|c| {
            let runs: Vec<AgentRun> = records(
                c,
                "SELECT record_json FROM agent_runs WHERE state='running' AND ?1 IS NOT NULL",
                owner,
            )?;
            for mut r in runs {
                if r.owner.as_deref() == Some(owner) {
                    charge_time(&mut r, now);
                    c.execute("UPDATE agent_steps SET status='ambiguous' WHERE run_id=?1 AND generation=?2 AND status='started'", params![r.id,r.generation])?;
                    invalidate(&mut r, RunState::Queued, Some("interrupted"));
                    r.not_before = now;
                    save_run(c, &r)?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
