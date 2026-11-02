import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { AgentResearchRequest, AgentDetail, AgentTaskControl, CreateAgentTaskRequest } from "./domain";
import { errorMessage } from "./errors";

export const AGENT_REFRESH_MS = 2000;
export type AgentStart = Omit<CreateAgentTaskRequest, "workflow">;
export type AgentView = { draft: string; tab: "conversation" | "result"; version: string | null };
const initialView: AgentView = { draft: "", tab: "result", version: null };

/** Lives above workspace mounting: unfinished text and selection survive leaving
 * Agents. Native SQLite owns work; polling never starts or resumes a job. */
export function useAgentTasks() {
  const [items, setItems] = useState<AgentDetail[]>([]);
  const [enabled, setEnabled] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [start, setStart] = useState<AgentStart | null>(null);
  const [views, setViews] = useState<Map<string, AgentView>>(new Map());
  const [activity, setActivity] = useState<{ id: string; label: string } | null>(null);
  const scroll = useRef(new Map<string, number>());
  const current = useRef(items); current.current = items;
  const sequence = useRef(0);
  const mutating = useRef(false);
  const alive = useRef(false);
  const initialized = useRef(false);

  const accept = useCallback((next: AgentDetail[], announce: boolean) => {
    if (!alive.current) return;
    if (announce) {
      const old = new Map(current.current.map(d => [d.task.id, d]));
      for (const d of next) {
        const before = old.get(d.task.id);
        if (d.task.unread && before && (d.run.state !== before.run.state || d.artifacts.length !== before.artifacts.length) && ["completed", "waiting_for_input"].includes(d.run.state)) {
          setActivity({ id: d.task.id, label: `${d.run.state === "completed" ? "Result ready" : "Answer needed"}: ${d.task.objective}` });
        }
      }
    }
    current.current = next; setItems(next);
  }, []);
  const refresh = useCallback(async () => {
    if (mutating.current) return;
    const token = ++sequence.current;
    try {
      const [next, setting] = await Promise.all([mailClient.listAgentTasks(undefined, true), mailClient.agentRunnerEnabled()]);
      if (!alive.current || token !== sequence.current) return;
      accept(next, initialized.current); setEnabled(setting); setLoaded(true); setError(""); initialized.current = true;
    } catch (reason) { if (alive.current && token === sequence.current) { setError(errorMessage(reason)); setLoaded(true); } }
  }, [accept]);
  useEffect(() => {
    alive.current = true; void refresh();
    const timer = window.setInterval(() => { if (initialized.current && current.current.some(d => ["queued", "running"].includes(d.run.state))) void refresh(); }, AGENT_REFRESH_MS);
    window.addEventListener("focus", refresh);
    return () => { alive.current = false; window.clearInterval(timer); window.removeEventListener("focus", refresh); };
  }, [refresh]);
  const mutate = useCallback(async (operation: () => Promise<AgentDetail | void>, deleted?: string) => {
    if (mutating.current) return false;
    mutating.current = true; sequence.current++; setBusy(true); setError("");
    try {
      const result = await operation();
      if (!alive.current) return false;
      if (deleted) { accept(current.current.filter(d => d.task.id !== deleted), false); setSelectedId(id => id === deleted ? null : id); setActivity(a => a?.id === deleted ? null : a); }
      else if (result) accept([result, ...current.current.filter(d => d.task.id !== result.task.id)], true);
      return true;
    } catch (reason) { if (alive.current) setError(errorMessage(reason)); return false; }
    finally { mutating.current = false; if (alive.current) setBusy(false); }
  }, [accept]);
  const control = useCallback((id: string, action: AgentTaskControl) => mutate(() => mailClient.controlAgentTask(id, action)), [mutate]);
  const view = (id: string) => views.get(id) ?? initialView;
  const patchView = (id: string, patch: Partial<AgentView>) => setViews(old => { const next = new Map(old); next.set(id, { ...(old.get(id) ?? initialView), ...patch }); return next; });
  return {
    items, enabled, loaded, error, busy, selectedId, start, activity, scroll, refresh,
    view, patchView, dismissActivity: () => setActivity(null),
    select: (id: string) => { setSelectedId(id); setStart(null); setActivity(a => a?.id === id ? null : a); },
    begin: (seed: AgentStart) => { setStart(seed); setSelectedId(null); }, setStart,
    enable: (value: boolean) => mutate(async () => { await mailClient.setAgentRunnerEnabled(value); const next = await mailClient.listAgentTasks(undefined, true); if (alive.current) { setEnabled(value); accept(next, false); setLoaded(true); initialized.current = true; } }),
    create: (request: CreateAgentTaskRequest) => mutate(async () => { const d = await mailClient.createAgentTask(request); if (alive.current) { setSelectedId(d.task.id); setStart(null); } return d; }),
    createResearch: (request: AgentResearchRequest) => mutate(async () => { const d = await mailClient.createAgentResearchTask(request); if (alive.current) { setSelectedId(d.task.id); setStart(null); } return d; }),
    control,
    remove: (id: string) => mutate(() => mailClient.deleteAgentTask(id), id),
    markRead: useCallback((id: string, generation: number, turnCount: number, artifactCount: number) => mutate(() => mailClient.markAgentTaskRead(id, generation, turnCount, artifactCount)), [mutate]),
    answer: (id: string, question: string, text: string) => mutate(() => mailClient.answerAgentQuestion(id, question, text)),
    interact: (id: string, text: string, mode: "discuss" | "revise") => mutate(() => mailClient.interactAgentTask(id, text, mode)),
    link: (id: string, task: string | null) => mutate(() => mailClient.linkAgentTask(id, task)),
  };
}
export type AgentController = ReturnType<typeof useAgentTasks>;
