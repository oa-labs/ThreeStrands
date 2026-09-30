import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskSidebar, type TaskWorkspaceHandle } from "./TaskSidebar";
import { mailClient } from "./data/client";
import type { ThreadTask } from "./domain";

function workspaceTask(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: null, sourceMessageId: null, subjectSnapshot: null,
    title: id, notes: null, kind: "action", dueKind: "none", dueValue: null, timeZone: null,
    repeatIntervalDays: null, status: "open", completionSource: null, evidenceText: null, waitAfter: null,
    createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null,
    ...overrides,
  };
}

function localDate(offsetDays: number): string {
  const date = new Date();
  date.setDate(date.getDate() + offsetDays);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

describe("TaskSidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("stays read-only until Add task opens the quick-add form", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={vi.fn()} />);

    await screen.findByText("0 tasks");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    expect(screen.getByRole("textbox", { name: "Task title" })).toBeInTheDocument();
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
    localStorage.setItem("threestrands.tasks.layout", "list");
    render(<TaskSidebar accountId={null} onOpenThread={vi.fn()} />);

    const card = (await screen.findByText("Set up the website", { selector: "strong" })).closest("article")!;
    expect(await within(screen.getByRole("region", { name: "Task details" })).findByText("Task", { selector: ".eyebrow" })).toBeInTheDocument();
    fireEvent.click(within(card).getByRole("button", { name: "Complete Set up the website" }));
    await waitFor(() => expect(setStatus).toHaveBeenCalledWith("task-1", "completed"));
  });

  it("offers an undo toast and an aria-live announcement when completing a task", async () => {
    const task = workspaceTask("task-1", { title: "Set up the website" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus")
      .mockResolvedValueOnce({ ...task, status: "completed" })
      .mockResolvedValueOnce({ ...task, status: "open" });
    localStorage.setItem("threestrands.tasks.layout", "list");
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const card = (await screen.findByText("Set up the website", { selector: "strong" })).closest("article")!;
    fireEvent.click(within(card).getByRole("button", { name: "Complete Set up the website" }));
    const undo = await screen.findByRole("button", { name: "Undo" });
    expect(screen.getByText("Completed: Set up the website")).toBeInTheDocument();

    fireEvent.click(undo);
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("task-1", "open"));
    expect(screen.queryByRole("button", { name: "Undo" })).not.toBeInTheDocument();
    expect(screen.getByText("Reopened: Set up the website")).toBeInTheDocument();
  });

  it("groups an overdue task separately from Today in the list layout and colors its due label", async () => {
    const tasks = [
      workspaceTask("overdue", { dueKind: "date", dueValue: localDate(-14) }),
      workspaceTask("today", { dueKind: "date", dueValue: localDate(0) }),
    ];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    localStorage.setItem("threestrands.tasks.layout", "list");
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    await screen.findByRole("heading", { name: "Overdue" });
    const overdueSection = screen.getByRole("heading", { name: "Overdue" }).closest("section");
    const todaySection = screen.getByRole("heading", { name: "Today" }).closest("section");
    expect(overdueSection?.querySelector("#task-overdue")).not.toBeNull();
    expect(overdueSection?.querySelector("#task-today")).toBeNull();
    expect(todaySection?.querySelector("#task-today")).not.toBeNull();
    expect(container.querySelector("#task-overdue .task-due-overdue")).not.toBeNull();
    expect(container.querySelector("#task-today .task-due-overdue")).toBeNull();
  });

  it("labels due dates relative to today", async () => {
    const tasks = [
      workspaceTask("today", { dueKind: "date", dueValue: localDate(0) }),
      workspaceTask("tomorrow", { dueKind: "date", dueValue: localDate(1) }),
      workspaceTask("yesterday", { dueKind: "date", dueValue: localDate(-1) }),
    ];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    localStorage.setItem("threestrands.tasks.layout", "list");
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const todayCard = (await screen.findByText("today", { selector: "strong" })).closest("article")!;
    expect(within(todayCard).getByText("Today")).toBeInTheDocument();
    const tomorrowCard = screen.getByText("tomorrow", { selector: "strong" }).closest("article")!;
    expect(within(tomorrowCard).getByText("Tomorrow")).toBeInTheDocument();
    const yesterdayCard = screen.getByText("yesterday", { selector: "strong" }).closest("article")!;
    expect(within(yesterdayCard).getByText("Yesterday")).toBeInTheDocument();
  });

  it("lets the inline due editor set a custom timezone and rejects an invalid one", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      saved = { ...saved, ...request };
      return saved;
    });
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add a due date" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Due type" }), { target: { value: "datetime" } });
    fireEvent.change(screen.getByLabelText("Due date and time"), { target: { value: "2030-10-02T14:30" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Timezone" }), { target: { value: "America/New Yok" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Choose a valid timezone");
    expect(updateTask).not.toHaveBeenCalled();

    fireEvent.change(screen.getByRole("combobox", { name: "Timezone" }), { target: { value: "Asia/Tokyo" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({
      id: "plan", dueKind: "datetime", dueValue: new Date("2030-10-02T14:30").toISOString(), timeZone: "Asia/Tokyo",
    }));
  });

  it("preserves the entered date when switching the inline due editor between date and date-and-time", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("plan", { title: "Plan launch" })]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add a due date" }));
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2030-10-01" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Due type" }), { target: { value: "datetime" } });
    expect(screen.getByLabelText("Due date and time")).toHaveValue("2030-10-01T09:00");

    fireEvent.change(screen.getByRole("combobox", { name: "Due type" }), { target: { value: "date" } });
    expect(screen.getByLabelText("Due date")).toHaveValue("2030-10-01");
  });

  it("hides completed tasks older than a few days from the board's Done column", async () => {
    const stale = new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString();
    const recent = new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString();
    const tasks = [
      workspaceTask("stale", { status: "completed", completedAt: stale }),
      workspaceTask("recent", { status: "completed", completedAt: recent }),
    ];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    await screen.findByRole("heading", { name: "Done" });
    expect(await screen.findByText("recent", { selector: "strong" })).toBeInTheDocument();
    expect(screen.queryByText("stale", { selector: "strong" })).not.toBeInTheDocument();
  });

  it("shows the selected task visually in the list layout", async () => {
    const tasks = [workspaceTask("task-1", { title: "First" }), workspaceTask("task-2", { title: "Second" })];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    localStorage.setItem("threestrands.tasks.layout", "list");
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByText("Second", { selector: "strong" }));
    expect(container.querySelector("#task-task-2")).toHaveClass("selected");
    expect(container.querySelector("#task-task-2")).toHaveAttribute("aria-current", "true");
    expect(container.querySelector("#task-task-1")).not.toHaveClass("selected");
  });

  it("lets the user dismiss the error banner", async () => {
    vi.spyOn(mailClient, "listTasks").mockRejectedValue(new Error("Could not load tasks"));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load tasks");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss error" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps the quick-add row open after creating a task for batch entry", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const onCreateTask = vi.fn()
      .mockResolvedValueOnce(workspaceTask("task-1", { title: "First" }))
      .mockResolvedValueOnce(workspaceTask("task-2", { title: "Second" }));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={onCreateTask} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add task" }));
    const input = screen.getByRole("textbox", { name: "Task title" });
    fireEvent.change(input, { target: { value: "First" } });
    fireEvent.click(within(input.closest("form")!).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("First"));

    expect(screen.getByRole("textbox", { name: "Task title" })).toHaveValue("");
    fireEvent.change(screen.getByRole("textbox", { name: "Task title" }), { target: { value: "Second" } });
    fireEvent.click(within(screen.getByRole("textbox", { name: "Task title" }).closest("form")!).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("Second"));
  });

  it("requires an account for a task added from All accounts", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const onCreateTask = vi.fn().mockResolvedValue(workspaceTask("work-task", { accountId: "work@example.com" }));
    render(<TaskSidebar accountId={null} accountOptions={["you@example.com", "work@example.com"]} onOpenThread={vi.fn()} onCreateTask={onCreateTask} />);
    expect(screen.getByText("· All accounts")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Add task" }));
    const form = screen.getByRole("textbox", { name: "Task title" }).closest("form")!;
    fireEvent.change(within(form).getByRole("textbox", { name: "Task title" }), { target: { value: "Review proposal" } });
    expect(within(form).getByRole("button", { name: "Add task" })).toBeDisabled();
    expect(onCreateTask).not.toHaveBeenCalled();
    fireEvent.change(within(form).getByRole("combobox", { name: "Account" }), { target: { value: "work@example.com" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("Review proposal", "work@example.com"));
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
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onDraftFollowUp={draftFollowUp} />);

    fireEvent.click(await screen.findByRole("button", { name: "Draft Follow-Up" }));
    expect(draftFollowUp).toHaveBeenCalledWith(task);
  });

  it("shows the account email and a task count in the header, like the mail inbox header", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={vi.fn(async (title) => workspaceTask("created", { title }))} />);

    await waitFor(() => expect(screen.getByText("0 tasks")).toBeInTheDocument());
    expect(screen.getByText("you@example.com", { exact: false })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check schedule" })).not.toBeInTheDocument();
    const addTask = screen.getByRole("button", { name: "Add task" });
    expect(addTask).toHaveClass("task-add-button");
    expect(addTask).toHaveTextContent("Add task");
    expect(addTask.querySelector("svg")).not.toBeNull();
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("creates a task from its title first and opens it in the detail pane", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const created = workspaceTask("new-task", { title: "Call the contractor" });
    const onCreateTask = vi.fn().mockResolvedValue(created);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={onCreateTask} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add task" }));
    const form = screen.getByRole("textbox", { name: "Task title" }).closest("form");
    expect(form).not.toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Task title" }), { target: { value: "  Call the contractor  " } });
    fireEvent.click(within(form!).getByRole("button", { name: "Add task" }));

    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("Call the contractor"));
    expect(await screen.findByRole("heading", { name: "Call the contractor" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a description" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a due date" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("filters the task list by date, waiting status, and completion without repeating email subjects", async () => {
    const tasks = [
      workspaceTask("overdue", { dueKind: "date", dueValue: localDate(-1), threadId: "thread-1", subjectSnapshot: "Old email subject" }),
      workspaceTask("today", { dueKind: "date", dueValue: localDate(0) }),
      workspaceTask("upcoming", { dueKind: "date", dueValue: localDate(1) }),
      workspaceTask("anytime"),
      workspaceTask("waiting", { kind: "waiting_for" }),
      workspaceTask("completed", { status: "completed" }),
    ];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const views = await screen.findByRole("navigation", { name: "Task views" });
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    await waitFor(() => expect(container.querySelectorAll(".task-card")).toHaveLength(5));
    expect(container.querySelector(".tasks-list-pane")).not.toHaveTextContent("Old email subject");
    expect(screen.getByRole("button", { name: "All" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("heading", { name: "Overdue" })).toBeInTheDocument();

    for (const [name, expected] of [["Today", "today"], ["Upcoming", "upcoming"], ["Anytime", "anytime"], ["Waiting", "waiting"], ["Completed", "completed"]] as const) {
      fireEvent.click(within(views).getByRole("button", { name }));
      expect(container.querySelectorAll(".task-card")).toHaveLength(1);
      expect(container.querySelector(".task-card")).toHaveAttribute("id", `task-${expected}`);
      if (name === "Waiting") expect(within(screen.getByRole("region", { name: "Task details" })).getByText("Waiting for reply", { selector: ".eyebrow" })).toBeInTheDocument();
    }
    fireEvent.click(within(views).getByRole("button", { name: "Overdue" }));
    expect(container.querySelector(".task-card")).toHaveAttribute("id", "task-overdue");
  });

  it("opens the workspace as a three-column board by status and remembers the list layout", async () => {
    const tasks = [
      workspaceTask("todo", { title: "Draft agenda" }),
      workspaceTask("doing", { title: "Write proposal", status: "in_progress" }),
      workspaceTask("done", { title: "Book venue", status: "completed", completedAt: new Date().toISOString() }),
      workspaceTask("stale", { title: "Old errand", status: "completed", completedAt: "2020-01-01T00:00:00Z" }),
    ];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    const onLayoutChange = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onLayoutChange={onLayoutChange} />);

    const todo = await screen.findByRole("region", { name: "To Do" });
    const doing = screen.getByRole("region", { name: "In Progress" });
    const done = screen.getByRole("region", { name: "Done" });
    await waitFor(() => expect(todo).toHaveTextContent("Draft agenda"));
    expect(doing).toHaveTextContent("Write proposal");
    expect(done).toHaveTextContent("Book venue");
    expect(done).not.toHaveTextContent("Old errand");
    expect(screen.getByRole("button", { name: "Board" })).toHaveAttribute("aria-pressed", "true");
    expect(onLayoutChange).toHaveBeenLastCalledWith("board");
    expect(within(todo).queryByRole("button", { name: /Move Draft agenda to To Do/ })).not.toBeInTheDocument();
    expect(within(done).queryByRole("button", { name: /Move Book venue to Done/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "List" }));
    expect(onLayoutChange).toHaveBeenLastCalledWith("list");
    expect(screen.queryByRole("region", { name: "To Do" })).not.toBeInTheDocument();
    expect(screen.getByText("In progress")).toBeInTheDocument();
    expect(localStorage.getItem("threestrands.tasks.layout")).toBe("list");

    cleanup();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    expect(await screen.findByRole("button", { name: "List" })).toHaveAttribute("aria-pressed", "true");
  });

  it("resizes the board detail panel from the divider and persists the width", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("todo", { title: "Draft agenda" })]);
    const originalInnerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth");
    Object.defineProperty(window, "innerWidth", { value: 1600, configurable: true });
    try {
      const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
      await screen.findByRole("region", { name: "To Do" });

      const handle = screen.getByRole("separator", { name: "Resize task detail" });
      expect(handle).toHaveAttribute("aria-controls", "task-detail-panel");
      expect(handle).toHaveAttribute("aria-valuemax", "720");
      const body = container.querySelector(".tasks-workspace-body") as HTMLElement;
      const appliedWidth = () => body.style.getPropertyValue("--task-detail-width");
      expect(appliedWidth()).toBe("440px");

      // The detail panel sits to the right of the handle, so ArrowLeft widens it.
      fireEvent.keyDown(handle, { key: "ArrowLeft" });
      expect(appliedWidth()).toBe("450px");
      fireEvent.keyDown(handle, { key: "ArrowRight", shiftKey: true });
      expect(appliedWidth()).toBe("410px");
      await waitFor(() => expect(localStorage.getItem("threestrands.taskDetailWidth")).toBe("410"));

      cleanup();
      const remount = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
      expect(await screen.findByRole("region", { name: "To Do" })).toBeInTheDocument();
      const remountedBody = remount.container.querySelector(".tasks-workspace-body") as HTMLElement;
      expect(remountedBody.style.getPropertyValue("--task-detail-width")).toBe("410px");

      fireEvent.dblClick(screen.getByRole("separator", { name: "Resize task detail" }));
      expect(remountedBody.style.getPropertyValue("--task-detail-width")).toBe("440px");
      await waitFor(() => expect(localStorage.getItem("threestrands.taskDetailWidth")).toBe("440"));
    } finally {
      cleanup();
      if (originalInnerWidth) Object.defineProperty(window, "innerWidth", originalInnerWidth);
      localStorage.removeItem("threestrands.taskDetailWidth");
    }
  });

  it("moves cards between columns with buttons and keyboard handles, and undo restores the prior column", async () => {
    let current = workspaceTask("plan", { title: "Plan launch" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([current]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (_id, status) => {
      current = { ...current, status };
      return current;
    });
    const ref = createRef<TaskWorkspaceHandle>();
    render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Move Plan launch to In Progress" }));
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("plan", "in_progress"));
    expect(await within(screen.getByRole("region", { name: "In Progress" })).findByText("Plan launch")).toBeInTheDocument();
    expect(screen.getByText("Started: Plan launch")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Task details" })).toHaveTextContent("In progress");

    act(() => ref.current?.moveSelected(1));
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("plan", "completed"));
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("plan", "in_progress"));

    act(() => ref.current?.moveSelected(-1));
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("plan", "open"));
    expect(await screen.findByText("Moved to To Do: Plan launch")).toBeInTheDocument();
    const calls = setTaskStatus.mock.calls.length;
    act(() => ref.current?.moveSelected(-1));
    expect(setTaskStatus).toHaveBeenCalledTimes(calls);
  });

  it("jumps between non-empty board columns while keeping the row position", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      workspaceTask("todo-1"), workspaceTask("todo-2"),
      workspaceTask("done-1", { status: "completed", completedAt: new Date().toISOString() }),
    ]);
    const ref = createRef<TaskWorkspaceHandle>();
    const { container } = render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);
    const selected = () => container.querySelector(".task-card.selected")?.id;

    await waitFor(() => expect(selected()).toBe("task-todo-1"));
    act(() => ref.current?.selectNext());
    expect(selected()).toBe("task-todo-2");
    act(() => ref.current?.selectAdjacentColumn(1));
    expect(selected()).toBe("task-done-1");
    act(() => ref.current?.selectAdjacentColumn(1));
    expect(selected()).toBe("task-done-1");
    act(() => ref.current?.selectAdjacentColumn(-1));
    expect(selected()).toBe("task-todo-1");
    act(() => ref.current?.toggleLayout());
    expect(screen.getByRole("button", { name: "List" })).toHaveAttribute("aria-pressed", "true");
  });

  it("edits the task title, description, and due date in the detail pane", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch", notes: "Draft outline" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      saved = { ...saved, ...request };
      return saved;
    });
    const onTasksChanged = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onTasksChanged={onTasksChanged} />);

    fireEvent.click(await screen.findByRole("button", { name: "Edit title: Plan launch" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Task title" }), { target: { value: "Plan product launch" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", title: "Plan product launch" }));
    expect(await screen.findByRole("heading", { name: "Plan product launch" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Draft outline" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Prepare launch checklist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", notes: "Prepare launch checklist" }));
    expect(screen.getByRole("button", { name: "Prepare launch checklist" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Add a due date" }));
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2030-10-01" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", dueKind: "date", dueValue: "2030-10-01", timeZone: null }));
    expect(onTasksChanged).toHaveBeenCalledTimes(3);

    fireEvent.click(screen.getByRole("button", { name: "Oct 1, 2030" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear date" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", dueKind: "none", dueValue: null, timeZone: null }));

    fireEvent.click(screen.getByRole("button", { name: "Add a due date" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Due type" }), { target: { value: "datetime" } });
    fireEvent.change(screen.getByLabelText("Due date and time"), { target: { value: "2030-10-02T14:30" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({
      id: "plan", dueKind: "datetime", dueValue: new Date("2030-10-02T14:30").toISOString(),
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }));
  });

  it("keeps an inline edit open when saving fails", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("plan", { title: "Plan launch" })]);
    vi.spyOn(mailClient, "updateTask").mockRejectedValue(new Error("Could not save task"));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Edit title: Plan launch" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Task title" }), { target: { value: "Prepare launch" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save task");
    expect(screen.getByRole("textbox", { name: "Task title" })).toHaveValue("Prepare launch");
  });

  it("has no close control, like the contacts manager", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("0 tasks")).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Close Tasks" })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();
  });

  it("keeps the linked email below task details with its excerpt collapsed", async () => {
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
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} />);

    await screen.findByRole("heading", { name: "Set up the website" });
    const source = screen.getByRole("region", { name: "Source conversation" });
    expect(source.querySelector("details")).not.toHaveAttribute("open");
    expect(source).toHaveTextContent("Website setup");
    fireEvent.click(screen.getByText("Source excerpt"));
    expect(source).toHaveTextContent("Please set up the website by Friday.");
    fireEvent.click(screen.getByRole("button", { name: "Open conversation" }));
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
  });

  it("completes a task from its title and keeps other details editable", async () => {
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
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} onEditTask={onEditTask} />);

    const edit = await screen.findByRole("button", { name: "Task Options" });
    const done = within(screen.getByRole("region", { name: "Task details" })).getByRole("button", { name: "Complete Set up the website" });
    const open = screen.getByRole("button", { name: "Open conversation" });
    expect(edit).toHaveClass("action-button");
    expect(done.querySelector("svg")).not.toBeNull();
    const tooltips = screen.getAllByRole("tooltip", { hidden: true }).map((tooltip) => tooltip.textContent);
    expect(tooltips).toEqual(["Task options"]);

    fireEvent.click(edit);
    expect(onEditTask).toHaveBeenCalledWith(task);
    fireEvent.click(open);
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
    fireEvent.click(done);
    await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("task-1", "completed"));
    fireEvent.click(screen.getByRole("button", { name: "Completed" }));
    expect(await within(screen.getByRole("region", { name: "Task details" })).findByRole("button", { name: "Reopen Set up the website" })).toBeInTheDocument();
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
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} />);

    expect(await screen.findByRole("heading", { name: "Buy printer paper" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Source conversation" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open conversation" })).not.toBeInTheDocument();
    expect(onOpenThread).not.toHaveBeenCalled();
  });
});
