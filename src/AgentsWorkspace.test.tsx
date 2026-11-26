import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentsWorkspace } from "./AgentsWorkspace";
import { formatFullDate } from "./dateFormatting";
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
        reason: "You marked the linked task \"Send invoice\" done after this email arrived.",
        payload: { resolvedByKind: "task", resolvedById: "task-1", taskCompletionSource: "user" },
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
        onCreateTask={vi.fn()}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );

    const heading = screen.getByRole("heading", { name: "Inbox Assistants" });
    expect(heading).toBeInTheDocument();
    const titleGroup = heading.closest(".agents-toolbar-title");
    expect(titleGroup).not.toBeNull();
    expect(titleGroup?.querySelector(".account-scope-chip")).not.toBeNull();
    const actionsGroup = titleGroup?.parentElement?.querySelector(".agents-toolbar-actions");
    expect(actionsGroup).not.toBeNull();
    for (const label of ["Run Enabled", "Pause All", "Refresh"]) {
      const btn = screen.getByRole("button", { name: label });
      expect(actionsGroup).toContainElement(btn);
      expect(titleGroup).not.toContainElement(btn);
    }
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
        onCreateTask={vi.fn()}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );
    expect(screen.getByText(/Inbox assistants are paused/)).toBeInTheDocument();
  });

  it("renders digestible stale read cards with takeaways, clean sender, deadline, optional Save to Notes, and Create Task", () => {
    const controller = makeController(makeSnapshot({
      proposals: [
        {
          id: "p-eo",
          agentId: "stale_reads",
          accountId: "you@example.com",
          threadId: "t-eo",
          threadRevision: "2026-11-17T00:00:00Z",
          threadSubject: "Member Leaders: Encourage Your EO Chapter to Participate in the EO Member Experience Survey",
          threadSender: "\"Entrepreneurs' Organization (EO)\" <memberengagement@eonetwork.org>",
          threadLastReceivedAt: "2026-11-17T00:00:00Z",
          threadSnippet: "2026-2027 survey open",
          threadUnread: true,
          threadStarred: false,
          kind: "archive_with_summary",
          state: "pending",
          headline: "EO Member Experience Survey open through Dec 16",
          reason: "This repetition should stay hidden on stale read cards.",
          payload: {
            takeaways: [
              "Encourage chapter members to complete the 2026–2027 Member Experience Survey.",
              "Area and Member Engagement Directors are asked to promote participation before renewal season.",
            ],
            deadline: "Dec 16",
            suggestSaveToNote: false,
          },
          evidence: [{ kind: "message", id: "m-eo", label: "EO Survey", excerpt: "is now open through 16 December." }],
          createdAt: 101,
          updatedAt: 101,
        },
        {
          id: "p-handled",
          agentId: "resolved_and_expired",
          accountId: "you@example.com",
          threadId: "t-handled",
          threadRevision: "2026-11-10T00:00:00Z",
          threadSubject: "Contract confirmation",
          threadSender: "partner@example.com",
          threadLastReceivedAt: "2026-11-10T00:00:00Z",
          threadSnippet: "Signed copy attached",
          threadUnread: false,
          threadStarred: false,
          kind: "archive_as_resolved",
          state: "pending",
          headline: "Contract already confirmed",
          reason: "You replied on another thread.",
          payload: { resolvedByKind: "thread", resolvedById: "t-reply" },
          evidence: [{ kind: "thread", id: "t-reply", label: "Re: Contract", excerpt: "Received, thank you." }],
          createdAt: 100,
          updatedAt: 100,
        },
      ],
    }));
    const onCreateTask = vi.fn();

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
        onCreateTask={onCreateTask}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );

    const eoCard = screen.getByRole("article", {
      name: "Member Leaders: Encourage Your EO Chapter to Participate in the EO Member Experience Survey",
    });
    expect(within(eoCard).getByRole("heading", { name: "EO Member Experience Survey open through Dec 16" })).toBeInTheDocument();
    expect(within(eoCard).getByText("Member Leaders: Encourage Your EO Chapter to Participate in the EO Member Experience Survey")).toBeInTheDocument();

    // Parsed clean display name without raw angle brackets, with raw email tooltip.
    const senderLabel = within(eoCard).getByText("Entrepreneurs' Organization (EO)");
    expect(senderLabel).toHaveAttribute("title", "memberengagement@eonetwork.org");
    expect(within(eoCard).queryByText(/memberengagement@eonetwork\.org/)).not.toBeInTheDocument();

    // Single connected account omits account email noise.
    expect(within(eoCard).queryByText("you@example.com")).not.toBeInTheDocument();

    // Takeaways list + deadline chip shown; redundant rationale and excerpt omitted.
    expect(within(eoCard).getByText("Deadline: Dec 16")).toBeInTheDocument();
    const bullets = within(eoCard).getAllByRole("listitem");
    expect(bullets).toHaveLength(2);
    expect(within(eoCard).queryByText("This repetition should stay hidden on stale read cards.")).not.toBeInTheDocument();
    expect(within(eoCard).queryByText("is now open through 16 December.")).not.toBeInTheDocument();

    // `suggestSaveToNote: false` omits Save to Notes & Archive; Create Task is wired.
    expect(within(eoCard).queryByRole("button", { name: "Save to Notes & Archive" })).not.toBeInTheDocument();
    fireEvent.click(within(eoCard).getByRole("button", { name: "Create Task" }));
    expect(onCreateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "p-eo" }));

    // Non-summary cards still show reason and verbatim evidence.
    const handledCard = screen.getByRole("article", { name: "Contract confirmation" });
    expect(within(handledCard).getByText("You replied on another thread.")).toBeInTheDocument();
    expect(within(handledCard).getByText("Received, thank you.")).toBeInTheDocument();
  });

  it("shows proposal account only when viewing All Accounts with multiple connected accounts", () => {
    const twoAccounts: Account[] = [
      ACCOUNTS[0],
      { ...ACCOUNTS[0], email: "work@example.com", displayName: "Work", sortOrder: 1 },
    ];
    const snapshot = makeSnapshot({ proposals: [makeSnapshot().proposals[0]] });

    const { rerender } = render(
      <AgentsWorkspace
        controller={makeController(snapshot)}
        accounts={twoAccounts}
        accountId={null}
        onOpenThread={vi.fn()}
        onOpenTask={vi.fn()}
        onArchiveProposal={vi.fn()}
        onArchiveBatch={vi.fn()}
        onSaveToNoteAndArchive={vi.fn()}
        onCreateTask={vi.fn()}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );
    expect(within(screen.getByRole("article", { name: "Engineering Weekly" })).getByText("you@example.com")).toBeInTheDocument();

    rerender(
      <AgentsWorkspace
        controller={makeController(snapshot)}
        accounts={twoAccounts}
        accountId="you@example.com"
        onOpenThread={vi.fn()}
        onOpenTask={vi.fn()}
        onArchiveProposal={vi.fn()}
        onArchiveBatch={vi.fn()}
        onSaveToNoteAndArchive={vi.fn()}
        onCreateTask={vi.fn()}
        onReopenTask={vi.fn()}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );
    expect(within(screen.getByRole("article", { name: "Engineering Weekly" })).queryByText("you@example.com")).not.toBeInTheDocument();
  });

  it("formats task completion evidence, offers Reopen Task or Create Task appropriately, and guards Archive All Safe", () => {
    const onReopenTask = vi.fn();
    const onCreateTask = vi.fn();
    const onArchiveBatch = vi.fn();
    const completedIso = "2026-11-16T18:17:01.778288+00:00";
    const expectedDone = `Marked done ${formatFullDate(new Date(completedIso))}`;

    const snapshot = makeSnapshot({
      proposals: [
        {
          id: "p-task",
          agentId: "resolved_and_expired",
          accountId: "you@example.com",
          threadId: "t-task",
          threadRevision: "2026-11-15T00:00:00Z",
          threadSubject: "Re: Data Quality Resources",
          threadSender: "Carolyn Casavan <carolyn@example.com>",
          threadLastReceivedAt: "2026-11-15T00:00:00Z",
          threadSnippet: "Guidance attached",
          threadUnread: true,
          threadStarred: false,
          kind: "archive_as_resolved",
          state: "pending",
          headline: "Completed task \"Review links\"",
          reason: "You marked the linked task \"Review links\" done after this email arrived.",
          payload: { resolvedByKind: "task", resolvedById: "task-99", taskCompletionSource: "user" },
          evidence: [{ kind: "task", id: "task-99", label: "Review links", excerpt: `Completed ${completedIso}` }],
          createdAt: 102,
          updatedAt: 102,
        },
        {
          id: "p-thread-resolved",
          agentId: "resolved_and_expired",
          accountId: "you@example.com",
          threadId: "t-other",
          threadRevision: "2026-11-14T00:00:00Z",
          threadSubject: "Water system quote",
          threadSender: "vendor@example.com",
          threadLastReceivedAt: "2026-11-14T00:00:00Z",
          threadSnippet: "Quote details",
          threadUnread: false,
          threadStarred: false,
          kind: "archive_as_resolved",
          state: "pending",
          headline: "Handled in another thread",
          reason: "Replied elsewhere.",
          payload: { resolvedByKind: "thread", resolvedById: "t-reply-2" },
          evidence: [{ kind: "thread", id: "t-reply-2", label: "Re: Quote", excerpt: "Confirmed" }],
          createdAt: 101,
          updatedAt: 101,
        },
        {
          id: "p-legacy-task",
          agentId: "resolved_and_expired",
          accountId: "you@example.com",
          threadId: "t-legacy",
          threadRevision: "2026-11-13T00:00:00Z",
          threadSubject: "Legacy item",
          threadSender: "legacy@example.com",
          threadLastReceivedAt: "2026-11-13T00:00:00Z",
          threadSnippet: "Legacy",
          threadUnread: false,
          threadStarred: false,
          kind: "archive_as_resolved",
          state: "pending",
          headline: "Completed task \"Old item\"",
          reason: "Legacy proposal without source metadata.",
          payload: { resolvedByKind: "task", resolvedById: "task-old" },
          evidence: [{ kind: "task", id: "task-old", label: "Old item", excerpt: null }],
          createdAt: 100,
          updatedAt: 100,
        },
        {
          id: "p-expired",
          agentId: "resolved_and_expired",
          accountId: "you@example.com",
          threadId: "t-exp",
          threadRevision: "2026-11-01T00:00:00Z",
          threadSubject: "Past workshop",
          threadSender: "events@example.com",
          threadLastReceivedAt: "2026-11-01T00:00:00Z",
          threadSnippet: "Workshop ended",
          threadUnread: false,
          threadStarred: false,
          kind: "archive_as_expired",
          state: "pending",
          headline: "Workshop already passed",
          reason: "Event date passed.",
          payload: { expiredDate: "2026-11-05" },
          evidence: [],
          createdAt: 99,
          updatedAt: 99,
        },
      ],
    });

    render(
      <AgentsWorkspace
        controller={makeController(snapshot)}
        accounts={ACCOUNTS}
        accountId={null}
        onOpenThread={vi.fn()}
        onOpenTask={vi.fn()}
        onArchiveProposal={vi.fn()}
        onArchiveBatch={onArchiveBatch}
        onSaveToNoteAndArchive={vi.fn()}
        onCreateTask={onCreateTask}
        onReopenTask={onReopenTask}
        onReviewDraft={vi.fn()}
        onOpenAiSettings={vi.fn()}
      />,
    );

    // Task-linked card formats completion timestamp cleanly and offers Reopen Task (not duplicate Create Task).
    const taskCard = screen.getByRole("article", { name: "Re: Data Quality Resources" });
    expect(within(taskCard).getByText(expectedDone)).toBeInTheDocument();
    expect(within(taskCard).queryByText(/T18:17:01/)).not.toBeInTheDocument();
    expect(within(taskCard).queryByRole("button", { name: "Create Task" })).not.toBeInTheDocument();
    fireEvent.click(within(taskCard).getByRole("button", { name: "Reopen Task" }));
    expect(onReopenTask).toHaveBeenCalledWith(expect.objectContaining({ id: "p-task" }), "task-99");

    // Non-task cards offer Create Task.
    const threadCard = screen.getByRole("article", { name: "Water system quote" });
    fireEvent.click(within(threadCard).getByRole("button", { name: "Create Task" }));
    expect(onCreateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "p-thread-resolved" }));

    const expiredCard = screen.getByRole("article", { name: "Past workshop" });
    fireEvent.click(within(expiredCard).getByRole("button", { name: "Create Task" }));
    expect(onCreateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "p-expired" }));

    // Batch safe archive includes only user-completed task + expired (excludes thread-resolved and legacy unknown source).
    fireEvent.click(screen.getByRole("button", { name: "Archive All 2 Safe" }));
    expect(onArchiveBatch).toHaveBeenCalledTimes(1);
    expect(onArchiveBatch.mock.calls[0][0].map((p: { id: string }) => p.id)).toEqual(["p-task", "p-expired"]);
  });
});
