import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentsWorkspace } from "./AgentsWorkspace";
import type { Account, InboxAgentProposal, InboxAgentsSnapshot } from "./domain";
import { InboxAssistantBanner } from "./InboxAssistantBanner";
import type { InboxAgentsController } from "./useInboxAgents";

const ACCOUNTS: Account[] = [
  {
    email: "you@example.com",
    displayName: "You",
    color: "#4285F4",
    status: "connected",
    provider: "gmail",
    labelModel: "gmailLabels",
    sortOrder: 0,
    connectedAt: "2026-11-01T00:00:00Z",
    lastSyncedAt: null,
    onThisDevice: true,
  },
];

function daysAgo(days: number): string {
  return new Date(Date.now() - days * 86_400_000).toISOString();
}

function digestItem(id: string, subject: string, days: number, payload: InboxAgentProposal["payload"] = {}): InboxAgentProposal {
  return {
    id,
    agentId: "stale_reads_digest",
    accountId: "you@example.com",
    threadId: `t-${id}`,
    threadRevision: daysAgo(days),
    threadSubject: subject,
    threadSender: "Weekend Reader <hi@reader.example>",
    threadLastReceivedAt: daysAgo(days),
    threadSnippet: "Preview",
    threadUnread: true,
    threadStarred: false,
    kind: "digest_item",
    state: "pending",
    headline: subject,
    reason: `Unread for ${days} days`,
    payload: { summary: `Gist of ${subject}.`, takeaways: [`Gist of ${subject}.`], noteTitle: subject, suggestSaveToNote: false, ...payload },
    evidence: [],
    createdAt: 100,
    updatedAt: 100,
  };
}

const OTHER: InboxAgentProposal = {
  id: "other",
  agentId: "resolved_and_expired",
  accountId: "you@example.com",
  threadId: "t-other",
  threadRevision: "2026-11-02T00:00:00Z",
  threadSubject: "Old Webinar",
  threadSender: "events@example.com",
  threadLastReceivedAt: "2026-11-02T00:00:00Z",
  threadSnippet: "Nov 10 webinar",
  threadUnread: true,
  threadStarred: false,
  kind: "archive_as_expired",
  state: "pending",
  headline: "Webinar already passed",
  reason: "Event date passed.",
  payload: { expiredDate: "2026-11-10" },
  evidence: [],
  createdAt: 50,
  updatedAt: 50,
};

function snapshotWith(proposals: InboxAgentProposal[]): InboxAgentsSnapshot {
  return {
    runnerEnabled: true,
    agents: [
      {
        agentId: "stale_reads_digest",
        name: "Stale Reads Digest",
        description: "Rolls up unread reads.",
        enabled: true,
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
        updatedAt: 0,
      },
    ],
    activeRuns: [],
    proposals,
  };
}

function controllerFor(snapshot: InboxAgentsSnapshot): InboxAgentsController {
  return {
    snapshot,
    loaded: true,
    busy: false,
    error: "",
    pendingCount: snapshot.proposals.length,
    refresh: vi.fn().mockResolvedValue(undefined),
    toggleRunner: vi.fn().mockResolvedValue(true),
    updateConfig: vi.fn().mockResolvedValue(true),
    runAgent: vi.fn().mockResolvedValue(true),
    runEnabled: vi.fn().mockResolvedValue(true),
    resolveProposal: vi.fn().mockResolvedValue(true),
    proposalsForThread: (tid) => snapshot.proposals.filter((p) => p.threadId === tid),
  };
}

function setup(proposals: InboxAgentProposal[]) {
  const controller = controllerFor(snapshotWith(proposals));
  const handlers = {
    onOpenThread: vi.fn(),
    onArchiveProposal: vi.fn(),
    onArchiveBatch: vi.fn(),
    onSaveToNoteAndArchive: vi.fn(),
  };
  render(
    <AgentsWorkspace
      controller={controller}
      accounts={ACCOUNTS}
      accountId={null}
      onOpenThread={handlers.onOpenThread}
      onOpenTask={vi.fn()}
      onArchiveProposal={handlers.onArchiveProposal}
      onArchiveBatch={handlers.onArchiveBatch}
      onSaveToNoteAndArchive={handlers.onSaveToNoteAndArchive}
      onCreateTask={vi.fn()}
      onReopenTask={vi.fn()}
      onReviewDraft={vi.fn()}
      onOpenAiSettings={vi.fn()}
      senderHygiene={{ splits: [], onUnsubscribe: vi.fn(), onMute: vi.fn(), onNewSplit: vi.fn(), onArchiveAll: vi.fn() }}
    />,
  );
  return { controller, ...handlers };
}

describe("Stale Reads Digest in the Agents workspace", () => {
  afterEach(cleanup);

  const items = [
    digestItem("d1", "Issue 41", 9),
    digestItem("d2", "Issue 42", 12, { deadline: "Dec 16", suggestSaveToNote: true }),
    digestItem("d3", "Issue 43", 8),
  ];

  it("groups every digest item into one card with a count, oldest first, and per-row details", () => {
    setup([OTHER, ...items]);
    const cards = screen.getAllByRole("article").filter((el) => el.classList.contains("assistant-proposal-card"));
    // One card for the digest plus one for the other proposal.
    expect(cards).toHaveLength(2);
    const digest = screen.getByRole("article", { name: "Digest of 3 unread reads" });
    expect(within(digest).getByRole("heading", { name: /^Stale Reads Digest\s*3 unread reads$/ })).toBeInTheDocument();
    const rows = within(digest).getAllByRole("listitem");
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual(["Issue 42", "Issue 41", "Issue 43"]);
    expect(within(rows[0]).getByText("Weekend Reader")).toBeInTheDocument();
    expect(within(rows[0]).getByText("12 days old")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Gist of Issue 42.")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Deadline: Dec 16")).toBeInTheDocument();
    expect(within(rows[1]).queryByText(/Deadline/)).toBeNull();
    // Save to Notes is always offered; the suggested one is marked.
    for (const row of rows) expect(within(row).getByRole("button", { name: "Save to Notes & Archive" })).toBeInTheDocument();
    expect(within(rows[0]).getByRole("button", { name: "Save to Notes & Archive" })).toHaveClass("assistant-digest-save-suggested");
    expect(within(rows[1]).getByRole("button", { name: "Save to Notes & Archive" })).not.toHaveClass("assistant-digest-save-suggested");
    // The assistant's sidebar badge counts items.
    expect(screen.getByRole("button", { name: /Stale Reads Digest\s*3/ })).toBeInTheDocument();
  });

  it("places the digest where its newest item sits in the queue", () => {
    setup([digestItem("d1", "Issue 41", 9), OTHER, digestItem("d3", "Issue 43", 8)]);
    const labels = screen
      .getAllByRole("article")
      .filter((el) => el.classList.contains("assistant-proposal-card"))
      .map((el) => el.getAttribute("aria-label"));
    expect(labels).toEqual(["Digest of 2 unread reads", "Old Webinar"]);
  });

  it("archives every item with Archive All and never offers it as a safe batch", () => {
    const { onArchiveBatch } = setup(items);
    expect(screen.queryByRole("button", { name: /Safe$/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Archive All 3" }));
    expect(onArchiveBatch).toHaveBeenCalledTimes(1);
    expect(onArchiveBatch.mock.calls[0][0].map((p: InboxAgentProposal) => p.id).sort()).toEqual(["d1", "d2", "d3"]);
  });

  it("saves, opens, and dismisses a single row", () => {
    const { controller, onSaveToNoteAndArchive, onOpenThread } = setup(items);
    const row = screen.getByRole("listitem", { name: "Issue 41" });
    fireEvent.click(within(row).getByRole("button", { name: "Save to Notes & Archive" }));
    expect(onSaveToNoteAndArchive).toHaveBeenCalledWith(expect.objectContaining({ id: "d1" }));
    fireEvent.click(within(row).getByRole("button", { name: "Open Conversation" }));
    expect(onOpenThread).toHaveBeenCalledWith("t-d1");
    fireEvent.click(within(row).getByRole("button", { name: "Dismiss" }));
    expect(controller.resolveProposal).toHaveBeenCalledTimes(1);
    expect(controller.resolveProposal).toHaveBeenCalledWith("d1", "dismissed");
  });

  it("dismisses every item with Dismiss All", () => {
    const { controller } = setup(items);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss All" }));
    expect(controller.resolveProposal).toHaveBeenCalledTimes(3);
    for (const id of ["d1", "d2", "d3"]) expect(controller.resolveProposal).toHaveBeenCalledWith(id, "dismissed");
  });

  it("treats the digest as one keyboard stop: e and x act on all items, o opens the first", () => {
    const { controller, onArchiveBatch, onArchiveProposal, onOpenThread } = setup([...items, OTHER]);
    const section = screen.getByRole("region", { name: "Inbox Assistants" });
    // The digest is first (newest items) and selected; one j reaches the next card, k returns.
    fireEvent.keyDown(section, { key: "j" });
    fireEvent.keyDown(section, { key: "j" });
    expect(onArchiveProposal).not.toHaveBeenCalled();
    fireEvent.keyDown(section, { key: "e" });
    expect(onArchiveProposal).toHaveBeenCalledWith(expect.objectContaining({ id: "other" }));
    fireEvent.keyDown(section, { key: "k" });
    fireEvent.keyDown(section, { key: "e" });
    expect(onArchiveBatch).toHaveBeenCalledTimes(1);
    expect(onArchiveBatch.mock.calls[0][0]).toHaveLength(3);
    fireEvent.keyDown(section, { key: "o" });
    expect(onOpenThread).toHaveBeenCalledWith("t-d2");
    fireEvent.keyDown(section, { key: "x" });
    expect(controller.resolveProposal).toHaveBeenCalledTimes(3);
  });
});

describe("InboxAssistantBanner for a digest item", () => {
  afterEach(cleanup);

  it("shows the gist with Save to Notes & Archive, Archive, and Dismiss", () => {
    const item = digestItem("d1", "Issue 41", 9);
    const onArchive = vi.fn();
    const onSave = vi.fn();
    const onDismiss = vi.fn();
    render(
      <InboxAssistantBanner
        proposals={[item]}
        busy={false}
        onArchive={onArchive}
        onSaveToNoteAndArchive={onSave}
        onCreateTask={vi.fn()}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenEvidenceThread={vi.fn()}
        onOpenEvidenceTask={vi.fn()}
        onDismiss={onDismiss}
        senderHygiene={{ splits: [], onUnsubscribe: vi.fn(), onMute: vi.fn(), onNewSplit: vi.fn(), onArchiveAll: vi.fn() }}
      />,
    );
    expect(screen.getByText("Digest")).toBeInTheDocument();
    expect(screen.getByText("Gist of Issue 41.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save to Notes & Archive" }));
    fireEvent.click(screen.getByRole("button", { name: "Archive" }));
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onSave).toHaveBeenCalledWith(item);
    expect(onArchive).toHaveBeenCalledWith(item);
    expect(onDismiss).toHaveBeenCalledWith(item);
  });
});
