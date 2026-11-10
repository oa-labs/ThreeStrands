import { formatDateTime, formatFullDate } from "./dateFormatting";
import frozenPolicy from "../docs/agent-integrations/contract-v1.json";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { Account, AgentDetail, AgentTaskControl, AgentResearchSettings as ResearchSettings, ThreadTask } from "./domain";
import { pausedByPauseAll, type AgentController, type AgentStart } from "./useAgentTasks";
import { PauseCircle } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { mailClient } from "./data/client";
import { useMediaQuery } from "./useMediaQuery";
import { errorMessage } from "./errors";
import { isEditableTarget } from "./commands";
import { useWorkspaceCommands } from "./workspaceCommands";

export const agentStatus = (d: AgentDetail) => ({ queued: "Queued", running: "Working", waiting_for_input: "Needs input", paused: "Paused", completed: "Completed", failed: "Failed", cancelled: "Cancelled" })[d.run.state];
const reasons: Record<string, string> = { configuration_changed: "Research settings changed. Review setup before resuming.", credentials_or_access: "Check your research keys and provider access in Settings → Web Research.", missing_credentials: "Save your research keys in Settings → Web Research before resuming.", provider_credits_required: "The provider requires credits or a billing change. No purchase was made.", retry_review: "The provider requested a delay that needs your review before resuming.", budget_or_configuration: "The task allowance or research configuration prevents more requests. Check usage and Settings → Web Research.", evidence_expired: "Raw evidence expired after seven days. Saved reports remain available; start a new task for fresh research.", context_limit: "The conversation reached the model context allowance. Saved reports remain available.", conversation_limit: "This conversation reached its saved-message limit.", no_report_published: "The model stopped without publishing a sourced report. No successful report was created.", response_invalid: "A provider returned an invalid response. Saved results are retained.", model_contract_invalid: "The model returned an unsupported or incomplete response. No tools were executed from it.", citation_not_retrieved: "A report cited evidence that was not retrieved. It was not published.", unsafe_dns: "A page resolved to a private or otherwise restricted network address. It was not fetched.", unsafe_url: "A page URL was private, signed, interactive or otherwise unsupported. It was not fetched.", provider_rejected: "The provider rejected this request. Review setup before retrying.", unsupported_page_type: "This page is not supported. Only HTML and plain text are read.", disabled: "Paused by Pause All. Choose Resume All, or resume this task on its own.", user: "Paused by you.", budget_exhausted: "This task has used its execution allowance.", retry_exhausted: "Automatic retries have been exhausted. You can retry within the remaining allowance.", unsupported: "The selected provider is not supported for this workflow.", interrupted: "Execution was interrupted; saved work is retained." };

function usePersonalTasks(account: string | undefined) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true; setTasks([]); setError("");
    if (account) void mailClient.listTasks(account).then(next => { if (active) setTasks(next); }).catch(e => { if (active) setError(errorMessage(e)); });
    return () => { active = false; };
  }, [account]);
  return { tasks, error };
}

function NewAgentTask({ controller, accounts, research, onSetup }: { controller: AgentController; accounts: Account[]; research: ResearchSettings | null; onSetup(): void }) {
  const seed = controller.start!;
  const id = useId();
  const [mode, setMode] = useState("demo");
  const [subjects, setSubjects] = useState("");
  const [consent, setConsent] = useState(false);
  const policy=research?.policy ?? frozenPolicy;
  const ready = Boolean(research?.config.enabled && research?.config.searchEnabled && research?.config.verified && research?.modelKeyConfigured && research?.searchKeyConfigured);
  const { tasks, error } = usePersonalTasks(seed.accountId);
  const change = (patch: Partial<AgentStart>) => controller.setStart({ ...seed, ...patch });
  return <form className="agent-new" aria-label="New Delegated Task" onSubmit={e => { e.preventDefault(); if (mode === "demo") void controller.create({ ...seed, workflow: "fixture_research_v1" }); else if (ready && consent) void controller.createResearch({task:{...seed,workflow:"web_research_v1"},subjects:subjects.split("\n").map(s=>s.trim()).filter(Boolean),consent}); }}>
    <h2>New Delegated Task</h2>
    <p className="agent-hint" id={`${id}-scope`}>{mode === "demo" ? "The local demonstration asks a question and prepares a sample report. It sends no email or web requests and has no provider charges." : "Your objective, public subjects, follow-up messages, and retrieved evidence go to Anthropic. Exa receives only the public subjects you review below, with fixed research terms. Page hosts receive your IP. The source email is linked for navigation; its body and addresses are not sent."}</p>
    {!controller.enabled ? <button type="button" className="btn" disabled={controller.busy} onClick={() => void controller.enable(true)}>{mode === "demo" ? "Enable local demonstration" : "Enable delegated work"}</button> : null}
    <div className="agent-field">
      <label className="agent-field-label" htmlFor={`${id}-account`}>Mail account</label>
      <select id={`${id}-account`} value={seed.accountId} onChange={e => change({ accountId: e.target.value, sourceThreadId: null, linkedTaskId: null })}>
        {accounts.filter(a => a.onThisDevice !== false).map(a => <option key={a.email} value={a.email}>{a.nickname || a.email}</option>)}
      </select>
    </div>
    <div className="agent-field">
      <label className="agent-field-label" htmlFor={`${id}-mode`}>Work</label>
      <select id={`${id}-mode`} value={mode} onChange={e => setMode(e.target.value)}><option value="demo">Invitation research demonstration</option><option value="web">Web research</option></select>
    </div>
    {mode === "web" ? <>
      {!ready ? <p role="status">Web research is not configured. Save separate research keys and check connections.</p> : null}
      <button type="button" className="btn" onClick={onSetup}>Research Settings</button>
      <div className="agent-field">
        <label className="agent-field-label" htmlFor={`${id}-subjects`}>Public search subjects (one per line)</label>
        <textarea id={`${id}-subjects`} value={subjects} onChange={e=>setSubjects(e.target.value)} aria-describedby={`${id}-public-help`} />
      </div>
      <p className="agent-hint" id={`${id}-public-help`}>Review up to five public show, event or organization names. Do not include private customer names, email bodies, credentials or a home address. Only these subjects plus fixed terms can be searched.</p>
      <p className="agent-hint">This task shares cumulative limits of {policy.limits.modelCallsPerTask} model calls, {policy.limits.searchCallsPerTask} searches, {policy.limits.pageCallsPerTask} pages and {policy.limits.activeSecondsPerTask/60} active minutes with retries and revisions. Estimates ({policy.verifiedOn}): ${policy.model.inputUsdPerMillion}/${policy.model.outputUsdPerMillion} per million input/output tokens and ${policy.search.usdPerRequest} per search; these are estimates, not a hard dollar cap. Routing and mailbox search are unavailable.</p>
      <label className="agent-toggle"><input type="checkbox" checked={consent} onChange={e=>setConsent(e.target.checked)} />I reviewed these public subjects and allow this task to use Anthropic, Exa and public pages under these limits.</label>
    </> : null}
    <div className="agent-field">
      <label className="agent-field-label" htmlFor={`${id}-objective`}>What would you like to find out?</label>
      <textarea id={`${id}-objective`} required value={seed.objective} onChange={e => change({ objective: e.target.value })} aria-describedby={`${id}-scope`} />
    </div>
    <div className="agent-field">
      <label className="agent-field-label" htmlFor={`${id}-task`}>Link a personal task (optional)</label>
      <select id={`${id}-task`} value={seed.linkedTaskId ?? ""} onChange={e => change({ linkedTaskId: e.target.value || null })}><option value="">No personal task</option>{tasks.map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select>
    </div>
    {error ? <p role="alert">Could not load personal tasks: {error}</p> : null}
    {seed.sourceThreadId ? <p className="agent-hint">Linked to the selected conversation.</p> : null}
    <div className="agent-actions"><button className="btn btn-primary" disabled={!controller.enabled || controller.busy || (mode === "web" && (!ready || !consent || !subjects.trim())) || !seed.accountId || !seed.objective.trim()}>{mode === "demo" ? "Start demonstration" : "Start research"}</button><button type="button" className="btn" onClick={() => controller.setStart(null)}>Cancel</button></div>
  </form>;
}

export function AgentTaskDetail({ detail: d, controller: c, onOpenThread, onOpenTask }: { detail: AgentDetail; controller: AgentController; onOpenThread(id: string): void; onOpenTask(id: string): void }) {
  const narrow = useMediaQuery("(max-width: 1000px)");
  const view = c.view(d.task.id);
  const [deleting, setDeleting] = useState(false);
  const [sourceError, setSourceError] = useState("");
  const { tasks, error: tasksError } = usePersonalTasks(d.task.accountId);
  const conversation = useRef<HTMLDivElement>(null);
  const report = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const readAttempt = useRef("");
  const currentDraft = useRef(view.draft); currentDraft.current = view.draft;
  const { markRead } = c;
  const latest = d.artifacts.at(-1);
  const artifact = d.artifacts.find(a => a.id === view.version) ?? latest;
  const question = d.run.state === "waiting_for_input" && d.run.questionId ? d.turns.find(t => t.questionId === d.run.questionId) : undefined;
  const canTalk = Boolean(question) || d.run.state === "completed";
  const act = (action: AgentTaskControl) => void c.control(d.task.id, action);
  const patch = (p: Parameters<AgentController["patchView"]>[1]) => c.patchView(d.task.id, p);
  useLayoutEffect(() => {
    for (const [name, ref] of [["conversation", conversation], ["report", report]] as const) if (ref.current) ref.current.scrollTop = c.scroll.current.get(`${d.task.id}:${name}`) ?? 0;
  }, [d.task.id, c.scroll, view.tab, narrow]);
  useEffect(() => { heading.current?.focus(); }, [d.task.id]);
  useEffect(() => {
    const token = `${d.task.id}:${d.run.generation}:${d.turns.length}:${d.artifacts.length}`;
    if (d.task.unread && !c.busy && readAttempt.current !== token) {
      readAttempt.current = token; void markRead(d.task.id, d.run.generation, d.turns.length, d.artifacts.length);
    }
  }, [d.task.id, d.task.unread, d.run.generation, d.turns.length, d.artifacts.length, c.busy, markRead]);
  async function submit(mode: "discuss" | "revise") {
    const sent = view.draft;
    if (!sent.trim()) return;
    const ok = question ? await c.answer(d.task.id, question.questionId!, sent) : await c.interact(d.task.id, sent, mode);
    if (ok && currentDraft.current === sent) patch({ draft: "" });
  }
  return <article className="agent-detail" aria-label="Delegated task details">
    <header><h2 ref={heading} tabIndex={-1}>{d.task.objective}</h2><p role="status">{agentStatus(d)}{d.run.reason ? ` · ${reasons[d.run.reason] ?? "Work could not continue. Your saved results are retained."}` : ""}</p>
      <p className="agent-disclosure">{d.task.workflow === "web_research_v1" ? "Read-only research · Anthropic, Exa and public pages. Finishing this work does not complete your personal task." : "Local demonstration · No external research is performed. Finishing this work does not complete your personal task."}</p>
      <div className="agent-actions">
        {d.task.sourceThreadId ? <button className="btn-link" onClick={() => { const source = d.task.sourceThreadId!; void mailClient.getThread(source).then(() => onOpenThread(source)).catch(() => setSourceError("The source conversation is no longer available.")); }}>Open source conversation</button> : <span>No linked conversation is available.</span>}
        {d.task.linkedTaskId ? <button className="btn-link" onClick={() => onOpenTask(d.task.linkedTaskId!)}>Open personal task</button> : null}
        {["queued", "running", "waiting_for_input"].includes(d.run.state) ? <button className="btn" disabled={c.busy} onClick={() => act("pause")}>Pause</button> : null}
        {d.run.state === "paused" ? <button className="btn" disabled={c.busy || !c.enabled} onClick={() => act("resume")}>Resume</button> : null}
        {d.run.state === "failed" ? <button className="btn" disabled={c.busy || !c.enabled} onClick={() => act("retry")}>Try Again</button> : null}
        {!["completed", "cancelled"].includes(d.run.state) ? <button className="btn" disabled={c.busy} onClick={() => act("cancel")}>Cancel work</button> : null}
        <button className="btn" disabled={c.busy} onClick={() => act(d.task.archived ? "unarchive" : "archive")}>{d.task.archived ? "Unarchive" : "Archive"}</button>
        <button className="btn" onClick={() => setDeleting(true)}>Delete delegated task</button>
      </div>
      {d.research ? <p className="agent-hint">Usage: {d.research.usage.modelCalls}/{frozenPolicy.limits.modelCallsPerTask} model calls · {d.research.usage.searchCalls}/{frozenPolicy.limits.searchCallsPerTask} searches · {d.research.usage.pageCalls}/{frozenPolicy.limits.pageCallsPerTask} pages · {d.research.usage.inputTokens + d.research.usage.outputTokens} reported tokens{d.research.usage.unknownUsage ? ` · ${d.research.usage.unknownUsage} requests with unconfirmed usage; reservations retained` : ""}{d.research.usage.unknownSearchCost ? ` · ${d.research.usage.unknownSearchCost} searches with unconfirmed charges` : ""} · Exa charges: {d.research.usage.searchCostUsd === null ? "not reported" : `$${d.research.usage.searchCostUsd.toFixed(3)} reported`}</p> : null}
      {sourceError ? <p role="alert">{sourceError}</p> : null}
      <label>Personal task<select aria-label="Linked personal task" value={d.task.linkedTaskId ?? ""} disabled={c.busy} onChange={e => void c.link(d.task.id, e.target.value || null)}><option value="">No personal task</option>{tasks.map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select></label>
      {tasksError ? <p role="alert">Could not load personal tasks: {tasksError}</p> : null}
      {deleting ? <div role="group" aria-label="Confirm deletion"><p>Delete this task, conversation, and every report version from this device?</p><button className="btn" disabled={c.busy} onClick={() => void c.remove(d.task.id)}>Delete permanently</button><button className="btn" onClick={() => setDeleting(false)}>Keep task</button></div> : null}
    </header>
    {narrow ? <div role="tablist" aria-label="Delegated task content" onKeyDown={e => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
      e.preventDefault(); const tab = e.key === "Home" ? "conversation" : e.key === "End" ? "result" : view.tab === "conversation" ? "result" : "conversation";
      patch({ tab }); e.currentTarget.querySelector<HTMLButtonElement>(`[data-tab="${tab}"]`)?.focus();
    }}>{(["conversation", "result"] as const).map(tab => <button className="btn agent-tab" key={tab} id={`agent-${d.task.id}-${tab}`} data-tab={tab} role="tab" aria-controls={`agent-panel-${d.task.id}-${tab}`} tabIndex={view.tab === tab ? 0 : -1} aria-selected={view.tab === tab} onClick={() => patch({ tab })}>{tab === "result" ? "Result" : "Conversation"}</button>)}</div> : null}
    <div className="agent-detail-columns">
      <section id={`agent-panel-${d.task.id}-conversation`} role={narrow ? "tabpanel" : undefined} aria-labelledby={narrow ? `agent-${d.task.id}-conversation` : undefined} className="agent-conversation" aria-label="Agent conversation" hidden={narrow && view.tab !== "conversation"}>
        <h3>Conversation</h3>
        <div className="agent-scroll" ref={conversation} onScroll={e => { if (!narrow || view.tab === "conversation") c.scroll.current.set(`${d.task.id}:conversation`, e.currentTarget.scrollTop); }}>
          {d.turns.map(t => <div className={`agent-turn ${t.role}`} key={t.id}><strong>{t.role === "user" ? "You" : "Agent"}</strong><p>{t.text}</p>{question?.id === t.id ? <span>Answer needed</span> : null}</div>)}
        </div>
        <form onSubmit={e => { e.preventDefault(); void submit("discuss"); }}>
          <label htmlFor={`agent-reply-${d.task.id}`}>{question ? "Your answer" : "Follow-up message"}</label>
          <textarea id={`agent-reply-${d.task.id}`} value={view.draft} disabled={!canTalk} onChange={e => patch({ draft: e.target.value })} />
          {!canTalk ? <p>Messages will be available when the current work finishes or asks a question.</p> : null}
          <div className="agent-actions"><button className="btn btn-primary" disabled={c.busy || !canTalk || !view.draft.trim()}>{question ? "Answer question" : "Send message"}</button>{d.run.state === "completed" ? <button type="button" className="btn" disabled={c.busy || !c.enabled || !view.draft.trim()} onClick={() => void submit("revise")}>Revise report</button> : null}</div>
        </form>
      </section>
      <section id={`agent-panel-${d.task.id}-result`} role={narrow ? "tabpanel" : undefined} aria-labelledby={narrow ? `agent-${d.task.id}-result` : undefined} className="agent-result" aria-label="Agent result" hidden={narrow && view.tab !== "result"}>
        <h3>Result</h3>
        {d.artifacts.length ? <label>Report version<select aria-label="Report version" value={artifact?.id ?? ""} onChange={e => patch({ version: e.target.value })}>{d.artifacts.map(a => <option key={a.id} value={a.id}>Version {a.version} · {formatDateTime(new Date(a.createdAt * 1000))}</option>)}</select></label> : null}
        <div className="agent-scroll" ref={report} onScroll={e => { if (!narrow || view.tab === "result") c.scroll.current.set(`${d.task.id}:report`, e.currentTarget.scrollTop); }}>
          {artifact ? <><p className="agent-disclosure">{artifact.notResearched ? "Demonstration report · Not researched" : "Research report · Verify important findings against the sources"}</p><div className="agent-report-body">{artifact.body}</div>{d.research ? <ul className="agent-sources" aria-label="Report sources">{d.research.sources.filter(s=>d.research!.citations[artifact.id]?.includes(s.id)).map(s=><li key={s.id}><button className="btn-link" onClick={()=>void mailClient.openAgentResearchSource(d.task.id,s.id).catch(e=>setSourceError(errorMessage(e)))}>{s.title}</button><p>{s.kind === "page" ? "Page retrieved" : "Search excerpt"} · {formatFullDate(new Date(s.retrievedAt*1000))} · {s.url}</p></li>)}</ul> : null}</> : <p>{d.run.state === "completed" ? "This task finished without a report." : "No report yet. Saved results will appear here."}</p>}
          {latest && d.run.state !== "completed" ? <p>The last completed report remains available while work continues.</p> : null}
        </div>
      </section>
    </div>
  </article>;
}

export function AgentsWorkspace({ controller: c, accounts, accountId, onOpenThread, onOpenTask, onOpenResearchSettings, settingsOpen = false }: { controller: AgentController; accounts: Account[]; accountId: string | null; onOpenThread(id: string): void; onOpenTask(id: string): void; onOpenResearchSettings(): void; /** Research setup is re-read whenever Settings closes. */ settingsOpen?: boolean }) {
  const [filter, setFilter] = useState("active");
  const [research, setResearch] = useState<ResearchSettings | null>(null);
  useEffect(()=> {let active=true;void mailClient.agentResearchSettings().then(s=>{if(active) setResearch(s);}).catch(()=>{if(active) setResearch(null);});return ()=>{active=false;};},[settingsOpen]);
  const [query, setQuery] = useState("");
  const search = useRef<HTMLInputElement>(null);
  const rows = c.items.filter(d => (filter === "archived" ? d.task.archived : !d.task.archived) && (filter !== "needs_input" || d.run.state === "waiting_for_input") && (filter !== "finished" || ["completed", "cancelled", "failed"].includes(d.run.state)) && d.task.objective.toLowerCase().includes(query.toLowerCase()));
  const selected = c.items.find(d => d.task.id === c.selectedId);
  // A fresh install has nothing to pause; the new-task form offers to turn work on.
  const paused = c.loaded && !c.enabled && c.items.length > 0;
  const waiting = c.items.filter(pausedByPauseAll).length;
  const begin = () => c.begin({ accountId: accountId ?? accounts.find(a => a.onThisDevice !== false)?.email ?? "", objective: "", sourceThreadId: null, linkedTaskId: null });
  useWorkspaceCommands({ "agents.new": begin, "agents.search": () => search.current?.focus() });
  return <section className="agents-workspace" aria-label="Agents" onKeyDown={e => {
    if (isEditableTarget(e.target)) return;
    if (["j", "k", "ArrowDown", "ArrowUp"].includes(e.key) && rows.length) {
      e.preventDefault(); const index = rows.findIndex(d => d.task.id === c.selectedId); c.select(rows[Math.max(0, Math.min(rows.length - 1, index + (["j", "ArrowDown"].includes(e.key) ? 1 : -1)))].task.id);
    }
  }}>
    <header className="agents-toolbar"><h1>Agents <span className="agent-preview">Preview</span></h1><button className="btn" onClick={begin}>New Delegated Task</button><button className="btn" disabled={c.busy} onClick={() => void c.refresh()}>Refresh Agents</button>{c.enabled ? <button className="btn" disabled={c.busy || !c.loaded} onClick={() => void c.enable(false)} title="Pauses queued and running tasks until you choose Resume All.">Pause All</button> : null}<button className="btn" onClick={onOpenResearchSettings}>Research Settings</button></header>
    {paused ? <div className="agents-paused" role="status"><PauseCircle size={ICON_SIZE.sm} aria-hidden="true" /><span><strong>Delegated work is paused.</strong> {waiting ? `${waiting === 1 ? "1 task is" : `${waiting} tasks are`} waiting to resume.` : "No tasks will run until you resume."}</span><button className="btn" disabled={c.busy} onClick={() => void c.resumeAll()}>Resume All</button></div> : null}
    {c.error ? <p className="form-error" role="alert">{c.error} <button className="btn-link" onClick={() => void c.refresh()}>Try Again</button></p> : null}
    <div className="agents-layout"><nav className="agent-list" aria-label="Delegated tasks">
      <label>Find delegated work<input ref={search} type="search" value={query} onChange={e => setQuery(e.target.value)} /></label>
      <label>Show<select value={filter} onChange={e => setFilter(e.target.value)}><option value="active">Unarchived</option><option value="needs_input">Needs input</option><option value="finished">Finished</option><option value="archived">Archived</option></select></label>
      {!c.loaded ? <p role="status">Loading delegated work…</p> : !rows.length ? <p>No delegated tasks in this view.</p> : null}
      {rows.map(d => <button className={`agent-list-row ${d.task.id === c.selectedId ? "selected" : ""}`} key={d.task.id} aria-current={d.task.id === c.selectedId ? "true" : undefined} onClick={() => c.select(d.task.id)}><strong>{d.task.objective}</strong><span>{agentStatus(d)}{d.task.unread ? " · Unread activity" : ""}</span><small>{d.task.accountId}</small></button>)}
    </nav>
    {c.start ? <NewAgentTask controller={c} accounts={accounts} research={research} onSetup={onOpenResearchSettings} /> : selected ? <AgentTaskDetail key={selected.task.id} detail={selected} controller={c} onOpenThread={onOpenThread} onOpenTask={onOpenTask} /> : <div className="agent-empty"><h2>Delegate, clarify, and review</h2><p>Select work or start a local demonstration. Configure research services to use the web, or explore the local demonstration.</p></div>}
    </div>
  </section>;
}
