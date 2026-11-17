import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { planItemKey } from "./dailyPlan";
import { mailClient } from "./data/client";
import type { CalendarOption, DailyPlan, DailyPlanItem, MeetingReadiness, Note, OpenPromise, PlanReview, PlanReviewItem, ScheduleEvent, ThreadTask } from "./domain";
import { MAX_SHUTDOWN_NOTE_CHARS } from "./dailyPlan";
import { meetingEventReference } from "./meetingNoteEntry";
import { notesChanged } from "./noteMutations";
import { EveningReview, EveningTomorrow, type EveningReviewProps } from "./TodayEvening";

const TODAY = "2026-11-17";
const TOMORROW = "2026-11-18";
const NOW = new Date(2026, 10, 17, 19, 0);
const ref = (id: string) => ({ version: 1 as const, kind: "task" as const, id });
const planItem = (id: string, overrides: Partial<DailyPlanItem> = {}): DailyPlanItem => ({
  key: planItemKey(ref(id)), kind: "task", reference: ref(id), title: `Task ${id}`, state: "open", marker: null, taskId: id,
  accountId: "me@example.com", goal: null, contactId: null, carryCount: 0, start: 0, end: 0, ...overrides,
});
const loose = (id: string, overrides: Partial<PlanReviewItem> = {}): PlanReviewItem => ({ ...planItem(id), plannedOn: TODAY, ...overrides });
const plan = (overrides: Partial<DailyPlan> = {}): DailyPlan => ({ date: TODAY, noteId: `daily:${TODAY}`, revision: "r1", items: [], notToday: [], ...overrides });
const review = (overrides: Partial<PlanReview> = {}): PlanReview => ({ date: TOMORROW, reviewed: TODAY, done: [], alsoDone: [], meetings: null, didntGetTo: [], ...overrides });
const note = (updatedAt: string): Note => ({ id: `daily:${TODAY}`, title: TODAY, body: "", dailyDate: TODAY, createdAt: updatedAt, updatedAt, trashed: false, trashedAt: null });
const saveResult = (operationId = "op", shutdown = false) => ({ operationId, note: note("r2"), added: [], removed: [], notToday: [], already: [], changed: true, shutdown });
const meeting = (id: string, hours: [number, number], overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id, accountId: "me@example.com", calendarId: "work", title: `Meeting ${id}`, allDay: false,
  start: `2026-11-17T${String(hours[0]).padStart(2, "0")}:00:00`, end: `2026-11-17T${String(hours[1]).padStart(2, "0")}:00:00`, ...overrides,
});
const readiness = (event: ScheduleEvent, followThrough: "reviewed" | "notReviewed" | "unknown"): MeetingReadiness => ({
  reference: meetingEventReference(event)!, eventAvailable: true, hasOccurrenceNote: false, priorOccurrencesWithNotes: 0, seriesNotes: 0,
  openCommitments: 0, candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, followThrough, unavailableSections: [], error: null,
});
const promise = (id: string, sentAt: string): OpenPromise => ({
  thread: { id: `t-${id}`, accountId: "me@example.com", subject: `Subject ${id}` } as OpenPromise["thread"],
  messageId: id, sentAt, recipients: ["ann@example.org"], excerpt: `I will send ${id}`,
});
const completed = (id: string): ThreadTask => ({ id, accountId: "me@example.com", title: `Done ${id}`, status: "completed", completedAt: "2026-11-17T11:00:00" } as ThreadTask);

function renderReview(overrides: Partial<EveningReviewProps> = {}) {
  const handlers = {
    onNotice: vi.fn(), onOpenNote: vi.fn(), onOpenInbox: vi.fn(), onOpenThread: vi.fn(), onOpenPlanItem: vi.fn(), onOpenCompletedTask: vi.fn(), onOpenEvent: vi.fn(),
    onPrepareMeeting: vi.fn(), onMakeTask: vi.fn(), onDismissPromise: vi.fn(), onClosedChange: vi.fn(), reloadPlan: vi.fn(async () => {}),
  };
  const props: EveningReviewProps = {
    today: TODAY, tomorrow: TOMORROW, now: NOW, plan: plan(), refreshKey: "", events: null, completedToday: [], promises: [], needsReply: null, ...handlers, ...overrides,
  };
  render(<EveningReview {...props} />);
  return { ...handlers, region: within(screen.getByRole("group", { name: "Today in review" })) };
}
const lastNotice = (onNotice: ReturnType<typeof vi.fn>) => onNotice.mock.calls.at(-1)![0] as { message: string; undo?: () => void; action?: { label: string; run(): void } };

beforeEach(() => {
  vi.spyOn(mailClient, "planReview").mockResolvedValue(review());
  vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(plan());
  vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([]);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

describe("EveningReview: Done today", () => {
  it("lists the plan's finished items, meetings attended, and tasks done outside the plan, and opens their sources", async () => {
    const ended = meeting("a", [9, 10]);
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({
      done: [planItem("shipped", { state: "done", title: "Ship the deck" })],
      alsoDone: [{ key: "k", taskId: "side", title: "Fix the printer", accountId: null, noteId: null, noteTitle: null, completedAt: "2026-11-17T12:00:00" }],
    }));
    const { region, onOpenPlanItem, onOpenCompletedTask, onOpenEvent } = renderReview({
      events: [ended, meeting("later", [20, 21]), meeting("declined", [10, 11], { responseStatus: "declined" }), meeting("allday", [0, 23], { allDay: true }), meeting("office", [8, 18], { eventType: "workingLocation" })],
    });
    const done = within(region.getByRole("region", { name: "Done today" }));
    fireEvent.click(await done.findByRole("button", { name: "Ship the deck" }));
    expect(onOpenPlanItem).toHaveBeenCalledWith(expect.objectContaining({ title: "Ship the deck" }));
    fireEvent.click(done.getByRole("button", { name: "Fix the printer" }));
    expect(onOpenCompletedTask).toHaveBeenCalledWith(expect.objectContaining({ taskId: "side" }));
    fireEvent.click(done.getByRole("button", { name: "Meeting a" }));
    expect(onOpenEvent).toHaveBeenCalledWith(ended);
    for (const absent of ["Meeting later", "Meeting declined", "Meeting allday", "Meeting office"]) expect(done.queryByText(absent)).not.toBeInTheDocument();
  });

  it("shows tasks completed today when today has no plan to review, and says so when nothing is done", async () => {
    // The most recent plan is an earlier day's, so none of its items are today's.
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ reviewed: "2026-11-16", done: [planItem("old", { state: "done", title: "Yesterday's thing" })] }));
    const first = renderReview({ completedToday: [completed("c1")] });
    const done = within(screen.getByText("Done today").closest("section")!);
    expect(await done.findByRole("button", { name: "Done c1" })).toBeInTheDocument();
    expect(done.queryByText("Yesterday's thing")).not.toBeInTheDocument();
    expect(first.onOpenPlanItem).not.toHaveBeenCalled();
    cleanup();
    renderReview();
    expect(await screen.findByText("Nothing finished yet today.")).toBeInTheDocument();
  });
});

describe("EveningReview: loose-end decisions", () => {
  const row = loose("memo", { title: "Draft the memo" });
  beforeEach(() => {
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [row] }));
  });
  const entry = (decision: object) => ({ reference: ref("memo"), label: "Draft the memo", decision });

  it("carries to tomorrow as one guarded item-level write to today's note, and Undo restores exactly what it wrote", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult());
    const undo = vi.spyOn(mailClient, "undoDailyPlanSave").mockResolvedValue(note("r3"));
    const { region, onNotice } = renderReview();
    fireEvent.click(await region.findByRole("button", { name: "Carry to Tomorrow" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save).toHaveBeenCalledWith(TODAY, "r1", expect.any(String), { items: [], remove: [], notToday: [entry({ kind: "deferred", date: TOMORROW })] });
    await waitFor(() => expect(onNotice).toHaveBeenCalled());
    expect(lastNotice(onNotice).message).toBe("Carried “Draft the memo” to tomorrow");
    act(() => lastNotice(onNotice).undo!());
    await waitFor(() => expect(undo).toHaveBeenCalledWith(TODAY, save.mock.calls[0][2]));
  });

  it("drops with its marker, and each decision uses its own operation id", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult());
    const { region, onNotice } = renderReview();
    fireEvent.click(await region.findByRole("button", { name: "Drop" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][3]).toEqual({ items: [], remove: [], notToday: [entry({ kind: "dropped" })] });
    await waitFor(() => expect(onNotice).toHaveBeenCalled());
    expect(lastNotice(onNotice).message).toBe("Dropped “Draft the memo” from planning");
    fireEvent.click(await region.findByRole("button", { name: "Carry to Tomorrow" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(save.mock.calls[1][2]).not.toBe(save.mock.calls[0][2]);
  });

  it("asks for a Defer date in a small dialog that writes nothing until Defer, from tomorrow on", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult());
    const { region, onNotice } = renderReview();
    fireEvent.click(await region.findByRole("button", { name: "Defer…" }));
    const dialog = within(screen.getByRole("dialog", { name: "Defer" }));
    const input = dialog.getByLabelText("Defer Until");
    expect(input).toHaveFocus();
    expect(input).toHaveAttribute("min", TOMORROW);
    expect(input).toHaveAccessibleDescription();
    fireEvent.change(input, { target: { value: "2026-11-17" } });
    expect(dialog.getByRole("button", { name: "Defer" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "2026-11-23" } });
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(dialog.getByRole("button", { name: "Defer" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][3].notToday).toEqual([entry({ kind: "deferred", date: "2026-11-23" })]);
    expect(screen.queryByRole("dialog", { name: "Defer" })).not.toBeInTheDocument();
    await waitFor(() => expect(onNotice).toHaveBeenCalled());
    expect(lastNotice(onNotice).message).toMatch(/^Deferred “Draft the memo” to /);
  });

  it("closes the Defer dialog with Cancel or Escape without writing", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult());
    const { region } = renderReview();
    fireEvent.click(await region.findByRole("button", { name: "Defer…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "Defer" })).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Defer" })).not.toBeInTheDocument();
    fireEvent.click(region.getByRole("button", { name: "Defer…" }));
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Defer" })).not.toBeInTheDocument());
    expect(save).not.toHaveBeenCalled();
  });

  it("keeps the row and offers Open Note when the write is refused, and reports a refused Undo", async () => {
    vi.spyOn(mailClient, "saveDailyPlan").mockRejectedValueOnce(new Error("That day's note changed. Open it before saving the plan."))
      .mockResolvedValue(saveResult());
    vi.spyOn(mailClient, "undoDailyPlanSave").mockRejectedValue(new Error("That day's note changed after the plan was saved. Open Note to change it."));
    const { region, onNotice, onOpenNote } = renderReview();
    fireEvent.click(await region.findByRole("button", { name: "Carry to Tomorrow" }));
    await waitFor(() => expect(onNotice).toHaveBeenCalled());
    expect(lastNotice(onNotice).message).toMatch(/changed/);
    expect(lastNotice(onNotice).undo).toBeUndefined();
    act(() => lastNotice(onNotice).action!.run());
    expect(onOpenNote).toHaveBeenCalledWith(`daily:${TODAY}`);
    expect(region.getByText("Draft the memo")).toBeInTheDocument();

    fireEvent.click(region.getByRole("button", { name: "Carry to Tomorrow" }));
    await waitFor(() => expect(onNotice).toHaveBeenCalledTimes(2));
    act(() => lastNotice(onNotice).undo!());
    await waitFor(() => expect(onNotice).toHaveBeenCalledTimes(3));
    expect(lastNotice(onNotice).message).toMatch(/Open Note to change it/);
    expect(lastNotice(onNotice).action?.label).toBe("Open Note");
  });

  it("cannot carry an item that is unavailable on this device, but can drop it", async () => {
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [loose("gone", { state: "unavailable", title: "Gone task" })] }));
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult());
    const { region } = renderReview();
    expect(await region.findByRole("button", { name: "Carry to Tomorrow" })).toBeDisabled();
    expect(region.getByText("Unavailable on this device")).toBeInTheDocument();
    fireEvent.click(region.getByRole("button", { name: "Drop" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  });

  it("shows the carry count with the hint, and leaves out what today's note already decided (including a carry that comes back as due tomorrow)", async () => {
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [loose("again", { title: "Again", carryCount: 3 }), loose("decided", { title: "Already carried" })] }));
    const decided = planItem("decided", { marker: { kind: "deferred", date: TOMORROW } });
    const { region } = renderReview({ plan: plan({ notToday: [decided] }) });
    expect(await region.findByText("Carried 3 times. Defer or drop it?")).toBeInTheDocument();
    expect(region.queryByText("Already carried")).not.toBeInTheDocument();
  });

  it("reads the review again when a note changes, so a decision made elsewhere leaves the list", async () => {
    const read = vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [row] }));
    const { region } = renderReview();
    await region.findByText("Draft the memo");
    read.mockResolvedValue(review());
    await act(async () => { await notesChanged(`daily:${TODAY}`); });
    await waitFor(() => expect(region.queryByText("Draft the memo")).not.toBeInTheDocument());
    expect(read).toHaveBeenCalledWith(TOMORROW);
  });
});

describe("EveningReview: meetings, promises, and mail", () => {
  const first = meeting("a", [9, 10]);
  const second = meeting("b", [11, 12]);

  it("lists ended meetings with their follow-through state and offers Review Follow-through and No Follow-through", async () => {
    vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness(first, "reviewed"), readiness(second, "notReviewed")]);
    const { region, onPrepareMeeting, onNotice } = renderReview({ events: [first, second] });
    const list = within(await region.findByRole("region", { name: "Meeting follow-through" }));
    expect(await list.findByText("Reviewed")).toBeInTheDocument();
    expect(list.getByText("Not reviewed yet")).toBeInTheDocument();
    expect(list.getByRole("button", { name: "Review follow-through for Meeting b" })).toBeInTheDocument();
    expect(list.queryByRole("button", { name: "Review follow-through for Meeting a" })).not.toBeInTheDocument();
    fireEvent.click(list.getByRole("button", { name: "No follow-through for Meeting b" }));
    expect(list.getByText("No follow-through")).toBeInTheDocument();
    expect(lastNotice(onNotice).message).toBe("No follow-through for Meeting b");
    fireEvent.click(list.getByRole("button", { name: /Meeting a/ }));
    expect(onPrepareMeeting).toHaveBeenCalledWith(first);
  });

  it("lists only today's promises, with Make a Task and Dismiss", async () => {
    const mine = promise("p1", "2026-11-17T10:00:00");
    const { region, onMakeTask, onDismissPromise, onOpenThread } = renderReview({ promises: [mine, promise("old", "2026-11-14T10:00:00")] });
    const list = within(await region.findByRole("region", { name: "You said you would" }));
    expect(list.queryByText("I will send old")).not.toBeInTheDocument();
    fireEvent.click(list.getByRole("button", { name: "Make a Task" }));
    expect(onMakeTask).toHaveBeenCalledWith(mine);
    fireEvent.click(list.getByRole("button", { name: "Dismiss" }));
    expect(onDismissPromise).toHaveBeenCalledWith(mine);
    fireEvent.click(list.getByRole("button", { name: /I will send p1/ }));
    expect(onOpenThread).toHaveBeenCalledWith("t-p1");
  });

  it("shows needs-reply mail as one summary row that opens Inbox, and nothing at zero", async () => {
    const { region, onOpenInbox } = renderReview({ needsReply: 3 });
    fireEvent.click(await region.findByRole("button", { name: /3 conversations need a reply/ }));
    expect(onOpenInbox).toHaveBeenCalled();
    cleanup();
    renderReview({ needsReply: 1 });
    expect(await screen.findByRole("button", { name: /1 conversation needs a reply/ })).toBeInTheDocument();
    cleanup();
    renderReview({ needsReply: 0 });
    expect(await screen.findByText("No loose ends.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /need a reply|needs a reply/ })).not.toBeInTheDocument();
  });
});

describe("EveningReview: Add Notes on the Day…", () => {
  const open = async () => {
    const view = renderReview();
    fireEvent.click(await view.region.findByRole("button", { name: "Add Notes on the Day…" }));
    return { ...view, dialog: within(screen.getByRole("dialog", { name: "Notes on the Day" })) };
  };

  it("opens a small modal with focus in the notes, and writes nothing until it is added", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult("op", true));
    const { dialog } = await open();
    expect(dialog.getByLabelText("Notes")).toHaveFocus();
    expect(dialog.getByLabelText("Notes")).toHaveAccessibleDescription();
    expect(dialog.getByRole("button", { name: "Add to Today's Note" })).toBeDisabled();
    fireEvent.change(dialog.getByLabelText("Notes"), { target: { value: "Shipped the deck." } });
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Notes on the Day" })).not.toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
  });

  it("appends the text as a guarded Shutdown write with Undo, and Cmd+Enter submits", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue(saveResult("op", true));
    const undo = vi.spyOn(mailClient, "undoDailyPlanSave").mockResolvedValue(note("r3"));
    const { dialog, onNotice } = await open();
    const notes = dialog.getByLabelText("Notes");
    fireEvent.change(notes, { target: { value: "Shipped the deck.\nCall Sam tomorrow." } });
    fireEvent.keyDown(notes, { key: "Enter", metaKey: true });
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save).toHaveBeenCalledWith(TODAY, "r1", expect.any(String), { items: [], remove: [], notToday: [], shutdown: "Shipped the deck.\nCall Sam tomorrow." });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Notes on the Day" })).not.toBeInTheDocument());
    expect(lastNotice(onNotice).message).toBe("Added notes on the day to today's note");
    act(() => lastNotice(onNotice).undo!());
    await waitFor(() => expect(undo).toHaveBeenCalledWith(TODAY, save.mock.calls[0][2]));
  });

  it("refuses text over the limit without writing, and keeps the text with Open Note when the write is refused", async () => {
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockRejectedValue(new Error("That day's note changed. Open it before saving the plan."));
    const { dialog, onOpenNote } = await open();
    const notes = dialog.getByLabelText("Notes");
    fireEvent.change(notes, { target: { value: "x".repeat(MAX_SHUTDOWN_NOTE_CHARS + 2) } });
    expect(dialog.getByRole("alert")).toHaveTextContent("Shorten the notes by 2 characters.");
    expect(dialog.getByRole("button", { name: "Add to Today's Note" })).toBeDisabled();
    fireEvent.change(notes, { target: { value: "x".repeat(MAX_SHUTDOWN_NOTE_CHARS) } });
    fireEvent.click(dialog.getByRole("button", { name: "Add to Today's Note" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent(/changed/);
    expect(save).toHaveBeenCalledTimes(1);
    expect(notes).toHaveValue("x".repeat(MAX_SHUTDOWN_NOTE_CHARS));
    fireEvent.click(dialog.getByRole("button", { name: "Open Note" }));
    expect(onOpenNote).toHaveBeenCalledWith(`daily:${TODAY}`);
  });
});

describe("EveningReview: Day closed", () => {
  const ended = meeting("a", [9, 10]);
  const lastClosed = (fn: ReturnType<typeof vi.fn>) => fn.mock.calls.at(-1)?.[0];

  it("is closed when every ended meeting is reviewed and no planned item is undecided", async () => {
    vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness(ended, "reviewed")]);
    const { onClosedChange } = renderReview({ events: [ended] });
    await waitFor(() => expect(lastClosed(onClosedChange)).toBe(true));
  });

  it("is not closed while an item is undecided or a meeting unreviewed, and not on a guess while things load", async () => {
    vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [loose("memo")] }));
    vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness(ended, "notReviewed")]);
    const { onClosedChange } = renderReview({ events: [ended] });
    await screen.findByText("Task memo");
    await screen.findByText("Not reviewed yet");
    expect(onClosedChange).not.toHaveBeenCalledWith(true);
    expect(lastClosed(onClosedChange)).toBe(false);
  });

  it("treats a meeting set aside with No Follow-through as done, the one device-local input", async () => {
    vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness(ended, "notReviewed")]);
    const { region, onClosedChange } = renderReview({ events: [ended] });
    await region.findByText("Not reviewed yet");
    expect(lastClosed(onClosedChange)).toBe(false);
    fireEvent.click(region.getByRole("button", { name: "No follow-through for Meeting a" }));
    await waitFor(() => expect(lastClosed(onClosedChange)).toBe(true));
  });

  it("agrees with a decision or review made on another device once its note has synced", async () => {
    // The item and the meeting show as undecided, then today's note arrives from another device with both settled.
    const read = vi.spyOn(mailClient, "planReview").mockResolvedValue(review({ didntGetTo: [loose("memo")] }));
    const readyState = vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness(ended, "notReviewed")]);
    const view = renderReview({ events: [ended] });
    await screen.findByText("Task memo");
    await screen.findByText("Not reviewed yet");
    await waitFor(() => expect(lastClosed(view.onClosedChange)).toBe(false));
    read.mockResolvedValue(review());
    readyState.mockResolvedValue([readiness(ended, "reviewed")]);
    await act(async () => { await notesChanged(`daily:${TODAY}`); });
    await waitFor(() => expect(lastClosed(view.onClosedChange)).toBe(true));
  });

  it("is closed without a calendar once nothing is undecided", async () => {
    const view = renderReview();
    await waitFor(() => expect(lastClosed(view.onClosedChange)).toBe(true));
  });

  it("is unknown, not closed, when the follow-through state couldn't be checked", async () => {
    vi.spyOn(mailClient, "meetingReadiness").mockRejectedValue(new Error("offline"));
    const { onClosedChange } = renderReview({ events: [ended] });
    await screen.findByText(/couldn't be checked/);
    expect(onClosedChange).not.toHaveBeenCalledWith(true);
  });
});

describe("EveningTomorrow", () => {
  const calendars: CalendarOption[] = [{ id: "work", accountId: "me@example.com", name: "Work", primary: true, selected: true, writable: true }];
  const tomorrowMeeting: ScheduleEvent = { ...meeting("t1", [10, 11]), start: "2026-11-18T10:00:00", end: "2026-11-18T11:00:00" };
  const schedule = { events: [tomorrowMeeting], loading: false, error: null, reload: vi.fn() };
  const tomorrow = new Date(2026, 10, 18);

  function renderTomorrow(overrides: Partial<Parameters<typeof EveningTomorrow>[0]> = {}) {
    const handlers = { onChooseEvent: vi.fn(), onOpenPlanItem: vi.fn(), onPlanTomorrow: vi.fn(), onOpenCalendar: vi.fn() };
    render(<EveningTomorrow tomorrow={tomorrow} now={NOW} events={[tomorrowMeeting]} schedule={schedule} calendars={calendars} carried={[]}
      workingHours={{ timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, workingWindows: [{ weekday: 3, start: "09:00", end: "17:00" }] }} selectedEvent={null}
      eventDecoration={() => ({ description: "Notes · 1 open commitment", mark: <span data-testid="mark" /> })} gridRef={createRef<HTMLDivElement>()} preview={null}
      selectedKey={null} {...handlers} {...overrides} />);
    return { ...handlers, region: within(screen.getByRole("group", { name: "Tomorrow" })) };
  }

  it("lists what was carried to tomorrow above the day view, with Plan Tomorrow", () => {
    const carried = [planItem("memo", { title: "Draft the memo", marker: { kind: "deferred", date: TOMORROW } })];
    const { region, onOpenPlanItem, onPlanTomorrow } = renderTomorrow({ carried });
    const heading = region.getByRole("heading", { name: "Carried to tomorrow" });
    const dayView = region.getByRole("group", { name: "Tomorrow's day view" });
    expect(heading.compareDocumentPosition(dayView) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(region.getByRole("button", { name: "Draft the memo" }));
    expect(onOpenPlanItem).toHaveBeenCalledWith(carried[0]);
    fireEvent.click(region.getByRole("button", { name: "Plan Tomorrow" }));
    expect(onPlanTomorrow).toHaveBeenCalled();
  });

  it("shows tomorrow's meetings on the day view with readiness, and choosing one prepares it", () => {
    const { region, onChooseEvent } = renderTomorrow();
    expect(region.getByText("Nothing carried to tomorrow yet.")).toBeInTheDocument();
    expect(region.getByTestId("mark")).toBeInTheDocument();
    const block = within(region.getByRole("group", { name: "Tomorrow's day view" })).getByRole("button", { name: /Meeting t1/ });
    expect(block).toHaveAccessibleDescription(/1 open commitment/);
    fireEvent.click(block);
    expect(onChooseEvent).toHaveBeenCalledWith(tomorrowMeeting, expect.anything());
  });

  it("without a calendar still offers Plan Tomorrow and says so", () => {
    const { region } = renderTomorrow({ schedule: null, events: [] });
    expect(region.getByText(/Connect a calendar/)).toBeInTheDocument();
    expect(region.getByRole("button", { name: "Plan Tomorrow" })).toBeInTheDocument();
  });
});
