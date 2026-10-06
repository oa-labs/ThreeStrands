import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskSidebar, type TaskWorkspaceHandle } from "./TaskSidebar";
import { mailClient } from "./data/client";
import { expectSharedButtons } from "./test/sharedButtons";
import type { Goal, ThreadTask } from "./domain";
import { formatPeriod, periodFor, shiftPeriod } from "./goals";

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

async function openTask(title: string) {
  fireEvent.click(await screen.findByText(title, { selector: "strong" }));
  return within(await screen.findByRole("dialog", { name: "Task details" }));
}

describe("TaskSidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("stays read-only until Add Task opens the quick-add form", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={vi.fn()} />);

    await screen.findByText("0 tasks");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
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

  it("saves a due date and timezone from the dialog and holds back an invalid timezone", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      saved = { ...saved, ...request };
      return saved;
    });
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Plan launch");
    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "datetime" } });
    fireEvent.change(dialog.getByLabelText("Due date and time"), { target: { value: "2030-10-02T14:30" } });
    fireEvent.change(dialog.getByRole("combobox", { name: "Timezone" }), { target: { value: "America/New Yok" } });
    fireEvent.blur(dialog.getByRole("combobox", { name: "Timezone" }));

    expect(await dialog.findByRole("alert")).toHaveTextContent("Choose a valid timezone");
    expect(updateTask).not.toHaveBeenCalled();

    fireEvent.change(dialog.getByRole("combobox", { name: "Timezone" }), { target: { value: "Asia/Tokyo" } });
    fireEvent.blur(dialog.getByRole("combobox", { name: "Timezone" }));
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({
      id: "plan", dueKind: "datetime", dueValue: new Date("2030-10-02T14:30").toISOString(), timeZone: "Asia/Tokyo",
    }));
    expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("preserves the entered date when switching the dialog's due type between date and date-and-time", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("plan", { title: "Plan launch" })]);
    vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => ({ ...workspaceTask("plan", { title: "Plan launch" }), ...request }));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Plan launch");
    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    fireEvent.change(dialog.getByLabelText("Due date"), { target: { value: "2030-10-01" } });
    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "datetime" } });
    expect(dialog.getByLabelText("Due date and time")).toHaveValue("2030-10-01T09:00");

    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    expect(dialog.getByLabelText("Due date")).toHaveValue("2030-10-01");
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

    const done = screen.getByRole("region", { name: "Done" });
    const showOlder = within(done).getByRole("button", { name: "Show 1 older completed" });
    expect(showOlder).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(showOlder);
    expect(within(done).getByText("stale", { selector: "strong" })).toBeInTheDocument();
    fireEvent.click(within(done).getByRole("button", { name: "Hide older completed" }));
    expect(within(done).queryByText("stale", { selector: "strong" })).not.toBeInTheDocument();
  });

  it("explains an empty Done column when only older completed tasks exist", async () => {
    const stale = new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString();
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("stale", { status: "completed", completedAt: stale })]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const done = await screen.findByRole("region", { name: "Done" });
    expect(await within(done).findByRole("button", { name: "Show 1 older completed" })).toBeInTheDocument();
    expect(done).not.toHaveTextContent("No tasks");
    expect(screen.getByRole("region", { name: "To Do" })).toHaveTextContent("No tasks");
  });

  it("shows the full title, task kind, and email source on board cards without repeating the subject", async () => {
    const longTitle = "Email Crunchtime Sync Errors to management before the Thursday review";
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      workspaceTask("follow", { title: longTitle, kind: "follow_up", threadId: "thread-1", subjectSnapshot: "Re: Data quality" }),
      workspaceTask("wait", { title: "Hear back from venue", kind: "waiting_for" }),
      workspaceTask("plain", { title: "Standalone errand" }),
    ]);
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const follow = await waitFor(() => {
      const card = container.querySelector("#task-follow") as HTMLElement;
      expect(card).not.toBeNull();
      return card;
    });
    expect(within(follow).getByText(longTitle, { selector: "strong" })).toBeInTheDocument();
    expect(within(follow).getByText("Follow up")).toBeInTheDocument();
    expect(within(follow).getByText("From an email")).toBeInTheDocument();
    expect(container.querySelector(".task-board")).not.toHaveTextContent("Re: Data quality");
    expect(container.querySelector("#task-wait")).toHaveTextContent("Waiting");
    expect(container.querySelector("#task-wait")).not.toHaveTextContent("From an email");
    expect(container.querySelector("#task-plain .task-card-meta")).toBeNull();
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

    fireEvent.click(await screen.findByRole("button", { name: "Add Task" }));
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
    fireEvent.click(await screen.findByRole("button", { name: "Add Task" }));
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
    const addTask = screen.getByRole("button", { name: "Add Task" });
    expect(addTask).toHaveClass("btn", "task-add-button");
    expect(addTask).toHaveTextContent("Add Task");
    expect(addTask.querySelector("svg")).not.toBeNull();
    expectSharedButtons(screen.getByRole("region", { name: "Tasks" }).querySelector(".tasks-sidebar-header-actions")!);
    expect(screen.getByRole("group", { name: "Task layout" })).toHaveClass("segmented");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("creates a task from its title and selects it without opening the dialog, so batch entry continues", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const created = workspaceTask("new-task", { title: "Call the contractor" });
    const onCreateTask = vi.fn().mockResolvedValue(created);
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onCreateTask={onCreateTask} />);

    fireEvent.click(await screen.findByRole("button", { name: "Add Task" }));
    const form = screen.getByRole("textbox", { name: "Task title" }).closest("form");
    expect(form).not.toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Task title" }), { target: { value: "  Call the contractor  " } });
    fireEvent.click(within(form!).getByRole("button", { name: "Add task" }));

    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("Call the contractor"));
    await waitFor(() => expect(container.querySelector("#task-new-task")).toHaveAttribute("aria-current", "true"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task title" })).toHaveFocus();
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
      if (name === "Waiting") expect(container.querySelector(".task-card .task-card-kind")).toHaveTextContent("Waiting");
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

  it("gives the board the full workspace and opens task details in a dialog instead of a side pane", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("todo", { title: "Draft agenda" })]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    await screen.findByRole("region", { name: "To Do" });
    expect(screen.queryByRole("region", { name: "Task details" })).not.toBeInTheDocument();
    expect(screen.queryByRole("separator", { name: "Resize task detail" })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    const dialog = await openTask("Draft agenda");
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Draft agenda");
    expect(screen.getByRole("dialog", { name: "Task details" })).toHaveAttribute("aria-modal", "true");
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

    const start = await screen.findByRole("button", { name: "Move Plan launch to In Progress" });
    expect(start).toHaveTextContent("In Progress");
    fireEvent.click(start);
    await waitFor(() => expect(setTaskStatus).toHaveBeenLastCalledWith("plan", "in_progress"));
    expect(await within(screen.getByRole("region", { name: "In Progress" })).findByText("Plan launch")).toBeInTheDocument();
    expect(screen.getByText("Started: Plan launch")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move Plan launch to To Do" })).toHaveTextContent("To Do");
    expect(screen.getByRole("button", { name: "Move Plan launch to Done" })).toHaveTextContent("Done");

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
    // Up and down stay inside the column and stop at its ends instead of wrapping into the next column.
    act(() => ref.current?.selectNext());
    expect(selected()).toBe("task-todo-2");
    act(() => ref.current?.selectPrevious());
    act(() => ref.current?.selectPrevious());
    expect(selected()).toBe("task-todo-1");
    act(() => ref.current?.selectNext());
    act(() => ref.current?.selectAdjacentColumn(1));
    expect(selected()).toBe("task-done-1");
    act(() => ref.current?.selectAdjacentColumn(1));
    expect(selected()).toBe("task-done-1");
    act(() => ref.current?.selectAdjacentColumn(-1));
    expect(selected()).toBe("task-todo-1");
    act(() => ref.current?.toggleLayout());
    expect(screen.getByRole("button", { name: "List" })).toHaveAttribute("aria-pressed", "true");
  });

  it("cycles task views in both directions, wrapping and skipping Completed on the board, and restores the last view", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("todo-1")]);
    const ref = createRef<TaskWorkspaceHandle>();
    const { unmount } = render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);
    const views = () => within(screen.getByRole("navigation", { name: "Task views" }));
    const active = () => views().getAllByRole("button").find((button) => button.getAttribute("aria-pressed") === "true")?.textContent;

    await screen.findByText("todo-1");
    expect(active()).toBe("All1");
    act(() => ref.current?.cycleView(1));
    expect(views().getByRole("button", { name: "Overdue" })).toHaveAttribute("aria-pressed", "true");
    act(() => ref.current?.cycleView(-1));
    act(() => ref.current?.cycleView(-1));
    // The board has no Completed view, so cycling back from All wraps to Waiting.
    expect(views().getByRole("button", { name: "Waiting" })).toHaveAttribute("aria-pressed", "true");
    act(() => ref.current?.cycleView(1));
    expect(active()).toBe("All1");
    act(() => ref.current?.toggleLayout());
    act(() => ref.current?.cycleView(-1));
    expect(views().getByRole("button", { name: "Completed" })).toHaveAttribute("aria-pressed", "true");
    act(() => ref.current?.cycleView(-1));
    expect(views().getByRole("button", { name: "Waiting" })).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("threestrands.tasks.view")).toBe("Waiting");
    unmount();

    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    await waitFor(() => expect(views().getByRole("button", { name: "Waiting" })).toHaveAttribute("aria-pressed", "true"));
  });

  it("falls back to All when the stored task view is unknown", async () => {
    localStorage.setItem("threestrands.tasks.view", "Someday");
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    await screen.findByText("0 tasks");
    expect(within(screen.getByRole("navigation", { name: "Task views" })).getByRole("button", { name: "All" })).toHaveAttribute("aria-pressed", "true");
  });

  it("drops the Completed view and the Done column on the board when a date filter narrows open work", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      workspaceTask("overdue", { title: "Pay invoice", dueKind: "date", dueValue: localDate(-1) }),
      workspaceTask("doing", { title: "Draft memo", status: "in_progress" }),
      workspaceTask("done", { title: "Book venue", status: "completed", completedAt: new Date().toISOString() }),
    ]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const views = await screen.findByRole("navigation", { name: "Task views" });
    await screen.findByText("Book venue", { selector: "strong" });
    expect(within(views).queryByRole("button", { name: /Completed/ })).not.toBeInTheDocument();
    expect(within(views).getByRole("button", { name: "Overdue" })).toHaveTextContent("Overdue1");
    expect(within(views).getByRole("button", { name: "Today" })).toHaveTextContent(/^Today$/);
    expect(screen.getByRole("region", { name: "Done" })).toBeInTheDocument();

    fireEvent.click(within(views).getByRole("button", { name: "Overdue" }));
    expect(screen.queryByRole("region", { name: "Done" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "To Do" })).toHaveTextContent("Pay invoice");
    expect(screen.getByRole("region", { name: "In Progress" })).toHaveTextContent("No tasks");

    fireEvent.click(screen.getByRole("button", { name: "List" }));
    fireEvent.click(within(screen.getByRole("navigation", { name: "Task views" })).getByRole("button", { name: "Completed" }));
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    expect(within(screen.getByRole("navigation", { name: "Task views" })).getByRole("button", { name: "All" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "Done" })).toHaveTextContent("Book venue");
  });

  it("orders board cards by soonest due with overdue first and marks overdue and cancelled cards", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      workspaceTask("undated", { title: "Undated" }),
      workspaceTask("later", { title: "Later", dueKind: "date", dueValue: localDate(5) }),
      workspaceTask("overdue", { title: "Overdue", dueKind: "date", dueValue: localDate(-2) }),
      workspaceTask("cancelled", { title: "Dropped", status: "cancelled", updatedAt: new Date().toISOString() }),
    ]);
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const todo = await screen.findByRole("region", { name: "To Do" });
    await waitFor(() => expect(within(todo).getAllByRole("article")).toHaveLength(3));
    expect(within(todo).getAllByRole("article").map((card) => card.id)).toEqual(["task-overdue", "task-later", "task-undated"]);
    expect(container.querySelector("#task-overdue")).toHaveClass("task-card-overdue");
    expect(container.querySelector("#task-later")).not.toHaveClass("task-card-overdue");
    expect(container.querySelector("#task-cancelled")).toHaveTextContent("Cancelled");
  });

  describe("dragging board cards", () => {
    const originalElementFromPoint = document.elementFromPoint;
    afterEach(() => {
      document.elementFromPoint = originalElementFromPoint;
    });

    async function renderDraggableBoard() {
      let current = workspaceTask("plan", { title: "Plan launch" });
      vi.spyOn(mailClient, "listTasks").mockResolvedValue([current, workspaceTask("other", { title: "Other", status: "in_progress" })]);
      const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (_id, status) => {
        current = { ...current, status, completedAt: status === "completed" ? new Date().toISOString() : null };
        return current;
      });
      const view = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
      const card = await screen.findByText("Plan launch", { selector: "strong" });
      const main = card.closest("button") as HTMLButtonElement;
      main.setPointerCapture = vi.fn();
      main.releasePointerCapture = vi.fn();
      const point = (column: string) => {
        document.elementFromPoint = vi.fn(() => screen.getByRole("region", { name: column }));
      };
      return { ...view, main, point, setTaskStatus };
    }

    it("moves a card to the column it is dropped on and highlights the target while dragging", async () => {
      const { container, main, point, setTaskStatus } = await renderDraggableBoard();

      point("Done");
      fireEvent.pointerDown(main, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
      fireEvent.pointerMove(main, { pointerId: 1, clientX: 400, clientY: 20 });
      expect(screen.getByRole("region", { name: "Done" })).toHaveClass("drop-target");
      expect(container.querySelector("#task-plan")).toHaveClass("dragging");
      expect(container.querySelector(".task-drag-preview")).toHaveTextContent("Plan launch");

      fireEvent.pointerUp(main, { pointerId: 1, clientX: 400, clientY: 20 });
      fireEvent.click(main);
      await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("plan", "completed"));
      expect(container.querySelector(".task-drag-preview")).toBeNull();
      expect(await within(screen.getByRole("region", { name: "Done" })).findByText("Plan launch")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Undo" })).toBeInTheDocument();
    });

    it("treats a short press as a click and ignores drops on the card's own column or after Escape", async () => {
      const { container, main, point, setTaskStatus } = await renderDraggableBoard();
      fireEvent.click(screen.getByText("Other", { selector: "strong" }));
      expect(container.querySelector("#task-other")).toHaveClass("selected");
      expect(screen.getByRole("dialog", { name: "Task details" })).toBeInTheDocument();
      fireEvent.keyDown(window, { key: "Escape" });

      point("Done");
      fireEvent.pointerDown(main, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
      fireEvent.pointerMove(main, { pointerId: 1, clientX: 12, clientY: 11 });
      expect(container.querySelector(".task-drag-preview")).toBeNull();
      fireEvent.pointerUp(main, { pointerId: 1, clientX: 12, clientY: 11 });
      fireEvent.click(main);
      expect(container.querySelector("#task-plan")).toHaveClass("selected");
      expect(within(screen.getByRole("dialog", { name: "Task details" })).getByRole("textbox", { name: "Title" })).toHaveValue("Plan launch");
      fireEvent.keyDown(window, { key: "Escape" });

      point("To Do");
      fireEvent.pointerDown(main, { button: 0, pointerId: 2, clientX: 10, clientY: 10 });
      fireEvent.pointerMove(main, { pointerId: 2, clientX: 10, clientY: 80 });
      expect(screen.getByRole("region", { name: "To Do" })).not.toHaveClass("drop-target");
      fireEvent.pointerUp(main, { pointerId: 2, clientX: 10, clientY: 80 });

      point("In Progress");
      fireEvent.pointerDown(main, { button: 0, pointerId: 3, clientX: 10, clientY: 10 });
      fireEvent.pointerMove(main, { pointerId: 3, clientX: 300, clientY: 10 });
      fireEvent.keyDown(window, { key: "Escape" });
      expect(container.querySelector(".task-drag-preview")).toBeNull();
      fireEvent.pointerUp(main, { pointerId: 3, clientX: 300, clientY: 10 });
      expect(setTaskStatus).not.toHaveBeenCalled();
    });
  });

  it("steps between tasks with j and k outside a field, saving the edit in progress first", async () => {
    let tasks = [workspaceTask("task-1", { title: "First" }), workspaceTask("task-2", { title: "Second" }), workspaceTask("task-3", { title: "Third" })];
    vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      tasks = tasks.map((task) => task.id === request.id ? { ...task, ...request } : task);
      return tasks.find((task) => task.id === request.id)!;
    });
    localStorage.setItem("threestrands.tasks.layout", "list");
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("First");
    const body = screen.getByRole("dialog", { name: "Task details" }).querySelector(".task-detail-form") as HTMLElement;
    expect(body).toHaveFocus();
    const position = screen.getByRole("dialog", { name: "Task details" }).querySelector(".task-detail-hints")!;
    expect(position).toHaveTextContent("1 of 3");

    // Letters typed into a field are text, not navigation.
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Title" }), { key: "j" });
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("First");

    fireEvent.change(dialog.getByRole("textbox", { name: "Description" }), { target: { value: "Notes for first" } });
    fireEvent.keyDown(body, { key: "j" });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "task-1", notes: "Notes for first" }));
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Second");
    expect(dialog.getByRole("textbox", { name: "Description" })).toHaveValue("");
    expect(container.querySelector("#task-task-2")).toHaveAttribute("aria-current", "true");
    expect(position).toHaveTextContent("2 of 3");

    fireEvent.keyDown(body, { key: "ArrowDown" });
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Third");
    fireEvent.keyDown(body, { key: "k" });
    fireEvent.keyDown(body, { key: "ArrowUp" });
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("First");
    expect(dialog.getByRole("textbox", { name: "Description" })).toHaveValue("Notes for first");
    expect(updateTask).toHaveBeenCalledTimes(1);
  });

  it("saves each field as the user leaves it, without a Save button", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch", notes: "Draft outline" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      saved = { ...saved, ...request };
      return saved;
    });
    const onTasksChanged = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onTasksChanged={onTasksChanged} />);

    const dialog = await openTask("Plan launch");
    expect(dialog.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    const title = dialog.getByRole("textbox", { name: "Title" });
    fireEvent.change(title, { target: { value: "Plan product launch" } });
    fireEvent.blur(title);
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", title: "Plan product launch" }));
    expect(await screen.findByText("Plan product launch", { selector: "strong" })).toBeInTheDocument();

    const description = dialog.getByRole("textbox", { name: "Description" });
    fireEvent.change(description, { target: { value: "Prepare launch checklist" } });
    fireEvent.blur(description);
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", notes: "Prepare launch checklist" }));

    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    expect(updateTask).toHaveBeenCalledTimes(2);
    fireEvent.change(dialog.getByLabelText("Due date"), { target: { value: "2030-10-01" } });
    // Enter in an input saves it in place.
    fireEvent.keyDown(dialog.getByLabelText("Due date"), { key: "Enter" });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", dueKind: "date", dueValue: "2030-10-01", timeZone: null }));

    fireEvent.change(dialog.getByRole("combobox", { name: "Due" }), { target: { value: "none" } });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", dueKind: "none", dueValue: null, timeZone: null }));

    fireEvent.change(dialog.getByRole("combobox", { name: "Type" }), { target: { value: "follow_up" } });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", kind: "follow_up" }));
    const repeat = dialog.getByRole("spinbutton", { name: "Repeat every (days)" });
    fireEvent.change(repeat, { target: { value: "0" } });
    fireEvent.blur(repeat);
    expect(await dialog.findByRole("alert")).toHaveTextContent("Repeat every 1 to 365 days.");
    fireEvent.change(repeat, { target: { value: "7" } });
    fireEvent.blur(repeat);
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", repeatIntervalDays: 7 }));
    expect(onTasksChanged).toHaveBeenCalledTimes(6);

    // An emptied title is never saved; the last good title stays.
    fireEvent.change(title, { target: { value: "  " } });
    fireEvent.blur(title);
    expect(await dialog.findByRole("alert")).toHaveTextContent("A task needs a title.");
    expect(updateTask).toHaveBeenCalledTimes(6);
  });

  it("saves the field being edited when Escape or Cmd+Enter closes the dialog", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      saved = { ...saved, ...request };
      return saved;
    });
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    let dialog = await openTask("Plan launch");
    const description = dialog.getByRole("textbox", { name: "Description" });
    description.focus();
    fireEvent.change(description, { target: { value: "Typed then escaped" } });
    fireEvent.keyDown(description, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", notes: "Typed then escaped" }));

    dialog = await openTask("Plan launch");
    expect(dialog.getByRole("textbox", { name: "Description" })).toHaveValue("Typed then escaped");
    fireEvent.change(dialog.getByRole("textbox", { name: "Title" }), { target: { value: "Plan the launch" } });
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Title" }), { key: "Enter", metaKey: true });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", title: "Plan the launch" }));

    // Closing with nothing changed saves nothing.
    await openTask("Plan the launch");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(updateTask).toHaveBeenCalledTimes(2);
  });

  it("sends saves one at a time so a slow earlier save cannot overwrite a later one", async () => {
    let saved = workspaceTask("plan", { title: "Plan launch" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([saved]);
    const pending: Array<() => void> = [];
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation((request) => new Promise((resolve) => {
      pending.push(() => {
        saved = { ...saved, ...request };
        resolve(saved);
      });
    }));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Plan launch");
    fireEvent.change(dialog.getByRole("textbox", { name: "Title" }), { target: { value: "Plan launch v2" } });
    fireEvent.blur(dialog.getByRole("textbox", { name: "Title" }));
    fireEvent.change(dialog.getByRole("combobox", { name: "Type" }), { target: { value: "waiting_for" } });
    await waitFor(() => expect(updateTask).toHaveBeenCalledTimes(1));
    // The type change waits for the title save to finish before it is sent.
    await act(async () => { await Promise.resolve(); });
    expect(updateTask).toHaveBeenCalledTimes(1);
    await act(async () => pending[0]());
    await waitFor(() => expect(updateTask).toHaveBeenCalledTimes(2));
    expect(updateTask).toHaveBeenLastCalledWith(expect.objectContaining({ id: "plan", kind: "waiting_for" }));
    await act(async () => pending[1]());
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Plan launch v2");
    expect(dialog.getByRole("combobox", { name: "Type" })).toHaveValue("waiting_for");
  });

  it("keeps the typed value and reports the error when a save fails, then shows it in the banner once closed", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("plan", { title: "Plan launch" })]);
    vi.spyOn(mailClient, "updateTask").mockRejectedValue(new Error("Could not save task"));
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Plan launch");
    fireEvent.change(dialog.getByRole("textbox", { name: "Title" }), { target: { value: "Prepare launch" } });
    fireEvent.blur(dialog.getByRole("textbox", { name: "Title" }));

    expect(await dialog.findByRole("alert")).toHaveTextContent("Could not save task");
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Prepare launch");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save task");
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
    const task = workspaceTask("task-1", {
      threadId: "thread-1", sourceMessageId: "message-1", subjectSnapshot: "Website setup",
      title: "Set up the website", evidenceText: "Please set up the website by Friday.",
    });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const onOpenThread = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} />);

    const dialog = await openTask("Set up the website");
    const source = dialog.getByRole("region", { name: "Source conversation" });
    expect(source.querySelector("details")).not.toHaveAttribute("open");
    expect(source).toHaveTextContent("Website setup");
    fireEvent.click(dialog.getByText("Source excerpt"));
    expect(source).toHaveTextContent("Please set up the website by Friday.");
    fireEvent.click(dialog.getByRole("button", { name: "Open conversation" }));
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens the source conversation with o from outside a field", async () => {
    const task = workspaceTask("task-1", { threadId: "thread-1", subjectSnapshot: "Website setup", title: "Set up the website" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const onOpenThread = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} />);

    const dialog = await openTask("Set up the website");
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Description" }), { key: "o" });
    expect(onOpenThread).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Task details" }).querySelector(".task-detail-form")!, { key: "o" });
    expect(onOpenThread).toHaveBeenCalledWith("thread-1");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("changes status from the dialog and keeps the dialog on the task after it moves to Done", async () => {
    let task = workspaceTask("task-1", { threadId: "thread-1", subjectSnapshot: "Website setup", title: "Set up the website" });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([task]);
    const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (_id, status) => {
      task = { ...task, status, completedAt: status === "completed" ? new Date().toISOString() : null };
      return task;
    });
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Set up the website");
    const status = dialog.getByRole("combobox", { name: "Status" });
    expect(status).toHaveValue("open");
    expect(dialog.queryByRole("option", { name: "Cancelled" })).not.toBeInTheDocument();
    fireEvent.change(status, { target: { value: "in_progress" } });
    await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("task-1", "in_progress"));
    expect(await within(screen.getByRole("region", { name: "In Progress", hidden: true })).findByText("Set up the website")).toBeInTheDocument();
    fireEvent.change(status, { target: { value: "completed" } });
    await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("task-1", "completed"));
    expect(await within(screen.getByRole("region", { name: "Done", hidden: true })).findByText("Set up the website")).toBeInTheDocument();
    expect(dialog.getByRole("combobox", { name: "Status" })).toHaveValue("completed");
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Set up the website");
  });

  it("shows a cancelled task's status without offering Cancelled for other tasks", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("task-1", { title: "Dropped", status: "cancelled", completedAt: new Date().toISOString() })]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const dialog = await openTask("Dropped");
    expect(dialog.getByRole("combobox", { name: "Status" })).toHaveValue("cancelled");
  });

  it("shows no conversation controls for a standalone task", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("standalone-1", { title: "Buy printer paper" })]);
    const onOpenThread = vi.fn();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={onOpenThread} />);

    const dialog = await openTask("Buy printer paper");
    expect(dialog.queryByRole("region", { name: "Source conversation" })).not.toBeInTheDocument();
    expect(dialog.queryByRole("button", { name: "Open conversation" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Task details" }).querySelector(".task-detail-form")!, { key: "o" });
    expect(onOpenThread).not.toHaveBeenCalled();
  });

  it("opens the selected task's details through the workspace handle", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([workspaceTask("task-1", { title: "First" }), workspaceTask("task-2", { title: "Second" })]);
    localStorage.setItem("threestrands.tasks.layout", "list");
    const ref = createRef<TaskWorkspaceHandle>();
    render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);

    await screen.findByText("Second", { selector: "strong" });
    act(() => ref.current!.selectNext());
    act(() => ref.current!.openDetails());
    const dialog = within(await screen.findByRole("dialog", { name: "Task details" }));
    expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Second");
  });

});

describe("TaskSidebar goals", () => {
  const quarter = periodFor("quarter");
  const year = periodFor("year");

  function workspaceGoal(id: string, overrides: Partial<Goal> = {}): Goal {
    return {
      id, accountId: "you@example.com", title: id, notes: null, horizon: "quarter", period: quarter, status: "active",
      parentGoalId: null, createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", closedAt: null,
      ...overrides,
    };
  }

  function setup(tasks: ThreadTask[], goals: Goal[]) {
    let taskStore = [...tasks];
    let goalStore = [...goals];
    vi.spyOn(mailClient, "listTasks").mockImplementation(async () => taskStore);
    vi.spyOn(mailClient, "listGoals").mockImplementation(async () => goalStore);
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      taskStore = taskStore.map((task) => task.id === request.id ? { ...task, ...request } : task);
      return taskStore.find((task) => task.id === request.id)!;
    });
    const createGoal = vi.spyOn(mailClient, "createGoal").mockImplementation(async (request) => {
      const created = workspaceGoal("created-goal", { ...request, notes: request.notes ?? null, parentGoalId: request.parentGoalId ?? null });
      goalStore = [...goalStore, created];
      return created;
    });
    const updateGoal = vi.spyOn(mailClient, "updateGoal").mockImplementation(async (request) => {
      goalStore = goalStore.map((goal) => goal.id === request.id ? { ...goal, ...request } : goal);
      return goalStore.find((goal) => goal.id === request.id)!;
    });
    const deleteGoal = vi.spyOn(mailClient, "deleteGoal").mockImplementation(async (id) => {
      goalStore = goalStore.filter((goal) => goal.id !== id);
      taskStore = taskStore.map((task) => task.goalId === id ? { ...task, goalId: null } : task);
    });
    return { updateTask, createGoal, updateGoal, deleteGoal };
  }

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("lists the current quarter's and year's goals with linked task counts, hiding an empty half and older periods", async () => {
    setup([
      workspaceTask("a", { goalId: "ship" }), workspaceTask("b", { goalId: "ship", status: "completed", completedAt: new Date().toISOString() }),
      workspaceTask("c"),
    ], [
      workspaceGoal("ship", { title: "Ship IMAP", parentGoalId: "grow" }),
      workspaceGoal("grow", { title: "Grow the practice", horizon: "year", period: year }),
      workspaceGoal("old", { title: "Old quarter", period: "2020-Q1", status: "achieved" }),
    ]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const pane = await screen.findByRole("complementary", { name: "Goals" });
    const quarterGoals = within(pane).getByRole("region", { name: "Quarter goals" });
    expect(await within(quarterGoals).findByRole("button", { name: /^Ship IMAP/ })).toHaveTextContent("1 open · 1 done");
    expect(within(quarterGoals).getByRole("button", { name: /^Ship IMAP/ })).toHaveTextContent("↳ Grow the practice");
    expect(within(within(pane).getByRole("region", { name: "Year goals" })).getByRole("button", { name: /^Grow the practice/ })).toHaveTextContent("0 open · 0 done");
    expect(within(pane).queryByRole("region", { name: "Half goals" })).not.toBeInTheDocument();
    const other = within(pane).getByText("Other periods (1)").closest("details")!;
    expect(other).not.toHaveAttribute("open");
    expect(other).toHaveTextContent("Achieved");
    expect(within(pane).getByRole("button", { name: /No goal/ })).toHaveTextContent("1 open");
  });

  it("filters tasks to a goal and the goals supporting it, to tasks with no goal, and clears from the header chip", async () => {
    setup([
      workspaceTask("direct", { title: "Direct", goalId: "grow" }),
      workspaceTask("through", { title: "Through quarter", goalId: "ship" }),
      workspaceTask("loose", { title: "Loose" }),
    ], [
      workspaceGoal("grow", { title: "Grow", horizon: "year", period: year }),
      workspaceGoal("ship", { title: "Ship", parentGoalId: "grow" }),
    ]);
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    const titles = () => [...container.querySelectorAll(".task-card strong")].map((node) => node.textContent);
    await waitFor(() => expect(titles()).toHaveLength(3));
    const pane = screen.getByRole("complementary", { name: "Goals" });

    fireEvent.click(within(pane).getByRole("button", { name: /^Grow/ }));
    expect(titles()).toEqual(expect.arrayContaining(["Direct", "Through quarter"]));
    expect(titles()).toHaveLength(2);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("2 tasks");
    expect(within(pane).getByRole("button", { name: /^Grow/ })).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(within(pane).getByRole("button", { name: /^Ship/ }));
    expect(titles()).toEqual(["Through quarter"]);
    // Choosing the selected goal again shows every task.
    fireEvent.click(within(pane).getByRole("button", { name: /^Ship/ }));
    expect(titles()).toHaveLength(3);

    fireEvent.click(within(pane).getByRole("button", { name: /No goal/ }));
    expect(titles()).toEqual(["Loose"]);
    fireEvent.click(screen.getByRole("button", { name: "Show all tasks, not only those with no goal" }));
    expect(titles()).toHaveLength(3);
    expect(within(pane).getByRole("button", { name: /All tasks/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows the supported goal on cards and links a task added while one goal is in view", async () => {
    setup([workspaceTask("linked", { title: "Linked", goalId: "ship" })], [workspaceGoal("ship", { title: "Ship IMAP", accountId: "work@example.com" })]);
    const onCreateTask = vi.fn().mockResolvedValue(workspaceTask("new", { title: "Write tests", goalId: "ship" }));
    const { container } = render(<TaskSidebar accountId={null} accountOptions={["you@example.com", "work@example.com"]} onOpenThread={vi.fn()} onCreateTask={onCreateTask} />);

    const card = (await screen.findByText("Linked", { selector: "strong" })).closest("article")!;
    await waitFor(() => expect(card.querySelector(".task-card-goal")).toHaveTextContent("Supports Ship IMAP"));

    fireEvent.click(within(screen.getByRole("complementary", { name: "Goals" })).getByRole("button", { name: /^Ship IMAP/ }));
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
    const form = container.querySelector(".task-quick-add") as HTMLElement;
    expect(form).toHaveTextContent("Supports Ship IMAP");
    expect(within(form).queryByRole("combobox", { name: "Account" })).not.toBeInTheDocument();
    fireEvent.change(within(form).getByRole("textbox", { name: "Task title" }), { target: { value: "Write tests" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith("Write tests", "work@example.com", "ship"));
  });

  it("links a task to one of its account's active goals from the task dialog", async () => {
    const { updateTask } = setup([workspaceTask("plan", { title: "Plan launch" })], [
      workspaceGoal("ship", { title: "Ship IMAP" }),
      workspaceGoal("grow", { title: "Grow", horizon: "year", period: year }),
      workspaceGoal("done", { title: "Finished goal", status: "achieved" }),
      workspaceGoal("theirs", { title: "Other account goal", accountId: "other@example.com" }),
    ]);
    render(<TaskSidebar accountId={null} onOpenThread={vi.fn()} />);

    const dialog = await openTask("Plan launch");
    const select = dialog.getByRole("combobox", { name: "Goal" });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "No goal", `Ship IMAP (${formatPeriod(quarter)})`, `Grow (${year})`,
    ]);
    fireEvent.change(select, { target: { value: "ship" } });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", goalId: "ship" }));
    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(updateTask).toHaveBeenLastCalledWith({ id: "plan", goalId: null }));
  });

  it("adds a goal that supports a longer one and shows its tasks", async () => {
    const { createGoal } = setup([], [workspaceGoal("grow", { title: "Grow", horizon: "year", period: year })]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await within(await screen.findByRole("complementary", { name: "Goals" })).findByRole("button", { name: "Add goal" }));
    const dialog = within(screen.getByRole("dialog", { name: "Add goal" }));
    expect(dialog.getByRole("combobox", { name: "Horizon" })).toHaveValue("quarter");
    expect(dialog.getByRole("combobox", { name: "Period" })).toHaveValue(quarter);
    expect(dialog.getByRole("button", { name: "Add goal" })).toBeDisabled();
    fireEvent.change(dialog.getByRole("textbox", { name: "Goal" }), { target: { value: "  Ship IMAP  " } });
    fireEvent.change(dialog.getByRole("combobox", { name: "Supports" }), { target: { value: "grow" } });
    fireEvent.click(dialog.getByRole("button", { name: "Add goal" }));

    await waitFor(() => expect(createGoal).toHaveBeenCalledWith({
      accountId: "you@example.com", title: "Ship IMAP", notes: null, horizon: "quarter", period: quarter, parentGoalId: "grow",
    }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /Show all tasks, not only those supporting/ })).toBeInTheDocument();
  });

  it("discards a new goal on Escape", async () => {
    const { createGoal } = setup([], []);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    fireEvent.click(await within(await screen.findByRole("complementary", { name: "Goals" })).findByRole("button", { name: "Add goal" }));
    fireEvent.change(within(screen.getByRole("dialog", { name: "Add goal" })).getByRole("textbox", { name: "Goal" }), { target: { value: "Half typed" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(createGoal).not.toHaveBeenCalled();
  });

  it("saves goal edits as the user leaves each field and unlinks a supported goal the new horizon cannot support", async () => {
    const { updateGoal } = setup([], [
      workspaceGoal("grow", { title: "Grow", horizon: "year", period: year }),
      workspaceGoal("ship", { title: "Ship", parentGoalId: "grow" }),
    ]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Edit goal Ship" }));
    const dialog = within(screen.getByRole("dialog", { name: "Goal" }));
    expect(dialog.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    const title = dialog.getByRole("textbox", { name: "Goal" });
    fireEvent.change(title, { target: { value: "Ship the IMAP provider" } });
    fireEvent.blur(title);
    await waitFor(() => expect(updateGoal).toHaveBeenCalledWith({ id: "ship", title: "Ship the IMAP provider" }));

    fireEvent.change(dialog.getByRole("combobox", { name: "Status" }), { target: { value: "achieved" } });
    await waitFor(() => expect(updateGoal).toHaveBeenLastCalledWith({ id: "ship", status: "achieved" }));

    // A year goal cannot support another year goal, so the link goes with the move.
    fireEvent.change(dialog.getByRole("combobox", { name: "Horizon" }), { target: { value: "year" } });
    await waitFor(() => expect(updateGoal).toHaveBeenLastCalledWith({ id: "ship", horizon: "year", period: year, parentGoalId: null }));
    expect(dialog.queryByRole("combobox", { name: "Supports" })).not.toBeInTheDocument();

    fireEvent.change(dialog.getByRole("textbox", { name: "Notes" }), { target: { value: "Escape keeps this" } });
    fireEvent.keyDown(dialog.getByRole("textbox", { name: "Notes" }), { key: "Escape" });
    await waitFor(() => expect(updateGoal).toHaveBeenLastCalledWith({ id: "ship", notes: "Escape keeps this" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("deletes a goal only after confirmation and unlinks its tasks", async () => {
    const { deleteGoal } = setup([workspaceTask("linked", { title: "Linked", goalId: "ship" })], [workspaceGoal("ship", { title: "Ship" })]);
    const onTasksChanged = vi.fn();
    const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} onTasksChanged={onTasksChanged} />);

    await waitFor(() => expect(container.querySelector(".task-card-goal")).toHaveTextContent("Ship"));
    fireEvent.click(screen.getByRole("button", { name: /^Ship/ }));
    fireEvent.click(screen.getByRole("button", { name: "Edit goal Ship" }));
    const dialog = within(screen.getByRole("dialog", { name: "Goal" }));
    fireEvent.click(dialog.getByRole("button", { name: "Delete goal" }));
    expect(deleteGoal).not.toHaveBeenCalled();
    expect(dialog.getByText(/Its tasks stay and are unlinked/)).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(deleteGoal).toHaveBeenCalledWith("ship"));
    await waitFor(() => expect(container.querySelector(".task-card-goal")).toBeNull());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show all tasks/ })).not.toBeInTheDocument();
    expect(onTasksChanged).toHaveBeenCalled();
  });

  it("resizes the goals pane from its divider and remembers the width", async () => {
    setup([workspaceTask("todo", { title: "Draft agenda" })], []);
    const originalInnerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth");
    Object.defineProperty(window, "innerWidth", { value: 1600, configurable: true });
    try {
      const { container } = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
      const handle = await screen.findByRole("separator", { name: "Resize goals" });
      expect(handle).toHaveAttribute("aria-controls", "goals-pane");
      expect(container.querySelector("#goals-pane")).toBeInTheDocument();
      const body = container.querySelector(".tasks-workspace-body") as HTMLElement;
      expect(body.style.getPropertyValue("--goals-pane-width")).toBe("320px");
      // The pane sits right of the divider, so ArrowLeft widens it.
      fireEvent.keyDown(handle, { key: "ArrowLeft" });
      expect(body.style.getPropertyValue("--goals-pane-width")).toBe("330px");
      await waitFor(() => expect(localStorage.getItem("threestrands.goalsPaneWidth")).toBe("330"));
      fireEvent.dblClick(handle);
      expect(body.style.getPropertyValue("--goals-pane-width")).toBe("320px");
    } finally {
      if (originalInnerWidth) Object.defineProperty(window, "innerWidth", originalInnerWidth);
    }
  });

  it("moves focus into the goals with the handle, steps with j and k without moving the task selection, and returns on Escape", async () => {
    setup([workspaceTask("first", { title: "First" }), workspaceTask("second", { title: "Second" })], [workspaceGoal("ship", { title: "Ship" })]);
    localStorage.setItem("threestrands.tasks.layout", "list");
    const ref = createRef<TaskWorkspaceHandle>();
    const { container } = render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);
    await screen.findByRole("button", { name: /^Ship/ });
    await waitFor(() => expect(container.querySelector("#task-first")).toHaveAttribute("aria-current", "true"));

    act(() => ref.current!.focusGoals());
    const allTasks = screen.getByRole("button", { name: /All tasks/ });
    expect(allTasks).toHaveFocus();
    const down = new KeyboardEvent("keydown", { key: "j", bubbles: true, cancelable: true });
    act(() => { allTasks.dispatchEvent(down); });
    expect(down.defaultPrevented).toBe(true);
    expect(screen.getByRole("button", { name: /No goal/ })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: /^Ship/ })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "k" });
    expect(screen.getByRole("button", { name: /No goal/ })).toHaveFocus();

    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(container.querySelector("#task-first .task-card-main")).toHaveFocus();
  });
  it("flags a current goal stale after three weeks without task activity", async () => {
    const old = new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString();
    setup([workspaceTask("quiet", { goalId: "quiet-goal", updatedAt: old })], [
      workspaceGoal("quiet-goal", { title: "Quiet", createdAt: old }),
      workspaceGoal("fresh", { title: "Fresh" , createdAt: new Date().toISOString() }),
    ]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const quiet = await screen.findByRole("button", { name: /^Quiet/ });
    expect(within(quiet).getByText("Stale")).toHaveAttribute("title", "No task activity in 21 days");
    expect(within(screen.getByRole("button", { name: /^Fresh/ })).queryByText("Stale")).not.toBeInTheDocument();
  });

  it("prompts to review ended goals and settles each as achieved, dropped, or carried forward", async () => {
    const lastQuarter = shiftPeriod("quarter", quarter, -1);
    const { updateGoal } = setup([workspaceTask("linked", { goalId: "carry" })], [
      workspaceGoal("win", { title: "Win", period: lastQuarter }),
      workspaceGoal("drop", { title: "Drop", period: lastQuarter }),
      workspaceGoal("carry", { title: "Carry", period: lastQuarter }),
      workspaceGoal("current", { title: "Current" }),
    ]);
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    const prompt = await screen.findByText("3 goals from a past period need review.");
    fireEvent.click(within(prompt.closest(".goal-review-prompt") as HTMLElement).getByRole("button", { name: "Review" }));
    const dialog = within(screen.getByRole("dialog", { name: "Review goals" }));
    expect(dialog.getAllByRole("listitem").map((item) => item.getAttribute("aria-label"))).toEqual(["Carry", "Drop", "Win"]);

    fireEvent.click(within(dialog.getByRole("listitem", { name: "Win" })).getByRole("button", { name: "Achieved" }));
    await waitFor(() => expect(updateGoal).toHaveBeenCalledWith({ id: "win", status: "achieved" }));
    fireEvent.click(within(await dialog.findByRole("listitem", { name: "Drop" })).getByRole("button", { name: "Dropped" }));
    await waitFor(() => expect(updateGoal).toHaveBeenCalledWith({ id: "drop", status: "dropped" }));
    fireEvent.click(within(await dialog.findByRole("listitem", { name: "Carry" })).getByRole("button", { name: `Carry to ${formatPeriod(quarter)}` }));
    await waitFor(() => expect(updateGoal).toHaveBeenCalledWith({ id: "carry", period: quarter }));

    expect(await dialog.findByText(/All caught up/)).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Done" }));
    expect(screen.queryByText(/need review/)).not.toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Quarter goals" })).getByRole("button", { name: /^Carry/ })).toHaveTextContent("1 open");
  });

  it("shows a failed settle on its row and defers the prompt for the quarter with Later", async () => {
    const lastQuarter = shiftPeriod("quarter", quarter, -1);
    const { updateGoal } = setup([], [workspaceGoal("stuck", { title: "Stuck", period: lastQuarter })]);
    updateGoal.mockRejectedValueOnce(new Error("Some goals that support this one fall outside its new period. Unlink them first."));
    const view = render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const row = within(screen.getByRole("dialog", { name: "Review goals" }).querySelector("li")!);
    fireEvent.click(row.getByRole("button", { name: `Carry to ${formatPeriod(quarter)}` }));
    expect(await row.findByRole("alert")).toHaveTextContent("Unlink them first");
    fireEvent.click(screen.getByRole("button", { name: "Finish later" }));

    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(screen.queryByText(/needs review/)).not.toBeInTheDocument();
    expect(localStorage.getItem("threestrands.goals.reviewDeferredUntil")).toBe(quarter);
    view.unmount();
    render(<TaskSidebar accountId="you@example.com" onOpenThread={vi.fn()} />);
    await screen.findByRole("button", { name: /^Stuck/ });
    expect(screen.queryByText(/needs review/)).not.toBeInTheDocument();
  });

  it("links the selected task to a goal from the picker, creates a quarter goal from a new name, and unlinks", async () => {
    const { updateTask, createGoal } = setup([workspaceTask("plan", { title: "Plan launch" })], [
      workspaceGoal("ship", { title: "Ship IMAP" }),
      workspaceGoal("closed", { title: "Closed goal", status: "achieved" }),
    ]);
    const ref = createRef<TaskWorkspaceHandle>();
    render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);
    await screen.findByRole("button", { name: /^Ship IMAP/ });
    await waitFor(() => expect(screen.getByText("Plan launch", { selector: "strong" }).closest("article")).toHaveAttribute("aria-current", "true"));

    act(() => ref.current!.linkSelectedToGoal());
    let picker = within(screen.getByRole("dialog", { name: "Link “Plan launch” to a goal" }));
    expect(picker.getAllByRole("option").map((option) => option.getAttribute("aria-label"))).toEqual(["No goal, current", "Ship IMAP"]);
    fireEvent.change(picker.getByRole("combobox", { name: "Find or create a goal" }), { target: { value: "ship" } });
    fireEvent.keyDown(picker.getByRole("combobox", { name: "Find or create a goal" }), { key: "Enter" });
    await waitFor(() => expect(updateTask).toHaveBeenCalledWith({ id: "plan", goalId: "ship" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    act(() => ref.current!.linkSelectedToGoal());
    picker = within(screen.getByRole("dialog", { name: "Link “Plan launch” to a goal" }));
    fireEvent.change(picker.getByRole("combobox", { name: "Find or create a goal" }), { target: { value: "Hire a designer" } });
    fireEvent.click(picker.getByText(`Create ${formatPeriod(quarter)} goal “Hire a designer”`));
    await waitFor(() => expect(createGoal).toHaveBeenCalledWith({ accountId: "you@example.com", title: "Hire a designer", horizon: "quarter", period: quarter }));
    await waitFor(() => expect(updateTask).toHaveBeenLastCalledWith({ id: "plan", goalId: "created-goal" }));

    act(() => ref.current!.linkSelectedToGoal());
    picker = within(screen.getByRole("dialog", { name: "Link “Plan launch” to a goal" }));
    fireEvent.click(picker.getByRole("option", { name: "No goal" }));
    await waitFor(() => expect(updateTask).toHaveBeenLastCalledWith({ id: "plan", goalId: null }));
  });
});
