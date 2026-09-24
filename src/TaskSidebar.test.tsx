import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskSidebar, type ThreadActionAnalysis } from "./TaskSidebar";
import { mailClient } from "./data/client";
import type { ActionProposal, ThreadDetail, ThreadTask } from "./domain";

const detail: ThreadDetail = {
  thread: {
    id: "thread-1",
    providerThreadId: "provider-1",
    subject: "Website setup",
    snippet: "Please set up the website by Friday.",
    participants: ["client@example.com"],
    lastMessageAt: "2026-09-19T10:00:00Z",
    lastReceivedAt: "2026-09-19T10:00:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: "you@example.com",
    summary: null,
    summaryGeneratedAt: null,
    hasAttachments: false,
  },
  messages: [{
    id: "message-1",
    threadId: "thread-1",
    sender: "client@example.com",
    recipients: ["you@example.com"],
    sentAt: "2026-09-19T10:00:00Z",
    bodyHtml: "<p>Please set up the website by Friday.</p>",
    bodyText: "Please set up the website by Friday.",
    unread: false,
    unsubscribe: null,
    attachments: [],
  }],
};

function threadAnalysis(overrides: Partial<ThreadActionAnalysis> = {}): ThreadActionAnalysis {
  return {
    enabled: true,
    ready: true,
    loading: false,
    error: null,
    preview: null,
    proposals: [],
    onAnalyze: vi.fn(),
    ...overrides,
  };
}

describe("TaskSidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps task creation out of the read-only sidebar", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const onNewTask = vi.fn();
    render(<TaskSidebar onClose={vi.fn()} accountId="you@example.com" currentThread={detail} onOpenThread={vi.fn()} onNewTask={onNewTask} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add task" }));
    expect(onNewTask).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("renders tasks and lets the user complete them", async () => {
    const task: ThreadTask = {
      id: "task-1",
      accountId: "you@example.com",
      threadId: "thread-1",
      sourceMessageId: null,
      subjectSnapshot: "Website setup",
      title: "Set up the website",
      notes: null,
      kind: "action",
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: null,
      waitAfter: null,
      createdAt: "2026-09-19T10:01:00Z",
      updatedAt: "2026-09-19T10:01:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue({ ...task, status: "completed", completionSource: "user" });
    render(<TaskSidebar onClose={vi.fn()} accountId={null} currentThread={null} onOpenThread={vi.fn()} />);

    expect(await screen.findByText("Set up the website")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Complete Set up the website" }));
    await waitFor(() => expect(setStatus).toHaveBeenCalledWith("task-1", "completed"));
  });

  it("offers a follow-up draft for a due follow-up task", async () => {
    const task: ThreadTask = {
      id: "follow-up-1",
      accountId: "you@example.com",
      threadId: "thread-1",
      sourceMessageId: "message-1",
      subjectSnapshot: "Website setup",
      title: "Check in with the client",
      notes: "Ask whether the launch date is still on track.",
      kind: "follow_up",
      dueKind: "date",
      dueValue: "2020-01-01",
      timeZone: "America/New_York",
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: null,
      waitAfter: null,
      createdAt: "2020-01-01T10:00:00Z",
      updatedAt: "2020-01-01T10:00:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const draftFollowUp = vi.fn();
    render(<TaskSidebar onClose={vi.fn()} accountId="you@example.com" currentThread={detail} onOpenThread={vi.fn()} onDraftFollowUp={draftFollowUp} />);

    fireEvent.click(await screen.findByRole("button", { name: "Draft Follow-Up" }));
    expect(draftFollowUp).toHaveBeenCalledWith(task);
  });

  it("previews typed thread proposals with evidence and confirmation actions", async () => {
    const proposal: ActionProposal = {
      type: "task",
      kind: "action",
      title: "Set up the website",
      notes: "Complete the requested setup.",
      dueKind: "date",
      dueValue: "2026-09-25",
      timeZone: "America/New_York",
      repeatIntervalDays: null,
      confidence: 0.86,
      evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website by Friday." },
    };
    const reviewProposal = vi.fn();
    const discard = vi.fn();
    render(
      <TaskSidebar
        onClose={vi.fn()}
        accountId="you@example.com"
        currentThread={detail}
        onOpenThread={vi.fn()}
        title="Actions"
        analysis={threadAnalysis({
          preview: '{"emailContext":{"messages":[]}}',
          proposals: [proposal],
          onDiscardProposal: discard,
          onReviewProposal: reviewProposal,
        })}
      />,
    );

    expect(await screen.findByText("Set up the website")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Evidence"));
    expect(screen.getByText("Please set up the website by Friday.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review & Add Task" }));
    expect(reviewProposal).toHaveBeenCalledWith(0, proposal, "accept");
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(discard).toHaveBeenCalledWith(0);
  });

  it("shows the account email and a task count in the header, like the mail inbox header", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar variant="workspace" onClose={vi.fn()} accountId="you@example.com" currentThread={null} onOpenThread={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("0 tasks")).toBeInTheDocument());
    expect(screen.getByText("you@example.com", { exact: false })).toBeInTheDocument();
  });

  it("renders a status pill and a clickable evidence quote in the task detail pane", async () => {
    const task: ThreadTask = {
      id: "task-1",
      accountId: "you@example.com",
      threadId: "thread-1",
      sourceMessageId: "message-1",
      subjectSnapshot: "Website setup",
      title: "Set up the website",
      notes: null,
      kind: "action",
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: "Please set up the website by Friday.",
      waitAfter: null,
      createdAt: "2026-09-19T10:01:00Z",
      updatedAt: "2026-09-19T10:01:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const onOpenThread = vi.fn();
    render(<TaskSidebar variant="workspace" onClose={vi.fn()} accountId="you@example.com" currentThread={null} onOpenThread={onOpenThread} />);

    const pill = await screen.findByText("open", { selector: ".task-status-pill" });
    expect(pill).toHaveClass("task-status-open");

    const evidenceButton = screen.getByRole("button", { name: "Please set up the website by Friday." });
    fireEvent.click(evidenceButton);
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
  });

  it("renders task detail actions as icon buttons with labelled shortcut tooltips", async () => {
    const task: ThreadTask = {
      id: "task-1",
      accountId: "you@example.com",
      threadId: "thread-1",
      sourceMessageId: "message-1",
      subjectSnapshot: "Website setup",
      title: "Set up the website",
      notes: null,
      kind: "action",
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: null,
      waitAfter: null,
      createdAt: "2026-09-19T10:01:00Z",
      updatedAt: "2026-09-19T10:01:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue({ ...task, status: "completed", completedAt: "2026-09-19T11:00:00Z" });
    const onOpenThread = vi.fn();
    const onEditTask = vi.fn();
    render(<TaskSidebar variant="workspace" onClose={vi.fn()} accountId="you@example.com" currentThread={null} onOpenThread={onOpenThread} onEditTask={onEditTask} />);

    const edit = await screen.findByRole("button", { name: "Edit (Enter)" });
    const done = screen.getByRole("button", { name: "Mark Done (e)" });
    const open = screen.getByRole("button", { name: "Open Conversation (o)" });
    for (const button of [edit, done, open]) {
      expect(button).toHaveClass("action-button");
      expect(button.querySelector("svg")).not.toBeNull();
    }
    const tooltips = screen.getAllByRole("tooltip", { hidden: true }).map((tooltip) => tooltip.textContent);
    expect(tooltips).toEqual(["EditEnter", "Mark donee", "Open conversationo"]);

    fireEvent.click(edit);
    expect(onEditTask).toHaveBeenCalledWith(task);
    fireEvent.click(open);
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
    fireEvent.click(done);
    await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("task-1", "completed"));
    expect(await screen.findByRole("button", { name: "Reopen (Shift+E)" })).toBeInTheDocument();
  });

  it("shows no conversation controls for a standalone task", async () => {
    const task: ThreadTask = {
      id: "standalone-1",
      accountId: "you@example.com",
      threadId: null,
      sourceMessageId: null,
      subjectSnapshot: null,
      title: "Buy printer paper",
      notes: null,
      kind: "action",
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: null,
      waitAfter: null,
      createdAt: "2026-09-20T10:01:00Z",
      updatedAt: "2026-09-20T10:01:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const onOpenThread = vi.fn();
    render(<TaskSidebar variant="workspace" onClose={vi.fn()} accountId="you@example.com" currentThread={null} onOpenThread={onOpenThread} />);

    expect(await screen.findByRole("heading", { name: "Buy printer paper" })).toBeInTheDocument();
    expect(screen.queryByText("Conversation")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open conversation" })).not.toBeInTheDocument();
    expect(screen.queryByText("open conversation")).not.toBeInTheDocument();
    expect(onOpenThread).not.toHaveBeenCalled();
  });

  it("explains missing provider credentials separately from the feature flag", () => {
    render(
      <TaskSidebar
        onClose={vi.fn()}
        accountId="you@example.com"
        currentThread={detail}
        onOpenThread={vi.fn()}
        title="Actions"
        analysis={threadAnalysis({ ready: false })}
      />,
    );
    expect(screen.getByText("Configure an AI provider and API key in AI settings to analyze this conversation.")).toBeInTheDocument();
    expect(screen.queryByText("Enable Thread actions in AI settings to analyze this conversation.")).not.toBeInTheDocument();
  });

  it("shows thread analysis controls only when analysis is supplied", () => {
    const { rerender } = render(
      <TaskSidebar onClose={vi.fn()} accountId="you@example.com" currentThread={detail} onOpenThread={vi.fn()} title="Actions" />,
    );
    expect(screen.queryByRole("button", { name: "Analyze thread" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Thread actions" })).not.toBeInTheDocument();

    const onAnalyze = vi.fn();
    rerender(
      <TaskSidebar onClose={vi.fn()} accountId="you@example.com" currentThread={detail} onOpenThread={vi.fn()} title="Actions" analysis={threadAnalysis({ onAnalyze })} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze thread" }));
    expect(onAnalyze).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("region", { name: "Thread actions" })).toBeInTheDocument();
  });
});
