import type {
  Account,
  AiRunConfig,
  InboxAgentConfig,
  InboxAgentId,
  InboxAgentProposal,
  InboxAgentsSnapshot,
  ProposalState,
  Thread,
  ThreadTask,
  UpdateInboxAgentConfig,
} from "../domain";

type DemoState = {
  accounts: Account[];
  threads: Thread[];
  tasks: ThreadTask[];
};

const CATALOG: Record<InboxAgentId, Omit<InboxAgentConfig, "updatedAt">> = {
  stale_reads: {
    agentId: "stale_reads",
    name: "Stale Reads Summarizer",
    description: "Summarizes aging newsletters and long unread emails so you can read the gist, save to Notes, and archive.",
    enabled: false,
    ageDays: 7,
    maxThreadsPerRun: 15,
    includeStarred: false,
    accountId: null,
    scheduleHours: null,
    lastRunAt: null,
    lastRunStatus: null,
    lastRunError: null,
    lastRunScanned: 0,
    lastRunProposed: 0,
  },
  resolved_and_expired: {
    agentId: "resolved_and_expired",
    name: "Expired & Already-Handled Sweeper",
    description: "Spots aging emails already handled in another thread, completed task, or past meeting, or past an expired date.",
    enabled: false,
    ageDays: 7,
    maxThreadsPerRun: 15,
    includeStarred: false,
    accountId: null,
    scheduleHours: null,
    lastRunAt: null,
    lastRunStatus: null,
    lastRunError: null,
    lastRunScanned: 0,
    lastRunProposed: 0,
  },
  aging_replies: {
    agentId: "aging_replies",
    name: "Aging Reply Drafter",
    description: "Detects unreplied emails from people and stages a draft reply (plus a polite check-in for older threads) for your review.",
    enabled: false,
    ageDays: 3,
    maxThreadsPerRun: 10,
    includeStarred: true,
    accountId: null,
    scheduleHours: null,
    lastRunAt: null,
    lastRunStatus: null,
    lastRunError: null,
    lastRunScanned: 0,
    lastRunProposed: 0,
  },
};

export function createDemoInboxAgents(readState: () => DemoState) {
  let runnerEnabled = true;
  const configs: Record<InboxAgentId, InboxAgentConfig> = {
    stale_reads: { ...CATALOG.stale_reads, updatedAt: 0 },
    resolved_and_expired: { ...CATALOG.resolved_and_expired, updatedAt: 0 },
    aging_replies: { ...CATALOG.aging_replies, updatedAt: 0 },
  };
  let proposals: InboxAgentProposal[] = [];

  function syncWithThreads() {
    const { threads } = readState();
    const byId = new Map(threads.map((t) => [t.id, t]));
    proposals = proposals.map((p) => {
      if (p.state !== "pending") return p;
      const t = byId.get(p.threadId);
      if (!t || t.archived || t.trashed || t.lastMessageAt !== p.threadRevision) {
        return { ...p, state: "superseded" };
      }
      return p;
    });
  }

  function snapshot(accountId?: string): InboxAgentsSnapshot {
    syncWithThreads();
    return {
      runnerEnabled,
      agents: structuredClone([configs.stale_reads, configs.resolved_and_expired, configs.aging_replies]),
      activeRuns: [],
      proposals: structuredClone(
        proposals.filter((p) => p.state === "pending" && (!accountId || accountId === "all" || p.accountId === accountId)),
      ),
    };
  }

  function runOne(agentId: InboxAgentId, accountId?: string) {
    if (!runnerEnabled) throw new Error("Inbox assistants are paused. Resume them before running.");
    const cfg = configs[agentId];
    const now = Math.floor(Date.now() / 1000);
    const { threads, tasks } = readState();
    const scoped = threads.filter(
      (t) =>
        !t.archived &&
        !t.trashed &&
        (cfg.includeStarred || !t.starred) &&
        (!accountId || accountId === "all" || t.accountId === accountId) &&
        (!cfg.accountId || t.accountId === cfg.accountId),
    );
    let scanned = 0;
    let proposed = 0;

    for (const t of scoped.slice(0, cfg.maxThreadsPerRun)) {
      const already = proposals.some(
        (p) => p.agentId === agentId && p.threadId === t.id && p.threadRevision === t.lastMessageAt && (p.state === "pending" || p.state === "dismissed"),
      );
      if (already) continue;

      if (agentId === "resolved_and_expired") {
        const doneTask = tasks.find((task) => task.threadId === t.id && task.status === "completed");
        if (doneTask) {
          scanned++;
          proposed++;
          proposals.unshift({
            id: `demo-prop-${crypto.randomUUID()}`,
            agentId,
            accountId: t.accountId,
            threadId: t.id,
            threadRevision: t.lastMessageAt,
            threadSubject: t.subject,
            threadSender: t.participants[0] ?? "",
            threadLastReceivedAt: t.lastReceivedAt ?? t.lastMessageAt,
            threadSnippet: t.snippet,
            threadUnread: t.unread,
            threadStarred: t.starred,
            kind: "archive_as_resolved",
            state: "pending",
            headline: `Completed task "${doneTask.title}"`,
            reason: `The linked task "${doneTask.title}" was marked completed after this email arrived.`,
            payload: { resolvedByKind: "task", resolvedById: doneTask.id },
            evidence: [{ kind: "task", id: doneTask.id, label: doneTask.title, excerpt: null }],
            createdAt: now,
            updatedAt: now,
          });
          continue;
        }
      }

      if (agentId === "stale_reads" && (t.labels.includes("CATEGORY_UPDATES") || t.labels.includes("CATEGORY_PROMOTIONS") || t.snippet.length > 40)) {
        scanned++;
        proposed++;
        proposals.unshift({
          id: `demo-prop-${crypto.randomUUID()}`,
          agentId,
          accountId: t.accountId,
          threadId: t.id,
          threadRevision: t.lastMessageAt,
          threadSubject: t.subject,
          threadSender: t.participants[0] ?? "",
          threadLastReceivedAt: t.lastReceivedAt ?? t.lastMessageAt,
          threadSnippet: t.snippet,
          threadUnread: t.unread,
          threadStarred: t.starred,
          kind: "archive_with_summary",
          state: "pending",
          headline: `Summary ready: ${t.subject}`,
          reason: "Older informational message ready to skim and archive.",
          payload: {
            summary: `• ${t.snippet}`,
            noteTitle: t.subject,
            suggestSaveToNote: true,
          },
          evidence: [{ kind: "message", id: `${t.id}-message`, label: t.subject, excerpt: t.snippet }],
          createdAt: now,
          updatedAt: now,
        });
      }

      if (agentId === "aging_replies" && !t.labels.includes("CATEGORY_UPDATES") && !t.labels.includes("CATEGORY_PROMOTIONS") && !t.labels.includes("SENT")) {
        scanned++;
        proposed++;
        proposals.unshift({
          id: `demo-prop-${crypto.randomUUID()}`,
          agentId,
          accountId: t.accountId,
          threadId: t.id,
          threadRevision: t.lastMessageAt,
          threadSubject: t.subject,
          threadSender: t.participants[0] ?? "",
          threadLastReceivedAt: t.lastReceivedAt ?? t.lastMessageAt,
          threadSnippet: t.snippet,
          threadUnread: t.unread,
          threadStarred: t.starred,
          kind: "draft_reply",
          state: "pending",
          headline: `Reply draft ready for ${t.participants[0] ?? "sender"}`,
          reason: "This conversation has been waiting for a response.",
          payload: {
            messageId: `${t.id}-message`,
            replyDraft: `Thanks for reaching out about "${t.subject}". Here is a quick follow-up.`,
            gentleOutDraft: `Sorry for the slow reply — just checking in to see if this is still needed?`,
          },
          evidence: [{ kind: "message", id: `${t.id}-message`, label: t.subject, excerpt: t.snippet }],
          createdAt: now,
          updatedAt: now,
        });
      }
    }

    configs[agentId] = {
      ...cfg,
      lastRunAt: now,
      lastRunStatus: "completed",
      lastRunError: null,
      lastRunScanned: scanned,
      lastRunProposed: proposed,
      updatedAt: now,
    };
  }

  return {
    async getInboxAgentsSnapshot(accountId?: string): Promise<InboxAgentsSnapshot> {
      return snapshot(accountId);
    },
    async setInboxAgentRunnerEnabled(enabled: boolean): Promise<InboxAgentsSnapshot> {
      runnerEnabled = enabled;
      return snapshot();
    },
    async updateInboxAgentConfig(update: UpdateInboxAgentConfig): Promise<InboxAgentConfig> {
      if (update.ageDays < 1 || update.ageDays > 365) throw new Error("Age threshold must be between 1 and 365 days");
      if (update.maxThreadsPerRun < 1 || update.maxThreadsPerRun > 30) throw new Error("Max threads per run must be between 1 and 30");
      configs[update.agentId] = {
        ...configs[update.agentId],
        enabled: update.enabled,
        ageDays: update.ageDays,
        maxThreadsPerRun: update.maxThreadsPerRun,
        includeStarred: update.includeStarred,
        accountId: update.accountId,
        scheduleHours: update.scheduleHours,
        updatedAt: Math.floor(Date.now() / 1000),
      };
      return structuredClone(configs[update.agentId]);
    },
    async runInboxAgent(agentId: InboxAgentId, _aiConfig?: AiRunConfig | null, accountId?: string): Promise<InboxAgentsSnapshot> {
      runOne(agentId, accountId);
      return snapshot(accountId);
    },
    async runEnabledInboxAgents(_aiConfig?: AiRunConfig | null, accountId?: string): Promise<InboxAgentsSnapshot> {
      for (const id of ["stale_reads", "resolved_and_expired", "aging_replies"] as InboxAgentId[]) {
        if (configs[id].enabled) runOne(id, accountId);
      }
      return snapshot(accountId);
    },
    async resolveInboxAgentProposal(id: string, state: Extract<ProposalState, "accepted" | "dismissed">): Promise<void> {
      const target = proposals.find((p) => p.id === id);
      if (!target) throw new Error("Assistant proposal not found");
      target.state = state;
      target.updatedAt = Math.floor(Date.now() / 1000);
    },
    async syncInboxAgentAiConfig(_aiConfig: AiRunConfig | null): Promise<void> {},
  };
}
