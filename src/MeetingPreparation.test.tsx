import { act, cleanup, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { MeetingContext, MeetingReadiness, MeetingSection, ScheduleEvent } from "./domain";
import { MeetingPreparation } from "./MeetingPreparation";
import { MeetingReadinessList } from "./MeetingReadinessList";
import { MAX_NO_PREP_CHOICES, meetingsToPrepare, readinessSummary, readNoPrepNeeded, setNoPrepNeeded } from "./meetingPrep";
import { meetingEventReference } from "./meetingNoteEntry";
import { NOTES_CHANGED } from "./noteMutations";
import { noteReferenceHref, type NoteReference } from "./noteEntities";
import { useMeetingPreparation } from "./useMeetingPreparation";
import { MeetingPrepActions } from "./meetingPrepActions";

afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

const ME = "me@example.com";
const start = new Date(Date.now() + 60 * 60_000);
const meeting = (id = "evt", overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id, accountId: ME, calendarId: "work", title: `Meeting ${id}`, allDay: false,
  start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString(), attendees: ["ann@example.org"], ...overrides,
});
const reference = (id = "evt") => meetingEventReference(meeting(id))!;
const section = <T,>(items: T[], extra: Partial<MeetingSection<T>> = {}): MeetingSection<T> => ({ status: "ready", items, omitted: 0, message: null, ...extra });
const limits = { noteSources: 20, priorOccurrences: 8, excerptBytes: 4096, linkedNotePreviewBytes: 1024, attendees: 50, conversations: 10, mailLookbackDays: 90, commitments: 25 };

function context(overrides: Partial<MeetingContext> = {}, ref: NoteReference = reference()): MeetingContext {
  return {
    reference: ref,
    event: { title: "Meeting evt", start: start.toISOString(), end: start.toISOString(), allDay: false, seriesId: "series", attendees: ["ann@example.org"], attendeesOmitted: 0, sourceMessageId: null, responseStatus: null },
    mailScope: [ME],
    thisMeeting: section([{ noteId: "daily:2026-11-16", title: "2026-11-16", dailyDate: "2026-11-16", revision: "r1", kind: "excerpt", text: "- [3pm](x) for Sync\n  - Decided to ship <!-- threestrands-task:1 -->", truncated: false, start: 0, end: 10 }]),
    previousMeetings: section([{ reference: { ...ref, id: "earlier" }, start: "2026-11-09T15:00:00Z", date: null, sources: [{ noteId: "daily:2026-11-09", title: "2026-11-09", dailyDate: "2026-11-09", revision: "r0", kind: "excerpt", text: "- Last week's notes", truncated: false, start: 0, end: 5 }] }]),
    seriesNotes: section([]),
    conversations: section([{ threadId: "t1", accountId: ME, subject: "Agenda thread", lastMessageAt: "2026-11-14T09:00:00Z", reason: "attendees", attendees: ["ann@example.org"] }]),
    commitments: section([
      { id: "note:daily:2026-11-16:m1", title: "Send the deck", status: "open", noteId: "daily:2026-11-16", threadId: null, accountId: null, origin: "meetingNote", candidate: false },
      { id: "task-2", title: "Send Ann numbers", status: "open", noteId: null, threadId: "t1", accountId: ME, origin: "attendeeConversation", candidate: true },
    ]),
    limits,
    ...overrides,
  };
}

function renderPrep(overrides: Partial<Parameters<typeof MeetingPreparation>[0]> = {}) {
  const handlers = { onOpenNote: vi.fn(), onOpenThread: vi.fn(), onOpenTask: vi.fn(), onClose: vi.fn() };
  render(<MeetingPreparation event={meeting()} mailAccounts={[{ email: ME, label: ME }, { email: "other@example.com", label: "Other" }]} {...handlers} {...overrides} />);
  return handlers;
}

describe("useMeetingPreparation", () => {
  it("ignores a late result for a meeting the view has left, and resets the mail scope per meeting", async () => {
    let resolveA: (value: MeetingContext) => void = () => {};
    const load = vi.spyOn(mailClient, "meetingContext").mockImplementation((ref) => ref.id === "a"
      ? new Promise((resolve) => { resolveA = resolve; })
      : Promise.resolve(context({}, ref)));
    const { result, rerender } = renderHook(({ ref }) => useMeetingPreparation(ref), { initialProps: { ref: reference("a") } });
    act(() => result.current.setMailScope(["other@example.com"]));
    rerender({ ref: reference("b") });
    await waitFor(() => expect(result.current.context?.reference.id).toBe("b"));
    await act(async () => resolveA(context({}, reference("a"))));
    expect(result.current.context?.reference.id).toBe("b");
    expect(load).toHaveBeenLastCalledWith(reference("b"), null);
    expect(result.current.mailScope).toBeNull();
  });

  it("reloads when notes change and when the mail scope changes", async () => {
    const load = vi.spyOn(mailClient, "meetingContext").mockImplementation(async (ref) => context({}, ref));
    const { result } = renderHook(() => useMeetingPreparation(reference()));
    await waitFor(() => expect(result.current.context).not.toBeNull());
    await act(async () => { window.dispatchEvent(new CustomEvent(NOTES_CHANGED, { detail: "daily:2026-11-16" })); });
    await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
    act(() => result.current.setMailScope([ME, "other@example.com"]));
    await waitFor(() => expect(load).toHaveBeenLastCalledWith(reference(), [ME, "other@example.com"]));
  });
});

describe("MeetingPreparation", () => {
  it("lists each section and opens its source, without taking focus", async () => {
    vi.spyOn(mailClient, "meetingContext").mockResolvedValue(context());
    const handlers = renderPrep();
    const view = within(await screen.findByRole("region", { name: "Preparation for Meeting evt" }));
    await view.findByText(/Decided to ship/);
    expect(document.activeElement).toBe(document.body);
    expect(view.queryByText(/threestrands-task/)).not.toBeInTheDocument();

    fireEvent.click(view.getByRole("button", { name: /Decided to ship/ }));
    expect(handlers.onOpenNote).toHaveBeenCalledWith("daily:2026-11-16", noteReferenceHref(reference()));
    fireEvent.click(view.getByRole("button", { name: /Last week's notes/ }));
    expect(handlers.onOpenNote).toHaveBeenLastCalledWith("daily:2026-11-09", noteReferenceHref({ ...reference(), id: "earlier" }));
    fireEvent.click(view.getByRole("button", { name: /Agenda thread/ }));
    expect(handlers.onOpenThread).toHaveBeenCalledWith("t1");

    const commitments = within(view.getByRole("region", { name: "Open commitments" }));
    fireEvent.click(commitments.getByRole("button", { name: /Send the deck/ }));
    expect(handlers.onOpenTask).toHaveBeenCalledWith(expect.objectContaining({ id: "note:daily:2026-11-16:m1" }));
    expect(commitments.queryByText("Send Ann numbers")).not.toBeInTheDocument();
    expect(within(view.getByRole("region", { name: "Possibly related" })).getByText("Send Ann numbers")).toBeInTheDocument();

    fireEvent.click(view.getByRole("button", { name: "Close Preparation" }));
    expect(handlers.onClose).toHaveBeenCalled();
  });

  it("asks for mail accounts when the calendar has none, and looks only in the chosen ones", async () => {
    const load = vi.spyOn(mailClient, "meetingContext").mockImplementation(async (ref, scope) => scope
      ? context({ mailScope: scope }, ref)
      : context({ mailScope: [], conversations: section([], { status: "scopeRequired", message: "Choose which mail accounts to look in for related mail." }) }, ref));
    renderPrep();
    const view = within(await screen.findByRole("region", { name: "Preparation for Meeting evt" }));
    expect(await view.findByText("Choose which mail accounts to look in for related mail.")).toBeInTheDocument();
    fireEvent.click(view.getByRole("button", { name: "Choose Accounts…" }));
    const dialog = within(await screen.findByRole("dialog", { name: "Mail Accounts for This Meeting" }));
    expect(dialog.getByRole("button", { name: "Look in These Accounts" })).toBeDisabled();
    fireEvent.click(dialog.getByRole("checkbox", { name: "Other" }));
    fireEvent.click(dialog.getByRole("button", { name: "Look in These Accounts" }));
    await waitFor(() => expect(load).toHaveBeenLastCalledWith(reference(), ["other@example.com"]));
    expect(await view.findByText(/Looking in other@example.com/)).toBeInTheDocument();
  });

  it("explains unavailable, empty, and cut sections and keeps notes for a meeting missing from the cache", async () => {
    vi.spyOn(mailClient, "meetingContext").mockResolvedValue(context({
      event: null,
      previousMeetings: section([], { status: "unavailable", message: "Earlier meetings can't be ordered until this meeting's time is on this device." }),
      conversations: section([], { status: "unavailable", message: "Related mail needs this meeting's attendees, which aren't on this device." }),
      commitments: section([], { message: "Only commitments in this meeting's notes are shown." }),
      thisMeeting: section(context().thisMeeting.items, { status: "limited", omitted: 3 }),
    }));
    renderPrep();
    const view = within(await screen.findByRole("region", { name: "Preparation for Meeting evt" }));
    expect(await view.findByText(/only its notes are shown/)).toBeInTheDocument();
    expect(view.getByText(/Decided to ship/)).toBeInTheDocument();
    expect(view.getByText("3 more not shown.")).toBeInTheDocument();
    expect(view.getByText(/Earlier meetings can't be ordered/)).toBeInTheDocument();
    expect(view.getByText(/Related mail needs this meeting's attendees/)).toBeInTheDocument();
  });

  it("leads with mail naming the meeting or several attendees, keeps one-attendee mail collapsed, and offers Add Note and the actions", async () => {
    const base = context();
    vi.spyOn(mailClient, "meetingContext").mockResolvedValue(context({
      event: { ...base.event!, attendees: ["ann@example.org", "bo@example.org"] },
      thisMeeting: section([]),
      conversations: section([
        { threadId: "inv", accountId: ME, subject: "Invitation: Meeting evt", lastMessageAt: "2026-11-12T09:00:00Z", reason: "title", attendees: [] },
        { threadId: "both", accountId: ME, subject: "Budget for the review", lastMessageAt: "2026-11-13T09:00:00Z", reason: "attendees", attendees: ["ann@example.org", "bo@example.org"] },
        { threadId: "solo", accountId: ME, subject: "Lunch on Friday", lastMessageAt: "2026-11-15T09:00:00Z", reason: "attendees", attendees: ["ann@example.org"] },
      ]),
    }));
    const onAddNote = vi.fn();
    renderPrep({ onAddNote, actions: <button type="button">Reserve Prep Time</button> });
    const view = within(await screen.findByRole("region", { name: "Preparation for Meeting evt" }));
    const related = within(await view.findByRole("region", { name: "Conversations" }));
    expect(related.getByRole("button", { name: /Invitation: Meeting evt/ })).toHaveTextContent("Names this meeting");
    expect(related.getByRole("button", { name: /Budget for the review/ })).toBeInTheDocument();
    expect(related.queryByText("Lunch on Friday")).not.toBeInTheDocument();
    const other = view.getByRole("region", { name: "Other mail with attendees" });
    expect(within(other).getByText("Lunch on Friday")).not.toBeVisible();
    fireEvent.click(within(other).getByRole("button", { name: /Other mail with attendees/ }));
    expect(within(other).getByText("Lunch on Friday")).toBeVisible();

    expect(within(view.getByRole("group", { name: "Meeting actions" })).getByRole("button", { name: "Reserve Prep Time" })).toBeInTheDocument();
    fireEvent.click(within(view.getByRole("region", { name: "This meeting" })).getByRole("button", { name: "Add Note" }));
    expect(onAddNote).toHaveBeenCalled();
  });

  it("offers Try Again after a failure", async () => {
    const load = vi.spyOn(mailClient, "meetingContext").mockRejectedValueOnce(new Error("Database busy")).mockResolvedValue(context());
    renderPrep();
    fireEvent.click(await screen.findByRole("button", { name: "Try Again" }));
    await screen.findByText(/Decided to ship/);
    expect(load).toHaveBeenCalledTimes(2);
  });
});

describe("MeetingPrepActions", () => {
  it("offers Add Note until the meeting has a note, then Open Note, with Reserve Prep Time, Join, and Open in Calendar", async () => {
    const handlers = { onAddNote: vi.fn(), onReservePrep: vi.fn(), onOpenInCalendar: vi.fn() };
    const online = meeting("evt", { conferenceUrl: "https://meet.example.com/x" });
    const { unmount } = render(<MeetingPrepActions event={online} {...handlers} />);
    fireEvent.click(await screen.findByRole("button", { name: "Add Note" }));
    fireEvent.click(screen.getByRole("button", { name: "Reserve Prep Time" }));
    fireEvent.click(screen.getByRole("button", { name: "Open in Calendar" }));
    expect(screen.getByRole("button", { name: "Join" })).toBeInTheDocument();
    expect(handlers.onAddNote).toHaveBeenCalled();
    expect(handlers.onReservePrep).toHaveBeenCalled();
    expect(handlers.onOpenInCalendar).toHaveBeenCalled();
    unmount();

    await mailClient.createNote("Notes for evt", `- [Meeting](${noteReferenceHref(reference())})`);
    render(<MeetingPrepActions event={meeting()} onAddNote={vi.fn()} showJoin={false} />);
    expect(await screen.findByRole("button", { name: "Open Note" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Join" })).not.toBeInTheDocument();
  });
});

describe("meeting readiness", () => {
  const readiness = (overrides: Partial<MeetingReadiness> = {}): MeetingReadiness => ({
    reference: reference(), eventAvailable: true, hasOccurrenceNote: true, priorOccurrencesWithNotes: 2, seriesNotes: 0,
    openCommitments: 1, candidateCommitments: 0, latestCorrespondenceAt: "2026-11-14T09:00:00Z", scopeRequired: false, unavailableSections: [], error: null, ...overrides,
  });

  it("describes readiness as facts, not a score", () => {
    expect(readinessSummary(readiness())).toMatch(/^Has notes · 2 earlier meetings with notes · 1 open commitment · Last mail /);
    expect(readinessSummary(readiness({ hasOccurrenceNote: false, priorOccurrencesWithNotes: 0, openCommitments: 0, scopeRequired: true }))).toBe("No notes yet · Choose mail accounts");
    expect(readinessSummary(readiness({ error: "x" }))).toBe("Couldn't check this meeting");
  });

  it("lists meetings still to come that a note can link, without declined ones or working locations", () => {
    const now = new Date();
    const ended = meeting("ended", { start: new Date(now.getTime() - 3_600_000).toISOString(), end: new Date(now.getTime() - 1_800_000).toISOString() });
    const declined = meeting("declined", { responseStatus: "declined" });
    const location = meeting("loc", { eventType: "workingLocation" });
    const noCalendar = { ...meeting("bare"), calendarId: undefined };
    expect(meetingsToPrepare([meeting("a"), ended, declined, location, noCalendar], now).map((event) => event.id)).toEqual(["a"]);
  });

  it("keeps No Prep Needed choices on this device up to the limit", () => {
    for (let n = 0; n <= MAX_NO_PREP_CHOICES; n++) setNoPrepNeeded(`key-${n}`, true);
    const kept = readNoPrepNeeded();
    expect(kept).toHaveLength(MAX_NO_PREP_CHOICES);
    expect(kept).not.toContain("key-0");
    expect(setNoPrepNeeded(`key-${MAX_NO_PREP_CHOICES}`, false)).not.toContain(`key-${MAX_NO_PREP_CHOICES}`);
  });

  it("shows each meeting's readiness, prepares, skips with undo, and reserves prep time", async () => {
    vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([readiness()]);
    vi.spyOn(mailClient, "listContactProfiles").mockResolvedValue([]);
    const reserve = vi.spyOn(mailClient, "reserveMeetingPrep");
    const handlers = { onPrepare: vi.fn(), onOpenNote: vi.fn(), onNotice: vi.fn() };
    render(<MeetingReadinessList events={[meeting()]} ownAddresses={[ME]} {...handlers} />);
    const list = within(screen.getByRole("region", { name: /Meeting prep/ }));
    expect(await list.findByText(/Has notes · 2 earlier meetings with notes/)).toBeInTheDocument();

    fireEvent.click(list.getByRole("button", { name: /^Meeting evt/ }));
    expect(handlers.onPrepare).toHaveBeenCalledWith(meeting());

    fireEvent.click(list.getByRole("button", { name: "No prep needed for Meeting evt" }));
    expect(list.getByText("No prep needed")).toBeInTheDocument();
    expect(readNoPrepNeeded()).toHaveLength(1);
    act(() => handlers.onNotice.mock.calls.at(-1)![0].undo());
    expect(list.queryByText("No prep needed")).not.toBeInTheDocument();
    expect(readNoPrepNeeded()).toHaveLength(0);

    fireEvent.click(list.getByRole("button", { name: "Reserve prep time for Meeting evt" }));
    await waitFor(() => expect(reserve).toHaveBeenCalled());
    const [, , entry] = reserve.mock.calls[0]!;
    expect(entry).toMatchObject({ name: "Meeting evt", linkNote: false, event: reference() });
    expect(entry.attendees.map((attendee) => attendee.reference.id)).toEqual(["ann@example.org"]);
    await waitFor(() => expect(handlers.onNotice).toHaveBeenLastCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Prep task added to /) })));
    fireEvent.click(list.getByRole("button", { name: "Reserve prep time for Meeting evt" }));
    await waitFor(() => expect(handlers.onNotice).toHaveBeenLastCalledWith(expect.objectContaining({ message: "This meeting already has a prep task" })));
    expect(reserve).toHaveBeenCalledTimes(2);
  });
});
