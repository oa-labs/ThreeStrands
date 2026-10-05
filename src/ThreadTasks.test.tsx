import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ThreadTasks } from "./ThreadTasks";
import { mailClient } from "./data/client";
import type { Thread, ThreadTask } from "./domain";

const thread = { id: "thread-1", accountId: "you@example.com" } as Thread;

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: "thread-1", sourceMessageId: null, subjectSnapshot: "Website setup",
    title: `Task ${id}`, notes: null, kind: "action", dueKind: "none", dueValue: null, timeZone: null,
    repeatIntervalDays: null, status: "open", completionSource: null, evidenceText: null, waitAfter: null,
    createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null, ...overrides,
  };
}

function renderTasks(overrides: Partial<Parameters<typeof ThreadTasks>[0]> = {}) {
  const handlers = { onAddTask: vi.fn(), onEditTask: vi.fn(), onDraftFollowUp: vi.fn(), onTasksChanged: vi.fn() };
  const view = render(<ThreadTasks thread={thread} refreshKey={0} {...handlers} {...overrides} />);
  return { ...view, ...handlers };
}

describe("ThreadTasks", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("names its region apart from the Tasks workspace", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine")]);
    renderTasks();
    expect(await screen.findByRole("region", { name: "Conversation tasks" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
  });

  it("leaves the section out while there are no tasks and shows it once one is added", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const { rerender, onAddTask, onEditTask, onDraftFollowUp, onTasksChanged } = renderTasks();
    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    expect(screen.queryByRole("region", { name: "Conversation tasks" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add task" })).not.toBeInTheDocument();

    listTasks.mockResolvedValue([task("added")]);
    rerender(<ThreadTasks thread={thread} refreshKey={1} onAddTask={onAddTask} onEditTask={onEditTask} onDraftFollowUp={onDraftFollowUp} onTasksChanged={onTasksChanged} />);
    const section = await screen.findByRole("region", { name: "Conversation tasks" });
    expect(section.querySelector(".context-section-header-actions .context-count")).toHaveTextContent("1");
    expect(screen.getByRole("button", { name: "Add task" })).toBeInTheDocument();
  });

  it("lists open tasks linked to this conversation and those marked done today", async () => {
    const today = new Date();
    const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1, 12).toISOString();
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      task("mine"),
      task("other-thread", { threadId: "thread-2" }),
      task("standalone", { threadId: null }),
      task("done-today", { status: "completed", completedAt: today.toISOString() }),
      task("done-yesterday", { status: "completed", completedAt: yesterday }),
      task("cancelled", { status: "cancelled", completedAt: today.toISOString() }),
      task("started", { status: "in_progress" }),
    ]);
    renderTasks();

    const section = await screen.findByRole("region", { name: "Conversation tasks" });
    expect(screen.getByText("Task mine")).toBeInTheDocument();
    expect(screen.getByText("Task started")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Mark Task done-today not done" })).toBeChecked();
    expect(screen.queryByText("Task other-thread")).not.toBeInTheDocument();
    expect(screen.queryByText("Task standalone")).not.toBeInTheDocument();
    expect(screen.queryByText("Task done-yesterday")).not.toBeInTheDocument();
    expect(screen.queryByText("Task cancelled")).not.toBeInTheDocument();
    expect(section.querySelector(".context-section-header-actions .context-count")).toHaveTextContent("2");
    expect(listTasks).toHaveBeenCalledWith("you@example.com");
  });

  it("marks a task done with a labelled checkbox and keeps it listed, checked", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine")]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus")
      .mockResolvedValue(task("mine", { status: "completed", completedAt: new Date().toISOString() }));
    const { onTasksChanged } = renderTasks();

    const checkbox = await screen.findByRole("checkbox", { name: "Mark Task mine done" });
    expect(checkbox).not.toBeChecked();
    expect(checkbox.closest(".tooltip-anchor")?.querySelector("[role=tooltip]")).toHaveTextContent("Mark done");
    fireEvent.click(checkbox);

    const checked = await screen.findByRole("checkbox", { name: "Mark Task mine not done" });
    expect(checked).toBeChecked();
    expect(screen.getByText("Task mine").closest("article")).toHaveClass("context-task-done");
    expect(checked.closest(".tooltip-anchor")?.querySelector("[role=tooltip]")).toHaveTextContent("Mark not done");
    expect(setTaskStatus).toHaveBeenCalledWith("mine", "completed");
    expect(onTasksChanged).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Completed: Task mine")).toHaveAttribute("aria-live", "polite");
  });

  it("unchecks a task done today to reopen it", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine", { status: "completed", completedAt: new Date().toISOString() })]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue(task("mine"));
    const { onTasksChanged } = renderTasks();

    fireEvent.click(await screen.findByRole("checkbox", { name: "Mark Task mine not done" }));

    expect(await screen.findByRole("checkbox", { name: "Mark Task mine done" })).not.toBeChecked();
    expect(setTaskStatus).toHaveBeenCalledWith("mine", "open");
    expect(onTasksChanged).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Reopened: Task mine")).toBeInTheDocument();
  });

  it("keeps the checkbox state and shows the error when marking done fails", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine")]);
    vi.spyOn(mailClient, "setTaskStatus").mockRejectedValue(new Error("Database is locked"));
    const { onTasksChanged } = renderTasks();

    fireEvent.click(await screen.findByRole("checkbox", { name: "Mark Task mine done" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Database is locked");
    expect(screen.getByRole("checkbox", { name: "Mark Task mine done" })).not.toBeChecked();
    expect(onTasksChanged).not.toHaveBeenCalled();
  });

  it("opens editing, adding, and due follow-up drafting through the supplied handlers", async () => {
    const followUp = task("follow", { kind: "follow_up", dueKind: "date", dueValue: "2020-01-01" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine"), followUp]);
    const { onAddTask, onEditTask, onDraftFollowUp } = renderTasks();

    fireEvent.click(await screen.findByRole("button", { name: "Task mine" }));
    expect(onEditTask).toHaveBeenCalledWith(expect.objectContaining({ id: "mine" }));
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    expect(onAddTask).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Draft Follow-Up" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Draft Follow-Up" }));
    expect(onDraftFollowUp).toHaveBeenCalledWith(followUp);
  });

  it("adds open tasks from the person's other conversations, labelled with their subject", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine")]);
    const listContactTasks = vi.spyOn(mailClient, "listContactTasks").mockResolvedValue([
      task("mine"),
      task("elsewhere", { threadId: "thread-2", subjectSnapshot: "Budget review" }),
      task("elsewhere-done", { threadId: "thread-3", status: "completed" }),
    ]);
    renderTasks({ contactId: "contact:jane" });

    expect(await screen.findByText("Task elsewhere")).toBeInTheDocument();
    expect(screen.getByText("Budget review")).toBeInTheDocument();
    expect(screen.getAllByText("Task mine")).toHaveLength(1);
    expect(screen.queryByText("Task elsewhere-done")).not.toBeInTheDocument();
    expect(screen.queryByText("Website setup")).not.toBeInTheDocument();
    expect(listContactTasks).toHaveBeenCalledWith("contact:jane");
  });

  it("does not look up person tasks before the person is known", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const listContactTasks = vi.spyOn(mailClient, "listContactTasks");
    renderTasks({ contactId: null });
    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    expect(listContactTasks).not.toHaveBeenCalled();
  });

  it("reloads when the task revision changes", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValueOnce([]).mockResolvedValueOnce([task("added")]);
    const { rerender, onAddTask, onEditTask, onDraftFollowUp, onTasksChanged } = renderTasks();
    await waitFor(() => expect(listTasks).toHaveBeenCalledTimes(1));
    rerender(<ThreadTasks thread={thread} refreshKey={1} onAddTask={onAddTask} onEditTask={onEditTask} onDraftFollowUp={onDraftFollowUp} onTasksChanged={onTasksChanged} />);
    expect(await screen.findByText("Task added")).toBeInTheDocument();
  });

  it("shows a load failure without hiding the add control", async () => {
    vi.spyOn(mailClient, "listTasks").mockRejectedValue(new Error("Database is locked"));
    renderTasks();
    expect(await screen.findByRole("alert")).toHaveTextContent("Database is locked");
    expect(screen.getByRole("button", { name: "Add task" })).toBeInTheDocument();
  });
});
