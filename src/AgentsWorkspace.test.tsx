import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentsWorkspace } from "./AgentsWorkspace";
import type { Account, InboxAgentsSnapshot } from "./domain";
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

function makeSnapshot(overrides?: Partial<InboxAgentsSnapshot>): InboxAgentsSnapshot {
  return {
    runnerEnabled: true,
    agents: [
      {
        agentId: "stale_reads",
        name: "Stale Reads Summarizer",
        description: "Summarizes aging reads.",
        enabled: true,
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
        updatedAt: 0,
      },
      {
        agentId: "resolved_and_expired",
        name: "Expired & Already-Handled Sweeper",
        description: "Spots resolved and expired mail.",
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
        updatedAt: 0,
      },
      {
        agentId: "aging_replies",
        name: "Aging Reply Drafter",
        description: "Drafts replies to aging emails.",
        enabled: true,
        ageDays: 3,
        maxThreadsPerRun: 10,
        includeStarred: true,
        accountId: null,
        scheduleHours: 24,
        lastRunAt: null,
        lastRunStatus: null,
        lastRunError: null,
        lastRunScanned: 0,
        lastRunProposed: 0,
        updatedAt: 0,
      },
    ],
    activeRuns: [],
    proposals: [
      {
        id: "p1",
        agentId: "stale_reads",
        accountId: "you@example.com",
        threadId: "t1",
        threadRevision: "2026-11-01T00:00:00Z",
        threadSubject: "Engineering Weekly",
        threadSender: "news@example.com",
        threadLastReceivedAt: "2026-11-01T00:00:00Z",
        threadSnippet: "Release notes",
        threadUnread: true,
        threadStarred: false,
        kind: "archive_with_summary",
        state: "pending",
        headline: "Summary of Engineering Weekly",
        reason: "Informational digest from 3 weeks ago.",
        payload: { summary: "• Key highlights", noteTitle: "Engineering Weekly", suggestSaveToNote: true },
        evidence: [],
        createdAt: 100,
        updatedAt: 100,
      },
      {
        id: "p2",
        agentId: "resolved_and_expired",
        accountId: "you@example.com",
        threadId: "t2",
        threadRevision: "2026-11-02T00:00:00Z",
        threadSubject: "Old Webinar",
        threadSender: "events@example.com",
        threadLastReceivedAt: "2026-11-02T00:00:00Z",
        threadSnippet: "Nov 10 webinar",
        threadUnread: true,
        threadStarred: false,
        kind: "archive_as_expired",
        state: "pending",
        headline: "Webinar on Nov 10 already passed",
        reason: "Event date passed.",
        payload: { expiredDate: "2026-11-10" },
        evidence: [],
        createdAt: 99,
        updatedAt: 99,
      },
      {
        id: "p3",
        agentId: "resolved_and_expired",
        accountId: "you@example.com",
        threadId: "t3",
        threadRevision: "2026-11-03T00:00:00Z",
        threadSubject: "Send Invoice",
        threadSender: "acct@example.com",
        threadLastReceivedAt: "2026-11-03T00:00:00Z",
        threadSnippet: "Please send invoice",
        threadUnread: false,
        threadStarred: false,
        kind: "archive_as_resolved",
        state: "pending",
        headline: "Completed task \"Send invoice\"",
        reason: "Task marked completed.",
        payload: { resolvedByKind: "task", resolvedById: "task-1" },
        evidence: [{ kind: "task", id: "task-1", label: "Send invoice", excerpt: null }],
        createdAt: 98,
        updatedAt: 98,
      },
    ],
    ...overrides,
  };
}

function makeController(snapshot: InboxAgentsSnapshot): InboxAgentsController {
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

describe("AgentsWorkspace", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("renders assistants, validates configuration modal, and supports batch safe archive & keyboard shortcuts", async () => {
    const controller = makeController(makeSnapshot());
    const onArchiveProposal = vi.fn();
    const onArchiveBatch = vi.fn();
    const onSaveToNoteAndArchive = vi.fn();
    const onOpenThread = vi.fn();

    render(
      <AgentsWorkspace
        controller={controller}
        accounts={ACCOUNTS}
        accountId={null}
        onOpenThread={onOpenThread}
        onOpenTask={vi.fn()}
        onArchiveProposal={onArchiveProposal}
        onArchiveBatch={onArchiveBatch}
        onSaveToNoteAndArchive={onSaveToNoteAndArchive}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "Inbox Assistants" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Archive All 2 Safe" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Archive All 2 Safe" }));
    expect(onArchiveBatch).toHaveBeenCalledTimes(1);
    expect(onArchiveBatch.mock.calls[0][0]).toHaveLength(2);

    // Keyboard shortcuts on the first selected card (`p1`, Stale Read Summary)
    const section = screen.getByRole("region", { name: "Inbox Assistants" });
    fireEvent.keyDown(section, { key: "n" });
    expect(onSaveToNoteAndArchive).toHaveBeenCalledWith(expect.objectContaining({ id: "p1" }));

    fireEvent.keyDown(section, { key: "j" });
    fireEvent.keyDown(section, { key: "e" });
    expect(onArchiveProposal).toHaveBeenCalledWith(expect.objectContaining({ id: "p2" }));

    // Configure modal layout, initial focus, validation, and save
    const configureBtn = screen.getByRole("button", { name: "Configure Stale Reads Summarizer" });
    fireEvent.click(configureBtn);
    const dialog = await screen.findByRole("dialog", { name: "Configure Stale Reads Summarizer" });
    expect(dialog.querySelector("form.modal-form")).not.toBeNull();
    expect(dialog.querySelector(".modal-form-actions")).not.toBeNull();
    expect(dialog.querySelectorAll(".modal-form-check")).toHaveLength(2);
    const ageInput = within(dialog).getByLabelText("Only Review Emails Older Than (Days)");
    expect(ageInput).toHaveFocus();

    fireEvent.change(ageInput, { target: { value: "0" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("1 and 365");
    expect(ageInput).toHaveValue(0);

    fireEvent.change(ageInput, { target: { value: "10" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(controller.updateConfig).toHaveBeenCalledWith(expect.objectContaining({ agentId: "stale_reads", ageDays: 10 })));
  });

  it("shows paused banner when runner is paused", () => {
    const controller = makeController(makeSnapshot({ runnerEnabled: false, proposals: [] }));
    render(
      <AgentsWorkspace
        controller={controller}
        accounts={ACCOUNTS}
        accountId={null}
        onOpenThread={vi.fn()}
        onOpenTask={vi.fn()}
        onArchiveProposal={vi.fn()}
        onArchiveBatch={vi.fn()}
        onSaveToNoteAndArchive={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );
    expect(screen.getByText(/Inbox assistants are paused/)).toBeInTheDocument();
  });
});
