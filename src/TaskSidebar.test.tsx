import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskSidebar } from "./TaskSidebar";
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

describe("TaskSidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("creates an email-linked task from the current conversation", async () => {
    const created: ThreadTask = {
      id: "task-1",
      accountId: "you@example.com",
      threadId: "thread-1",
      sourceMessageId: "message-1",
      subjectSnapshot: "Website setup",
      title: "Set up the website",
      notes: null,
      kind: "action",
      dueKind: "date",
      dueValue: "2026-09-25",
      timeZone: "America/New_York",
      repeatIntervalDays: null,
      status: "open",
      completionSource: null,
      evidenceText: "Please set up the website by Friday.",
      waitAfter: "2026-09-19T10:00:00Z",
      createdAt: "2026-09-19T10:01:00Z",
      updatedAt: "2026-09-19T10:01:00Z",
      completedAt: null,
    };
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const create = vi.spyOn(mailClient, "createTask").mockResolvedValue(created);
    render(<TaskSidebar onClose={vi.fn()} accountId="you@example.com" currentThread={detail} onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Open add task form" }));
    fireEvent.change(screen.getByPlaceholderText("What needs doing?"), { target: { value: "Set up the website" } });
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2026-09-25" } });
    fireEvent.click(screen.getByText("Add task").closest("button")!);

    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      threadId: "thread-1",
      sourceMessageId: "message-1",
      title: "Set up the website",
      dueKind: "date",
    })));
    expect(await screen.findByText("Set up the website")).toBeInTheDocument();
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
    const addTask = vi.fn().mockResolvedValue(undefined);
    const discard = vi.fn();
    render(
      <TaskSidebar
        onClose={vi.fn()}
        accountId="you@example.com"
        currentThread={detail}
        onOpenThread={vi.fn()}
        title="Actions"
        analysisEnabled
        analysisReady
        analysisPreview={'{"emailContext":{"messages":[]}}'}
        proposals={[proposal]}
        onAnalyzeThread={vi.fn()}
        onDiscardProposal={discard}
        onAddTaskProposal={addTask}
      />,
    );

    expect(await screen.findByText("Set up the website")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Evidence"));
    expect(screen.getByText("Please set up the website by Friday.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(addTask).toHaveBeenCalledWith(proposal));
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(discard).toHaveBeenCalledWith(0);
  });

  it("explains missing provider credentials separately from the feature flag", () => {
    render(
      <TaskSidebar
        onClose={vi.fn()}
        accountId="you@example.com"
        currentThread={detail}
        onOpenThread={vi.fn()}
        title="Actions"
        analysisEnabled
        analysisReady={false}
        onAnalyzeThread={vi.fn()}
      />,
    );
    expect(screen.getByText("Configure an AI provider and API key in AI settings to analyze this conversation.")).toBeInTheDocument();
    expect(screen.queryByText("Enable Thread actions in AI settings to analyze this conversation.")).not.toBeInTheDocument();
  });
});
