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
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    renderTasks();
    expect(screen.getByRole("region", { name: "Conversation tasks" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
    await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
  });

  it("lists only open tasks linked to this conversation", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      task("mine"),
      task("other-thread", { threadId: "thread-2" }),
      task("standalone", { threadId: null }),
      task("done", { status: "completed", completedAt: "2026-09-19T11:00:00Z" }),
      task("started", { status: "in_progress" }),
    ]);
    renderTasks();

    expect(await screen.findByText("Task mine")).toBeInTheDocument();
    expect(screen.getByText("Task started")).toBeInTheDocument();
    expect(screen.queryByText("Task other-thread")).not.toBeInTheDocument();
    expect(screen.queryByText("Task standalone")).not.toBeInTheDocument();
    expect(screen.queryByText("Task done")).not.toBeInTheDocument();
    expect(listTasks).toHaveBeenCalledWith("you@example.com");
  });

  it("completes a task in place and reports the change", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("mine")]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue(task("mine", { status: "completed" }));
    const { onTasksChanged } = renderTasks();

    fireEvent.click(await screen.findByRole("button", { name: "Complete Task mine" }));

    await waitFor(() => expect(screen.queryByText("Task mine")).not.toBeInTheDocument());
    expect(setTaskStatus).toHaveBeenCalledWith("mine", "completed");
    expect(onTasksChanged).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Completed: Task mine")).toHaveAttribute("aria-live", "polite");
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
