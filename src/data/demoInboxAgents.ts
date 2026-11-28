import { parseAddress } from "../emailAddress";
import type {
  Account,
  AiRunConfig,
  InboxAgentConfig,
  InboxAgentId,
  InboxAgentProposal,
  InboxAgentsSnapshot,
  Message,
  ProposalState,
  Thread,
  ThreadTask,
  UpdateInboxAgentConfig,
} from "../domain";

type DemoState = {
  accounts: Account[];
  threads: Thread[];
  tasks: ThreadTask[];
  messagesFor(thread: Thread): Message[];
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
  sender_hygiene: {
    agentId: "sender_hygiene",
    name: "Sender Hygiene Advisor",
    description: "Spots senders you keep ignoring or dismissing within a second and suggests unsubscribing, muting, or giving them a split.",
    enabled: false,
    ageDays: 14,
    maxThreadsPerRun: 10,
    includeStarred: false,
    accountId: null,
    scheduleHours: null,
    lastRunAt: null,
    lastRunStatus: null,
    lastRunError: null,
    lastRunScanned: 0,
    lastRunProposed: 0,
  },
  stale_reads_digest: {
    agentId: "stale_reads_digest",
    name: "Stale Reads Digest",
    description: "Rolls up unread newsletters and articles older than a week into one scannable list you can clear in one click.",
    enabled: false,
    ageDays: 7,
    maxThreadsPerRun: 30,
    includeStarred: false,
    accountId: null,
    scheduleHours: 168,
    lastRunAt: null,
    lastRunStatus: null,
    lastRunError: null,
    lastRunScanned: 0,
    lastRunProposed: 0,
  },
};

const AGENT_ORDER: InboxAgentId[] = ["stale_reads", "resolved_and_expired", "aging_replies", "sender_hygiene", "stale_reads_digest"];
/** The digest's gist is the preview text, as in the app without an AI provider. */
const DEMO_GIST_MAX = 160;

/** The demo has no triage history, so a sender with several Inbox conversations stands in for one the user keeps ignoring. */
const DEMO_HYGIENE_MIN_THREADS = 2;
const DEMO_HYGIENE_QUICK = 5;
const DEMO_HYGIENE_DISMISSED = 6;

/** Days a pending suggestion waits, from creation, before it expires. Mirrors the app's per-kind limits. */
const DEMO_EXPIRY_DAYS: Record<InboxAgentProposal["kind"], number> = {
  draft_reply: 7,
  archive_with_summary: 14,
  archive_as_expired: 21,
  archive_as_resolved: 21,
  sender_hygiene: 30,
  digest_item: 14,
};

/** Dismissed and expired suggestions both stop the same conversation revision from being suggested again. */
const settledWithoutAction = (p: InboxAgentProposal) => p.state === "dismissed" || p.state === "expired";

export function createDemoInboxAgents(readState: () => DemoState) {
  let runnerEnabled = true;
  const configs: Record<InboxAgentId, InboxAgentConfig> = {
    stale_reads: { ...CATALOG.stale_reads, updatedAt: 0 },
    resolved_and_expired: { ...CATALOG.resolved_and_expired, updatedAt: 0 },
    aging_replies: { ...CATALOG.aging_replies, updatedAt: 0 },
    sender_hygiene: { ...CATALOG.sender_hygiene, updatedAt: 0 },
    stale_reads_digest: { ...CATALOG.stale_reads_digest, updatedAt: 0 },
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
      const nowSeconds = Math.floor(Date.now() / 1000);
      if (nowSeconds - p.createdAt >= DEMO_EXPIRY_DAYS[p.kind] * 86_400) {
        return { ...p, state: "expired", updatedAt: nowSeconds };
      }
      return p;
    });
  }

  function snapshot(accountId?: string): InboxAgentsSnapshot {
    syncWithThreads();
    return {
      runnerEnabled,
      agents: structuredClone(AGENT_ORDER.map((id) => configs[id])),
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

    if (agentId === "sender_hygiene") {
      const bySender = new Map<string, { name: string; email: string; threads: Thread[]; unsubscribeId: string | null }>();
      for (const t of scoped) {
        const messages = t.labels.includes("SENT") ? [] : readState().messagesFor(t);
        const latest = messages[messages.length - 1];
        if (!latest) continue;
        const parsed = parseAddress(latest.sender);
        const email = parsed.email.toLowerCase();
        if (!email) continue;
        const key = `${t.accountId}\n${email}`;
        const entry = bySender.get(key) ?? { name: parsed.name || email, email, threads: [], unsubscribeId: null };
        entry.threads.push(t);
        bySender.set(key, entry);
      }
      for (const entry of [...bySender.values()].sort((a, b) => b.threads.length - a.threads.length)) {
        if (proposed >= cfg.maxThreadsPerRun) break;
        if (entry.threads.length < DEMO_HYGIENE_MIN_THREADS) continue;
        scanned++;
        const sorted = [...entry.threads].sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt));
        const withUnsubscribe = sorted.find((t) => readState().messagesFor(t).some((m) => m.unsubscribe));
        const anchor = withUnsubscribe ?? sorted[0];
        const already = proposals.some(
          (p) => p.agentId === agentId && p.payload.senderEmail === entry.email && (p.state === "pending" || (settledWithoutAction(p) && p.threadId === anchor.id)),
        );
        if (already) continue;
        const unsubscribeMessage = withUnsubscribe ? readState().messagesFor(withUnsubscribe).find((m) => m.unsubscribe) : undefined;
        proposed++;
        proposals.unshift({
          id: `demo-prop-${crypto.randomUUID()}`,
          agentId,
          accountId: anchor.accountId,
          threadId: anchor.id,
          threadRevision: anchor.lastMessageAt,
          threadSubject: anchor.subject,
          threadSender: anchor.participants[0] ?? "",
          threadLastReceivedAt: anchor.lastReceivedAt ?? anchor.lastMessageAt,
          threadSnippet: anchor.snippet,
          threadUnread: anchor.unread,
          threadStarred: anchor.starred,
          kind: "sender_hygiene",
          state: "pending",
          headline: `You usually skip ${entry.name}`,
          reason: `Dismissed ${DEMO_HYGIENE_QUICK} of ${DEMO_HYGIENE_DISMISSED} emails within a second; ${entry.threads.length} emails still in your Inbox.`,
          payload: {
            senderEmail: entry.email,
            senderName: entry.name,
            agingUnreadCount: entry.threads.filter((t) => t.unread).length,
            quickDismissCount: DEMO_HYGIENE_QUICK,
            dismissCount: DEMO_HYGIENE_DISMISSED,
            inboxThreadIds: sorted.map((t) => t.id),
            recommendation: unsubscribeMessage ? "unsubscribe" : "mute",
            ...(unsubscribeMessage ? { messageId: unsubscribeMessage.id } : {}),
          },
          evidence: sorted.slice(0, 3).map((t) => ({ kind: "thread", id: t.id, label: t.subject, excerpt: t.snippet })),
          createdAt: now,
          updatedAt: now,
        });
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
      return;
    }

    if (agentId === "stale_reads_digest") {
      const cutoff = Date.now() - cfg.ageDays * 86_400_000;
      const ownAddresses = new Set(readState().accounts.map((a) => a.email.toLowerCase()));
      const bulk = scoped
        .filter((t) => {
          if (!t.unread || Date.parse(t.lastReceivedAt ?? t.lastMessageAt) > cutoff) return false;
          const messages = readState().messagesFor(t);
          const isBulk =
            t.labels.includes("CATEGORY_UPDATES") ||
            t.labels.includes("CATEGORY_PROMOTIONS") ||
            messages.some((m) => m.unsubscribe);
          const wroteIn = messages.some((m) => ownAddresses.has(parseAddress(m.sender).email.toLowerCase())) || t.labels.includes("SENT");
          const claimed = proposals.some(
            (p) =>
              p.threadId === t.id &&
              ((p.state === "pending" && (p.agentId === "stale_reads" || p.agentId === agentId)) ||
                (p.agentId === agentId && settledWithoutAction(p) && p.threadRevision === t.lastMessageAt)),
          );
          return isBulk && !wroteIn && !claimed;
        })
        .sort((a, b) => Date.parse(a.lastReceivedAt ?? a.lastMessageAt) - Date.parse(b.lastReceivedAt ?? b.lastMessageAt))
        .slice(0, cfg.maxThreadsPerRun);
      for (const t of bulk) {
        scanned++;
        proposed++;
        const received = t.lastReceivedAt ?? t.lastMessageAt;
        const days = Math.max(1, Math.floor((Date.now() - Date.parse(received)) / 86_400_000));
        const gist = t.snippet.replace(/\s+/g, " ").trim().slice(0, DEMO_GIST_MAX);
        proposals.unshift({
          id: `demo-prop-${crypto.randomUUID()}`,
          agentId,
          accountId: t.accountId,
          threadId: t.id,
          threadRevision: t.lastMessageAt,
          threadSubject: t.subject,
          threadSender: t.participants[0] ?? "",
          threadLastReceivedAt: received,
          threadSnippet: t.snippet,
          threadUnread: t.unread,
          threadStarred: t.starred,
          kind: "digest_item",
          state: "pending",
          headline: t.subject,
          reason: `Unread for ${days} ${days === 1 ? "day" : "days"}`,
          payload: { summary: gist, takeaways: [gist], noteTitle: t.subject, suggestSaveToNote: false },
          evidence: [{ kind: "message", id: `${t.id}-message`, label: t.subject, excerpt: t.snippet }],
          createdAt: now,
          updatedAt: now,
        });
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
      return;
    }

    for (const t of scoped.slice(0, cfg.maxThreadsPerRun)) {
      const already = proposals.some(
        (p) =>
          (p.agentId === agentId && p.threadId === t.id && p.threadRevision === t.lastMessageAt && (p.state === "pending" || settledWithoutAction(p))) ||
          (agentId === "stale_reads" && p.kind === "digest_item" && p.threadId === t.id && p.state === "pending"),
      );
      if (already) continue;

      if (agentId === "resolved_and_expired") {
        const hasActiveTask = tasks.some(
          (task) => task.threadId === t.id && (task.status === "open" || task.status === "in_progress"),
        );
        const doneTask = hasActiveTask
          ? undefined
          : tasks.find(
              (task) =>
                task.threadId === t.id &&
                task.status === "completed" &&
                task.completionSource === "user" &&
                !task.completedByEmail,
            );
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
            reason: `You marked the linked task "${doneTask.title}" done after this email arrived.`,
            payload: { resolvedByKind: "task", resolvedById: doneTask.id, taskCompletionSource: "user" },
            evidence: [{
              kind: "task",
              id: doneTask.id,
              label: doneTask.title,
              excerpt: doneTask.completedAt ? `Marked done ${doneTask.completedAt}` : null,
            }],
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
          headline: t.subject,
          reason: "Older informational message ready to skim and archive.",
          payload: {
            takeaways: [t.snippet],
            deadline: null,
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
      for (const id of AGENT_ORDER) {
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
