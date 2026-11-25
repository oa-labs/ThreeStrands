import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { mailClient } from "./data/client";
import { readAiEndpoint, readAiFastModel, readAiModel, readAiProvider, resolveAiModel } from "./aiSettings";
import type {
  AiRunConfig,
  InboxAgentId,
  InboxAgentProposal,
  InboxAgentsSnapshot,
  ProposalState,
  UpdateInboxAgentConfig,
} from "./domain";
import { errorMessage } from "./errors";

export const INBOX_AGENT_POLL_MS = 1500;

export function readCurrentAiRunConfig(): AiRunConfig | null {
  const provider = readAiProvider();
  if (provider === "none") return null;
  const model = resolveAiModel(provider, readAiModel());
  if (!model) return null;
  const endpoint = provider === "custom" ? readAiEndpoint().trim() : null;
  if (provider === "custom" && !endpoint) return null;
  const fastModel = readAiFastModel().trim() || null;
  return {
    provider,
    model,
    fastModel,
    endpoint,
    reasoning: fastModel ? "off" : "default",
  };
}

const EMPTY_SNAPSHOT: InboxAgentsSnapshot = {
  runnerEnabled: true,
  agents: [],
  activeRuns: [],
  proposals: [],
};

export function useInboxAgents(accountId: string | null, settingsOpen = false) {
  const [snapshot, setSnapshot] = useState<InboxAgentsSnapshot>(EMPTY_SNAPSHOT);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const alive = useRef(true);
  const sequence = useRef(0);
  const mutating = useRef(false);

  const refresh = useCallback(async () => {
    if (mutating.current) return;
    const token = ++sequence.current;
    try {
      const next = await mailClient.getInboxAgentsSnapshot(accountId ?? undefined);
      if (!alive.current || token !== sequence.current) return;
      setSnapshot(next);
      setLoaded(true);
      setError("");
    } catch (reason) {
      if (!alive.current || token !== sequence.current) return;
      setError(errorMessage(reason));
      setLoaded(true);
    }
  }, [accountId]);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const timer = window.setInterval(() => {
      if (snapshot.activeRuns.length > 0) void refresh();
    }, INBOX_AGENT_POLL_MS);
    window.addEventListener("focus", refresh);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [refresh, snapshot.activeRuns.length]);

  // Keep native scheduled runs aware of non-secret AI settings whenever Settings closes.
  useEffect(() => {
    if (settingsOpen) return;
    void mailClient.syncInboxAgentAiConfig(readCurrentAiRunConfig()).catch(() => {});
  }, [settingsOpen]);

  const mutate = useCallback(async (op: () => Promise<InboxAgentsSnapshot | void>, after?: () => void) => {
    if (mutating.current) return false;
    mutating.current = true;
    sequence.current++;
    setBusy(true);
    setError("");
    try {
      const result = await op();
      if (!alive.current) return false;
      if (result) {
        setSnapshot(result);
        setLoaded(true);
      } else {
        after?.();
      }
      return true;
    } catch (reason) {
      if (alive.current) setError(errorMessage(reason));
      return false;
    } finally {
      mutating.current = false;
      if (alive.current) setBusy(false);
    }
  }, []);

  const toggleRunner = useCallback(
    (enabled: boolean) => mutate(() => mailClient.setInboxAgentRunnerEnabled(enabled)),
    [mutate],
  );

  const updateConfig = useCallback(
    (config: UpdateInboxAgentConfig) =>
      mutate(async () => {
        const updated = await mailClient.updateInboxAgentConfig(config);
        setSnapshot((curr) => ({
          ...curr,
          agents: curr.agents.map((a) => (a.agentId === updated.agentId ? updated : a)),
        }));
      }),
    [mutate],
  );

  const runAgent = useCallback(
    (agentId: InboxAgentId) =>
      mutate(() => mailClient.runInboxAgent(agentId, readCurrentAiRunConfig(), accountId ?? undefined)),
    [accountId, mutate],
  );

  const runEnabled = useCallback(
    () => mutate(() => mailClient.runEnabledInboxAgents(readCurrentAiRunConfig(), accountId ?? undefined)),
    [accountId, mutate],
  );

  const resolveProposal = useCallback(
    (id: string, state: Extract<ProposalState, "accepted" | "dismissed">) =>
      mutate(
        () => mailClient.resolveInboxAgentProposal(id, state),
        () => setSnapshot((curr) => ({ ...curr, proposals: curr.proposals.filter((p) => p.id !== id) })),
      ),
    [mutate],
  );

  const proposalsByThread = useMemo(() => {
    const map = new Map<string, InboxAgentProposal[]>();
    for (const p of snapshot.proposals) {
      const list = map.get(p.threadId);
      if (list) list.push(p);
      else map.set(p.threadId, [p]);
    }
    return map;
  }, [snapshot.proposals]);

  const proposalsForThread = useCallback(
    (threadId: string) => proposalsByThread.get(threadId) ?? [],
    [proposalsByThread],
  );

  return {
    snapshot,
    loaded,
    busy,
    error,
    pendingCount: snapshot.proposals.length,
    refresh,
    toggleRunner,
    updateConfig,
    runAgent,
    runEnabled,
    resolveProposal,
    proposalsForThread,
  };
}

export type InboxAgentsController = ReturnType<typeof useInboxAgents>;
