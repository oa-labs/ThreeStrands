import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { Account, AgentDetail, AgentTaskControl, ThreadTask } from "./domain";
import type { AgentController, AgentStart } from "./useAgentTasks";
import { mailClient } from "./data/client";
import { useMediaQuery } from "./useMediaQuery";
import { errorMessage } from "./errors";
import { isEditableTarget } from "./commands";
import { useWorkspaceCommands } from "./workspaceCommands";

export const agentStatus = (d: AgentDetail) => ({ queued: "Queued", running: "Working", waiting_for_input: "Needs input", paused: "Paused", completed: "Completed", failed: "Failed", cancelled: "Cancelled" })[d.run.state];
const reasons: Record<string, string> = { disabled: "Delegated work is disabled. Enable the demonstration, then resume this task.", user: "Paused by you.", budget_exhausted: "This task has used its execution allowance.", retry_exhausted: "Automatic retries have been exhausted. You can retry within the remaining allowance.", unsupported: "The selected provider is not supported for this workflow.", interrupted: "Execution was interrupted; saved work is retained." };

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

function NewAgentTask({ controller, accounts }: { controller: AgentController; accounts: Account[] }) {
  const seed = controller.start!;
  const id = useId();
  const [mode, setMode] = useState("demo");
  const { tasks, error } = usePersonalTasks(seed.accountId);
  const change = (patch: Partial<AgentStart>) => controller.setStart({ ...seed, ...patch });
  return <form className="agent-new" aria-label="New delegated task" onSubmit={e => { e.preventDefault(); if (mode === "demo") void controller.create({ ...seed, workflow: "fixture_research_v1" }); }}>
    <h2>New delegated task</h2>
    <p id={`${id}-scope`}>The local demonstration asks a question and prepares a sample report. It sends no email or web requests and has no provider charges.</p>
    {!controller.enabled ? <button type="button" className="btn" disabled={controller.busy} onClick={() => void controller.enable(true)}>Enable local demonstration</button> : null}
    <label htmlFor={`${id}-account`}>Mail account</label>
    <select id={`${id}-account`} value={seed.accountId} onChange={e => change({ accountId: e.target.value, sourceThreadId: null, linkedTaskId: null })}>
      {accounts.filter(a => a.onThisDevice !== false).map(a => <option key={a.email} value={a.email}>{a.nickname || a.email}</option>)}
    </select>
    <label htmlFor={`${id}-mode`}>Work</label>
    <select id={`${id}-mode`} value={mode} onChange={e => setMode(e.target.value)}><option value="demo">Invitation research demonstration</option><option value="web">Web research</option></select>
    {mode === "web" ? <p role="status">Web research is not available yet. Choose the local demonstration to explore this workspace.</p> : null}
    <label htmlFor={`${id}-objective`}>What would you like to find out?</label>
    <textarea id={`${id}-objective`} required value={seed.objective} onChange={e => change({ objective: e.target.value })} aria-describedby={`${id}-scope`} />
    <label htmlFor={`${id}-task`}>Link a personal task (optional)</label>
    <select id={`${id}-task`} value={seed.linkedTaskId ?? ""} onChange={e => change({ linkedTaskId: e.target.value || null })}><option value="">No personal task</option>{tasks.map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select>
    {error ? <p role="alert">Could not load personal tasks: {error}</p> : null}
    {seed.sourceThreadId ? <p>Linked to the selected conversation.</p> : null}
    <div className="agent-actions"><button className="btn btn-primary" disabled={!controller.enabled || controller.busy || mode !== "demo" || !seed.accountId || !seed.objective.trim()}>Start demonstration</button><button type="button" className="btn" onClick={() => controller.setStart(null)}>Cancel</button></div>
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
      <p className="agent-disclosure">Local demonstration · No external research is performed. Finishing this work does not complete your personal task.</p>
      <div className="agent-actions">
        {d.task.sourceThreadId ? <button className="btn-link" onClick={() => { const source = d.task.sourceThreadId!; void mailClient.getThread(source).then(() => onOpenThread(source)).catch(() => setSourceError("The source conversation is no longer available.")); }}>Open source conversation</button> : <span>No linked conversation is available.</span>}
        {d.task.linkedTaskId ? <button className="btn-link" onClick={() => onOpenTask(d.task.linkedTaskId!)}>Open personal task</button> : null}
        {["queued", "running", "waiting_for_input"].includes(d.run.state) ? <button className="btn" disabled={c.busy} onClick={() => act("pause")}>Pause</button> : null}
        {d.run.state === "paused" ? <button className="btn" disabled={c.busy || !c.enabled} onClick={() => act("resume")}>Resume</button> : null}
        {d.run.state === "failed" ? <button className="btn" disabled={c.busy || !c.enabled} onClick={() => act("retry")}>Retry</button> : null}
        {!["completed", "cancelled"].includes(d.run.state) ? <button className="btn" disabled={c.busy} onClick={() => act("cancel")}>Cancel work</button> : null}
        <button className="btn" disabled={c.busy} onClick={() => act(d.task.archived ? "unarchive" : "archive")}>{d.task.archived ? "Unarchive" : "Archive"}</button>
        <button className="btn" onClick={() => setDeleting(true)}>Delete delegated task</button>
      </div>
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
        {d.artifacts.length ? <label>Report version<select aria-label="Report version" value={artifact?.id ?? ""} onChange={e => patch({ version: e.target.value })}>{d.artifacts.map(a => <option key={a.id} value={a.id}>Version {a.version} · {new Date(a.createdAt * 1000).toLocaleString()}</option>)}</select></label> : null}
        <div className="agent-scroll" ref={report} onScroll={e => { if (!narrow || view.tab === "result") c.scroll.current.set(`${d.task.id}:report`, e.currentTarget.scrollTop); }}>
          {artifact ? <><p className="agent-disclosure">Demonstration report · Not researched</p><div className="agent-report-body">{artifact.body}</div></> : <p>{d.run.state === "completed" ? "This task finished without a report." : "No report yet. Saved results will appear here."}</p>}
          {latest && d.run.state !== "completed" ? <p>The last completed report remains available while work continues.</p> : null}
        </div>
      </section>
    </div>
  </article>;
}

export function AgentsWorkspace({ controller: c, accounts, accountId, onOpenThread, onOpenTask }: { controller: AgentController; accounts: Account[]; accountId: string | null; onOpenThread(id: string): void; onOpenTask(id: string): void }) {
  const [filter, setFilter] = useState("active");
  const [query, setQuery] = useState("");
  const search = useRef<HTMLInputElement>(null);
  const rows = c.items.filter(d => (filter === "archived" ? d.task.archived : !d.task.archived) && (filter !== "needs_input" || d.run.state === "waiting_for_input") && (filter !== "finished" || ["completed", "cancelled", "failed"].includes(d.run.state)) && d.task.objective.toLowerCase().includes(query.toLowerCase()));
  const selected = c.items.find(d => d.task.id === c.selectedId);
  const begin = () => c.begin({ accountId: accountId ?? accounts.find(a => a.onThisDevice !== false)?.email ?? "", objective: "", sourceThreadId: null, linkedTaskId: null });
  useWorkspaceCommands({ "agents.new": begin, "agents.search": () => search.current?.focus() });
  return <section className="agents-workspace" aria-label="Agents" onKeyDown={e => {
    if (isEditableTarget(e.target)) return;
    if (["j", "k", "ArrowDown", "ArrowUp"].includes(e.key) && rows.length) {
      e.preventDefault(); const index = rows.findIndex(d => d.task.id === c.selectedId); c.select(rows[Math.max(0, Math.min(rows.length - 1, index + (["j", "ArrowDown"].includes(e.key) ? 1 : -1)))].task.id);
    }
  }}>
    <header className="agents-toolbar"><h1>Agents <span className="agent-preview">Preview</span></h1><button className="btn" onClick={begin}>New delegated task</button><button className="btn" disabled={c.busy} onClick={() => void c.refresh()}>Refresh agents</button><label><input type="checkbox" checked={c.enabled} disabled={c.busy || !c.loaded} onChange={e => void c.enable(e.target.checked)} /> Local demonstration enabled</label></header>
    {c.error ? <p className="form-error" role="alert">{c.error} <button className="btn-link" onClick={() => void c.refresh()}>Retry loading agents</button></p> : null}
    <div className="agents-layout"><nav className="agent-list" aria-label="Delegated tasks">
      <label>Find delegated work<input ref={search} type="search" value={query} onChange={e => setQuery(e.target.value)} /></label>
      <label>Show<select value={filter} onChange={e => setFilter(e.target.value)}><option value="active">Unarchived</option><option value="needs_input">Needs input</option><option value="finished">Finished</option><option value="archived">Archived</option></select></label>
      {!c.loaded ? <p role="status">Loading delegated work…</p> : !rows.length ? <p>No delegated tasks in this view.</p> : null}
      {rows.map(d => <button className={`agent-list-row ${d.task.id === c.selectedId ? "selected" : ""}`} key={d.task.id} aria-current={d.task.id === c.selectedId ? "true" : undefined} onClick={() => c.select(d.task.id)}><strong>{d.task.objective}</strong><span>{agentStatus(d)}{d.task.unread ? " · Unread activity" : ""}</span><small>{d.task.accountId}</small></button>)}
    </nav>
    {c.start ? <NewAgentTask controller={c} accounts={accounts} /> : selected ? <AgentTaskDetail key={selected.task.id} detail={selected} controller={c} onOpenThread={onOpenThread} onOpenTask={onOpenTask} /> : <div className="agent-empty"><h2>Delegate, clarify, and review</h2><p>Select work or start a local demonstration. Web research is not connected yet.</p></div>}
    </div>
  </section>;
}
