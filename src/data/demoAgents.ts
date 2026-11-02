import type { Account, AgentDetail, AgentTaskControl, CreateAgentTaskRequest, Thread, ThreadTask } from "../domain";

/** In-memory preview of phase-1 fixture work. Never calls external services.
 * The native scheduler owns durability; the demo advances synchronously. */
export function createDemoAgents(context: () => { accounts: Account[]; threads: Thread[]; tasks: ThreadTask[] }) {
  const items = new Map<string, AgentDetail>();
  let enabled = false;
  const now = () => Math.floor(Date.now() / 1000);
  const id = () => crypto.randomUUID();
  const reconcile = () => {
    const c = context();
    for (const [taskId, d] of items) {
      if (!c.accounts.some((a) => a.email === d.task.accountId && a.onThisDevice !== false)) { items.delete(taskId); continue; }
      if (d.task.sourceThreadId && !c.threads.some((t) => t.id === d.task.sourceThreadId)) d.task.sourceThreadId = null;
      if (d.task.linkedTaskId && !c.tasks.some((t) => t.id === d.task.linkedTaskId)) d.task.linkedTaskId = null;
    }
  };
  const get = (taskId: string) => {
    reconcile();
    const d = items.get(taskId);
    if (!d) throw new Error("Delegated task not found");
    return d;
  };
  const checkText = (text: string, limit: number) => {
    if (!text.trim() || [...text].length > limit) throw new Error("Agent text is empty or too long");
  };
  const advance = (d: AgentDetail) => {
    reconcile();
    if (!items.has(d.task.id) || !enabled || d.run.state !== "queued") return;
    if (d.run.checkpoint === 0) {
      const q = id();
      d.turns.push({ id: id(), role: "assistant", text: "Fixture question: what outcome matters most?", questionId: q, answerTo: null, createdAt: now() });
      d.run.questionId = q;
      d.run.checkpoint = 2;
      d.run.dispatches += 2;
      d.task.unread = true;
      d.turns.push({ id: id(), role: "assistant", text: "Fixture evidence step completed. No external research was performed.", questionId: null, answerTo: null, createdAt: now() });
    }
    if (d.run.questionId) { d.run.state = "waiting_for_input"; return; }
    const version = d.artifacts.length + 1;
    d.artifacts.push({ id: id(), version, kind: "fixture_report", body: "Fixture report. No external research was performed. Use this result only to verify delegated-work execution.", notResearched: true, createdAt: now() });
    d.run.state = "completed"; d.run.checkpoint = 3; d.run.dispatches += 1; d.task.unread = true;
  };
  return {
    async markAgentTaskRead(taskId: string, generation: number, turnCount: number, artifactCount: number) {
      const d = get(taskId);
      if (d.run.generation === generation && d.turns.length === turnCount && d.artifacts.length === artifactCount) d.task.unread = false;
      return structuredClone(d);
    },
    async interactAgentTask(taskId: string, message: string, mode: "discuss" | "revise") {
      checkText(message, 8000);
      const d = get(taskId);
      if (d.run.state !== "completed") throw new Error("Wait for the current work to finish");
      if (d.turns.length + 2 > 100) throw new Error("Conversation limit reached");
      if (!["discuss", "revise"].includes(mode)) throw new Error("Unknown interaction");
      if (mode === "revise" && (!enabled || d.run.dispatches >= 20 || d.run.activeSeconds >= 900)) throw new Error("Delegated work is disabled or its allowance is exhausted");
      d.turns.push({ id: id(), role: "user", text: message.trim(), questionId: null, answerTo: null, createdAt: now() });
      if (mode === "discuss") {
        d.turns.push({ id: id(), role: "assistant", text: "This is a demonstration response based on the saved fixture report. No external research was performed. Request a report revision to save your new criteria in a separate version.", questionId: null, answerTo: null, createdAt: now() });
        d.task.unread = true;
      } else {
        d.task.revision += 1; d.run.generation += 1; d.run.state = "queued"; d.run.checkpoint = 2;
        advance(d);
        d.artifacts.at(-1)!.body += "\n\nRevision request: " + message.trim();
      }
      return structuredClone(d);
    },
    async linkAgentTask(taskId: string, linkedTaskId: string | null) {
      const d = get(taskId);
      if (linkedTaskId && !context().tasks.some(t => t.id === linkedTaskId && t.accountId === d.task.accountId)) throw new Error("Linked task must belong to the account");
      d.task.linkedTaskId = linkedTaskId; return structuredClone(d);
    },
    async agentRunnerEnabled() { return enabled; },
    async setAgentRunnerEnabled(value: boolean) {
      enabled = value;
      if (!value) for (const d of items.values()) if (["queued", "running"].includes(d.run.state)) {
        d.run.state = "paused"; d.run.reason = "disabled"; d.run.generation += 1;
      }
    },
    async createAgentTask(request: CreateAgentTaskRequest): Promise<AgentDetail> {
      if (!enabled) throw new Error("Delegated work is disabled");
      checkText(request.objective, 4000);
      if (request.workflow !== "fixture_research_v1") throw new Error("Only the fixture workflow is available");
      const c = context();
      if (!c.accounts.some((a) => a.email === request.accountId && a.onThisDevice !== false)) throw new Error("Account is not available on this device");
      if (request.sourceThreadId && !c.threads.some((t) => t.id === request.sourceThreadId && t.accountId === request.accountId)) throw new Error("Source conversation must belong to the account");
      if (request.linkedTaskId && !c.tasks.some((t) => t.id === request.linkedTaskId && t.accountId === request.accountId)) throw new Error("Linked task must belong to the account");
      const taskId = id();
      const d: AgentDetail = {
        task: { id: taskId, accountId: request.accountId, objective: request.objective.trim(), workflow: request.workflow, sourceThreadId: request.sourceThreadId ?? null, linkedTaskId: request.linkedTaskId ?? null, revision: 1, createdAt: now(), archived: false, unread: false },
        run: { id: id(), taskId, state: "queued", reason: null, generation: 0, checkpoint: 0, owner: null, leaseUntil: null, startedAt: null, notBefore: now(), dispatches: 0, attempts: 0, activeSeconds: 0, questionId: null },
        turns: [{ id: id(), role: "user", text: request.objective.trim(), questionId: null, answerTo: null, createdAt: now() }], artifacts: [],
      };
      items.set(taskId, d);
      const result = structuredClone(d);
      queueMicrotask(() => { if (items.has(taskId)) advance(d); });
      return result;
    },
    async getAgentTask(taskId: string) { return structuredClone(get(taskId)); },
    async listAgentTasks(accountId?: string, includeArchived = false) {
      reconcile();
      return structuredClone([...items.values()].reverse().filter((d) => (!accountId || d.task.accountId === accountId) && (includeArchived || !d.task.archived)).slice(0, 100));
    },
    async answerAgentQuestion(taskId: string, questionId: string, answer: string) {
      checkText(answer, 8000);
      const d = get(taskId);
      const previous = d.turns.find((t) => t.answerTo === questionId);
      if (previous) { if (previous.text !== answer.trim()) throw new Error("Question already answered"); return structuredClone(d); }
      if (d.run.questionId !== questionId || ["completed", "cancelled", "failed"].includes(d.run.state)) throw new Error("Question is not pending");
      d.turns.push({ id: id(), role: "user", text: answer.trim(), questionId: null, answerTo: questionId, createdAt: now() });
      d.run.questionId = null;
      if (d.run.state === "waiting_for_input") { d.run.state = enabled ? "queued" : "paused"; d.run.reason = enabled ? null : "disabled"; }
      advance(d); return structuredClone(d);
    },
    async controlAgentTask(taskId: string, action: AgentTaskControl) {
      const d = get(taskId);
      if (action === "archive") d.task.archived = true;
      else if (action === "unarchive") d.task.archived = false;
      else if (action === "mark_read") d.task.unread = false;
      else if (action === "pause" && ["queued", "running", "waiting_for_input"].includes(d.run.state)) { d.run.state = "paused"; d.run.reason = "user"; d.run.generation += 1; }
      else if (action === "cancel" && !["completed", "cancelled"].includes(d.run.state)) { d.run.state = "cancelled"; d.run.reason = "user"; d.run.generation += 1; }
      else if ((action === "resume" && d.run.state === "paused") || (action === "retry" && d.run.state === "failed")) {
        if (!enabled) throw new Error("Delegated work is disabled");
        if (d.run.dispatches >= 20 || d.run.activeSeconds >= 900) throw new Error("Agent allowance exhausted");
        d.run.state = "queued"; d.run.reason = null; d.run.generation += 1; advance(d);
      } else throw new Error("Invalid delegated-task transition");
      return structuredClone(d);
    },
    async deleteAgentTask(taskId: string) { items.delete(taskId); },
  };
}
