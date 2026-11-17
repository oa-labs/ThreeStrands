import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { planItemKey } from "./dailyPlan";
import type { CalendarAccount, CalendarOption, DailyPlan, DailyPlanItem, Note, PlanReview, PlanReviewItem, ScheduleEvent, ThreadTask } from "./domain";
import { PlanTodayDialog, type PlanTodayDialogProps } from "./PlanTodayDialog";
import { readRecovery, startRoutine, addNewTask, writeRecovery } from "./planRoutine";
import type { PlanCandidate, PlanSource } from "./TodayPlan";

const localDate = (days: number) => {
  const date = new Date(Date.now() + days * 86_400_000);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
};
const DAY = localDate(1);
const ref = (id: string) => ({ version: 1 as const, kind: "task" as const, id });
const item = (id: string, overrides: Partial<DailyPlanItem> = {}): DailyPlanItem => ({
  key: planItemKey(ref(id)), kind: "task", reference: ref(id), title: `Task ${id}`, state: "open", marker: null, taskId: id, accountId: "me@example.com",
  goal: null, contactId: null, carryCount: 0, start: 0, end: 0, ...overrides,
});
const row = (id: string, overrides: Partial<PlanReviewItem> = {}): PlanReviewItem => ({ ...item(id), plannedOn: localDate(-1), ...overrides });
const plan = (items: DailyPlanItem[] = []): DailyPlan => ({ date: DAY, noteId: `daily:${DAY}`, revision: items.length ? "r1" : null, items, notToday: [] });
const emptyReview: PlanReview = { date: DAY, reviewed: null, done: [], alsoDone: [], meetings: null, didntGetTo: [] };
const note = (updatedAt: string): Note => ({ id: `daily:${DAY}`, title: DAY, body: "", dailyDate: DAY, createdAt: updatedAt, updatedAt, trashed: false, trashedAt: null });
const task = (id: string): ThreadTask => ({ id, accountId: "me@example.com", threadId: null, subjectSnapshot: null, title: `Task ${id}`, kind: "action", dueKind: "none", status: "open", createdAt: "", updatedAt: "" });
const calendars: CalendarOption[] = [{ id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true }];
const calendarAccounts: CalendarAccount[] = [{ email: "cal@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];
const allWeek = { timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, workingWindows: [0, 1, 2, 3, 4, 5, 6].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })) };

function candidate(id: string): PlanCandidate {
  return {
    id: `task:${id}`, title: `Task ${id}`, planKey: planItemKey(ref(id)),
    resolve: async () => ({ addition: { kind: "link", reference: ref(id), label: `Task ${id}` }, block: { title: `Task ${id}`, sourceTaskId: id, accountId: "me@example.com" }, title: `Task ${id}` }),
    row: { as: "article", onActivate: vi.fn() },
  };
}
const sources: PlanSource[] = [{ id: "tasks", label: "Tasks", empty: "No open tasks.", groups: [{ id: "open", title: "Open tasks", candidates: [candidate("c1"), candidate("c2")] }] }];

function setup({ current = plan(), review = emptyReview }: { current?: DailyPlan; review?: PlanReview } = {}) {
  vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(current);
  vi.spyOn(mailClient, "planReview").mockResolvedValue(review);
  vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("c1"), task("c2"), task("search-me")]);
  vi.spyOn(mailClient, "listNoteTasks").mockResolvedValue([]);
  vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([]);
  const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue({ operationId: "op", note: note("r9"), added: ["k"], removed: [], notToday: [], already: [], changed: true });
  const undo = vi.spyOn(mailClient, "undoDailyPlanSave").mockResolvedValue(note("r10"));
  return { save, undo };
}

/** The dialog opened from a trigger button, closing as Today closes it. */
function Harness(overrides: Partial<PlanTodayDialogProps>) {
  const [open, setOpen] = useState(false);
  return <>
    <button type="button" onClick={() => setOpen(true)}>Open Routine</button>
    {open ? <PlanTodayDialog date={DAY} dayName="Tomorrow" sources={sources} sourceId="tasks" onSelectSource={vi.fn()} events={null} scheduleLoading={false} scheduleError={null}
      onRetrySchedule={vi.fn()} workingHours={allWeek} booking={{ accounts: calendarAccounts, calendars, onNotice: overrides.onNotice ?? vi.fn() }}
      onNotice={vi.fn()} onOpenNote={vi.fn()} onSaved={vi.fn()} {...overrides} onClose={() => { overrides.onClose?.(); setOpen(false); }} /> : null}
  </>;
}

async function openRoutine(overrides: Partial<PlanTodayDialogProps> = {}) {
  render(<Harness {...overrides} />);
  const trigger = screen.getByRole("button", { name: "Open Routine" });
  trigger.focus();
  fireEvent.click(trigger);
  const dialog = await screen.findByRole("dialog", { name: overrides.dayName === "Today" ? "Plan Today" : "Plan Tomorrow" });
  await within(dialog).findByRole("navigation", { name: "Steps" });
  return { dialog, trigger };
}
const stepButton = (dialog: HTMLElement, name: string) => within(within(dialog).getByRole("navigation", { name: "Steps" })).getByRole("button", { name: new RegExp(`^\\d\\. ${name}$`) });

beforeEach(() => window.localStorage.clear());
afterEach(() => { cleanup(); vi.restoreAllMocks(); window.localStorage.clear(); });

describe("PlanTodayDialog", () => {
  it("is a modal form: modal shortcut scope, inert background, focus on the step's first control, Tab trapped", async () => {
    setup();
    const { dialog, trigger } = await openRoutine();
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    // The page under the dialog (here, the trigger's container) is inert.
    expect((trigger.parentElement as HTMLElement).inert).toBe(true);
    // Nothing to review, so the routine starts at Choose and Review isn't in the step list.
    expect(within(dialog).queryByRole("button", { name: /Review$/ })).not.toBeInTheDocument();
    expect(stepButton(dialog, "Choose")).toHaveAttribute("aria-current", "step");
    const body = within(dialog).getByRole("group", { name: "Choose" });
    await waitFor(() => expect(body.contains(document.activeElement)).toBe(true));
    const focusable = [...dialog.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex='-1'])")];
    focusable.at(-1)!.focus();
    fireEvent.keyDown(focusable.at(-1)!, { key: "Tab" });
    expect(document.activeElement).toBe(focusable[0]);
  });

  it("closes on Escape without choices and restores focus to the trigger", async () => {
    setup();
    const onClose = vi.fn();
    const { dialog, trigger } = await openRoutine({ onClose });
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(document.activeElement).toBe(trigger);
  });

  it("asks to keep or discard choices on Escape: Keep for Later keeps them on this device, Discard Choices forgets them", async () => {
    const { save } = setup();
    const { dialog } = await openRoutine();
    fireEvent.click(within(dialog).getByRole("button", { name: "Add to Today's Plan: Task c1" }));
    await within(dialog).findByRole("button", { name: "Remove: Task c1" });
    fireEvent.keyDown(dialog, { key: "Escape" });
    const ask = within(dialog).getByRole("group", { name: "Close Plan" });
    // A second Escape goes back to the routine with its choices.
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(within(dialog).queryByRole("group", { name: "Close Plan" })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(within(dialog).getByRole("group", { name: "Close Plan" })).getByRole("button", { name: "Keep for Later" }));
    expect(ask).not.toBeInTheDocument();
    expect(readRecovery(DAY)?.items.map((entry) => entry.title)).toEqual(["Task c1"]);
    expect(save).not.toHaveBeenCalled();

    // Opening again restores them, with Discard.
    fireEvent.click(screen.getByRole("button", { name: "Open Routine" }));
    const again = await screen.findByRole("dialog", { name: "Plan Tomorrow" });
    expect(await within(again).findByRole("status")).toHaveTextContent(/Restored the choices you hadn't saved/);
    expect(within(again).getByRole("button", { name: "Remove: Task c1" })).toBeInTheDocument();
    fireEvent.keyDown(again, { key: "Escape" });
    fireEvent.click(within(again).getByRole("button", { name: "Discard Choices" }));
    expect(readRecovery(DAY)).toBeNull();
  });

  it("restores kept choices after a restart and Discard starts again from the plan", async () => {
    setup({ current: plan([item("a")]) });
    writeRecovery(addNewTask({ ...startRoutine(DAY, plan([item("a")])), step: "choose" }, "Typed before restart"));
    const { dialog } = await openRoutine();
    expect(within(dialog).getByText("Typed before restart")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard" }));
    expect(within(dialog).queryByText("Typed before restart")).not.toBeInTheDocument();
    expect(within(dialog).getByText("Task a")).toBeInTheDocument();
    expect(readRecovery(DAY)).toBeNull();
  });

  it("reviews the last plan: Carry, Defer…, Drop, Carry All, carry counts, and the hint from three carries", async () => {
    const review: PlanReview = {
      ...emptyReview, reviewed: localDate(-1), done: [item("finished", { title: "Finished thing", state: "done" })],
      alsoDone: [{ key: "x", taskId: "x", title: "Unplanned win", accountId: null, noteId: null, noteTitle: null, completedAt: "" }],
      didntGetTo: [row("a"), row("b", { carryCount: 3 }), row("c"), row("gone", { state: "unavailable" })],
    };
    const { save } = setup({ review });
    const { dialog } = await openRoutine();
    expect(stepButton(dialog, "Review")).toHaveAttribute("aria-current", "step");
    expect(within(dialog).getByText("Finished thing")).toBeInTheDocument();
    expect(within(dialog).getByText("Unplanned win")).toBeInTheDocument();
    expect(within(dialog).getByText("Carried 3 times. Defer or drop it?")).toBeInTheDocument();
    expect(within(within(dialog).getByRole("group", { name: "Decide: Task gone" })).getByRole("button", { name: "Carry" })).toBeDisabled();

    const decide = (title: string) => within(within(dialog).getByRole("group", { name: `Decide: ${title}` }));
    fireEvent.click(decide("Task b").getByRole("button", { name: "Defer…" }));
    const until = within(dialog).getByLabelText("Defer Until");
    expect(until).toHaveAttribute("min", localDate(2));
    fireEvent.change(until, { target: { value: localDate(5) } });
    expect(decide("Task b").getByRole("button", { name: "Defer…" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(decide("Task gone").getByRole("button", { name: "Drop" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Carry All" }));
    expect(decide("Task a").getByRole("button", { name: "Carry" })).toHaveAttribute("aria-pressed", "true");
    expect(decide("Task c").getByRole("button", { name: "Carry" })).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(stepButton(dialog, "Save"));
    expect(within(dialog).getByText(/<!-- threestrands-plan:deferred/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Plan" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    const changes = save.mock.calls[0][3];
    expect(changes.items).toEqual([
      { kind: "add", addition: { kind: "link", reference: ref("a"), label: "Task a" }, carried: localDate(-1) },
      { kind: "add", addition: { kind: "link", reference: ref("c"), label: "Task c" }, carried: localDate(-1) },
    ]);
    expect(changes.notToday).toEqual([
      { reference: ref("b"), label: "Task b", decision: { kind: "deferred", date: localDate(5) } },
      { reference: ref("gone"), label: "Task gone", decision: { kind: "dropped" } },
    ]);
  });

  it("chooses from sources, search, and New Task, reorders with buttons and Alt+Up/Down, and notes more than five items", async () => {
    const { save } = setup({ current: plan([item("a")]) });
    const { dialog } = await openRoutine();
    // Edit Plan starts from the saved plan's items; cards already chosen are marked.
    expect(within(dialog).getByText("Task a")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Add to Today's Plan: Task c1" }));
    const chosen = await within(dialog).findByRole("button", { name: "Remove: Task c1" });
    expect(chosen).toBeInTheDocument();
    expect(within(within(dialog).getByRole("tabpanel")).getAllByText("In plan")).toHaveLength(1);

    fireEvent.change(within(dialog).getByRole("searchbox", { name: "Search Tasks" }), { target: { value: "search" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add: Task search-me" }));
    const title = within(dialog).getByRole("textbox", { name: "New Task" });
    fireEvent.change(title, { target: { value: "Draft memo" } });
    fireEvent.keyDown(title, { key: "Enter" });
    expect(title).toHaveValue("");

    const order = () => within(dialog).getAllByRole("listitem").filter((entry) => entry.hasAttribute("data-item")).map((entry) => entry.querySelector("strong")?.textContent);
    expect(order()).toEqual(["Task a", "Task c1", "Task search-me", "Draft memo"]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Move Up: Draft memo" }));
    const memo = within(dialog).getByRole("button", { name: "Move Up: Draft memo" });
    fireEvent.keyDown(memo, { key: "ArrowUp", altKey: true });
    expect(order()).toEqual(["Task a", "Draft memo", "Task c1", "Task search-me"]);
    fireEvent.keyDown(within(dialog).getByRole("button", { name: "Move Down: Task a" }), { key: "ArrowDown", altKey: true });
    expect(order()).toEqual(["Draft memo", "Task a", "Task c1", "Task search-me"]);

    expect(within(dialog).queryByText("Most people finish three to five planned items.")).not.toBeInTheDocument();
    for (const name of ["One", "Two"]) { fireEvent.change(title, { target: { value: name } }); fireEvent.click(within(dialog).getByRole("button", { name: "Add New Task" })); }
    expect(within(dialog).getByText("Most people finish three to five planned items.")).toBeInTheDocument();

    // Removing a plan item takes its line off; it isn't dropped.
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove: Task a" }));
    // Cmd/Ctrl+Enter finishes from any step.
    fireEvent.keyDown(title, { key: "Enter", metaKey: true });
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][1]).toBe("r1");
    expect(save.mock.calls[0][3].remove).toEqual([item("a").key]);
    expect(save.mock.calls[0][3].notToday).toEqual([]);
    expect(save.mock.calls[0][3].items.map((entry) => entry.kind === "add" ? entry.addition : entry)).toEqual([
      { kind: "newTask", title: "Draft memo" },
      { kind: "link", reference: ref("c1"), label: "Task c1" },
      { kind: "link", reference: ref("search-me"), label: "Task search-me" },
      { kind: "newTask", title: "One" },
      { kind: "newTask", title: "Two" },
    ]);
  });

  it("prepares a meeting inside the routine and returns to Meetings with the choices intact", async () => {
    setup();
    const start = new Date(`${DAY}T10:00:00`);
    const meeting: ScheduleEvent = { id: "m1", accountId: "cal@example.com", calendarId: "primary", title: "Planning sync", allDay: false, start: start.toISOString(), end: new Date(start.getTime() + 1_800_000).toISOString(), attendees: ["sam@example.com"] };
    const render = vi.fn((event: ScheduleEvent) => <p>Preparing {event.title}</p>);
    const { dialog } = await openRoutine({ events: [meeting], preparation: { render, ownAddresses: ["me@example.com"] } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add to Today's Plan: Task c1" }));
    await within(dialog).findByRole("button", { name: "Remove: Task c1" });
    fireEvent.click(stepButton(dialog, "Meetings"));
    fireEvent.click(await within(dialog).findByRole("button", { name: /^Planning sync/ }));
    expect(within(dialog).getByText("Preparing Planning sync")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Back to Meetings" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "Back to Meetings" }));
    expect(within(dialog).getByRole("button", { name: /^Planning sync/ })).toBeInTheDocument();
    fireEvent.click(stepButton(dialog, "Choose"));
    expect(within(dialog).getByRole("button", { name: "Remove: Task c1" })).toBeInTheDocument();
  });

  it("books a chosen item into the plan date's next free half hour at once, with Undo, and lists the booking at Save", async () => {
    setup();
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => ({
      id: "booked", accountId: request.accountId, calendarId: request.calendarId, title: request.title, allDay: false, start: request.start, end: request.end, sourceTaskId: request.sourceTaskId ?? null,
    }));
    const remove = vi.spyOn(mailClient, "deleteCalendarEvent").mockResolvedValue(undefined);
    const onNotice = vi.fn();
    const { dialog } = await openRoutine({ events: [], onNotice });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add to Today's Plan: Task c1" }));
    await within(dialog).findByRole("button", { name: "Remove: Task c1" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Next" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Next" }));
    expect(stepButton(dialog, "Fit")).toHaveAttribute("aria-current", "step");
    expect(within(dialog).getByText(/8 h free in working hours · 0 booked · 1 not booked/)).toBeInTheDocument();
    expect(within(dialog).getByRole("group", { name: "Plan Day" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Book: Task c1" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ title: "Task c1", sourceTaskId: "c1", start: new Date(`${DAY}T09:00:00`).toISOString() })));
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 9:00\s?am for “Task c1”$/), undo: expect.any(Function) })));
    fireEvent.click(stepButton(dialog, "Save"));
    expect(within(dialog).getByRole("region", { name: "Already Booked" })).toHaveTextContent("Task c1");
    await act(async () => { onNotice.mock.calls[0][0].undo(); });
    await waitFor(() => expect(remove).toHaveBeenCalled());
    await waitFor(() => expect(within(dialog).queryByRole("region", { name: "Already Booked" })).not.toBeInTheDocument());
  });

  it("says free time is unknown without a calendar and still saves", async () => {
    const { save } = setup();
    const { dialog } = await openRoutine({ events: null });
    const title = within(dialog).getByRole("textbox", { name: "New Task" });
    fireEvent.change(title, { target: { value: "Draft memo" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add New Task" }));
    fireEvent.click(stepButton(dialog, "Fit"));
    expect(within(dialog).getByText(/calendar isn't connected, so free time is unknown/)).toBeInTheDocument();
    fireEvent.click(stepButton(dialog, "Save"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Plan" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
  });

  it("previews the lines, saves once with Undo in the notice, copies a plain-text plan, and keeps choices when a save is refused", async () => {
    const { save, undo } = setup();
    save.mockRejectedValueOnce(new Error("That day's note changed. Open it before saving the plan."));
    const onNotice = vi.fn(), onSaved = vi.fn(), onOpenNote = vi.fn();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { dialog, trigger } = await openRoutine({ onNotice, onSaved, onOpenNote, dayName: "Tomorrow" });
    const title = within(dialog).getByRole("textbox", { name: "New Task" });
    fireEvent.change(title, { target: { value: "Draft memo" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add New Task" }));
    fireEvent.click(stepButton(dialog, "Save"));
    expect(within(dialog).getByText("- ( ) Draft memo")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Copy Plan" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(expect.stringMatching(/^Plan for .+\n\n1\. Draft memo\n$/)));

    fireEvent.click(within(dialog).getByRole("button", { name: "Save Plan" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/changed/);
    expect(within(dialog).getByText("- ( ) Draft memo")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Plan" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(save.mock.calls[1][2]).toBe(save.mock.calls[0][2]);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(document.activeElement).toBe(trigger);
    const saved = onNotice.mock.calls.find(([notice]) => notice.message === "Saved tomorrow's plan")![0];
    await act(async () => { saved.undo(); });
    await waitFor(() => expect(undo).toHaveBeenCalledWith(DAY, save.mock.calls[1][2]));
    expect(readRecovery(DAY)).toBeNull();
  });

  it("is titled for the day it plans", async () => {
    setup();
    const { dialog } = await openRoutine({ dayName: "Today", date: localDate(0) });
    expect(dialog).toHaveAccessibleName("Plan Today");
  });
});
