import { AccountScopeChip } from "./AccountScope";
import { formatCompactDate } from "./dateFormatting";
import { CalendarDays, CalendarPlus, ChevronLeft, ChevronRight, ContactRound, ListChecks, ListPlus, Mail, NotebookPen, Send, Star, Plus, X } from "lucide-react";
import { meetingEventReference } from "./meetingNoteEntry";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { isPreparable, meetingsToPrepare, preparationKey, readinessByKey, readinessSummary } from "./meetingPrep";
import { useMeetingReadiness } from "./useMeetingPreparation";
import { useWorkspaceCommands } from "./workspaceCommands";
import type { Notice } from "./useNotice";
import { isWorkingLocation } from "./WorkingLocation";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { Account, CalendarAccount, CalendarOption, ContactProfile, DailyPlanItem, OpenPromise, PlanCandidate as SourceCandidate, PlanSourceKind, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { agendaEventsFor, formatClockTime, isDeclined, isPastEvent } from "./agendaEvents";
import { addDays, dateInputValue, eventDate, formatEventDate, isSameDay, startOfLocalDay } from "./calendarTime";
import { errorMessage, logBackgroundFailure } from "./errors";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { dismissPromise, promiseRecipients, readDismissedPromises } from "./openPromises";
import { compareTasksForDisplay, formatDue, isActiveTaskStatus, isOverdue } from "./taskViews";
import { meetingsWith } from "./todayInsights";
import { TodayBackdrop } from "./TodayBackdrop";
import { skyPhase, useTodayBackdrop } from "./backdropPreference";
import { parseAddress } from "./emailAddress";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { normalizeContactAddress, type NoteReference } from "./noteEntities";
import { addToDailyPlan, removeFromDailyPlan } from "./noteCapture";
import { planItemKey } from "./dailyPlan";
import { changeNoteItem } from "./noteMutations";
import { todayTarget } from "./noteDrafts";
import { bookedTaskFor, TASK_BLOCK_MINUTES } from "./taskBooking";
import { bookBlock, nextFreeSlot, withinWorkingHours, type BlockTarget } from "./todayBooking";
import { bookedSource } from "./taskBooking";
import type { WorkingHours } from "./todayInsights";
import { usePlanSourcePage } from "./usePlanSourcePage";
import type { TaskFormRequest } from "./useTaskActions";
import { useDailyPlan } from "./useDailyPlan";
import { useTodayCompletedNotes } from "./useTodayCompletedNotes";
import { useDoneOnDay } from "./useDoneOnDay";
import { TodayCompletionCheckbox, useTodayCompletionTransition } from "./useTodayCompletionTransition";
import { DoneCard, uniqueDoneEntries, type DoneEntry } from "./DoneCard";
import { activeDayChoice, clearDayChoice, localDay, readDayChoice, writeDayChoice, type DayChoice } from "./todayDayChoice";
import { useTaskBlockDrag } from "./useTaskBlockDrag";
import { TaskDragChip } from "./TaskDragChip";
import { isFinished, itemBlock, carriedLabel, linkKey, type PlanCandidate, type PlanDragSource } from "./TodayPlan";
import { PlanningInbox, uniqueInboxEntries, interleaveInboxCategories, type InboxCategory, type InboxEntry, type InboxFilter } from "./PlanningInbox";
import { TodayDayView } from "./TodayDayView";
import { EventNoteAction } from "./NoteEntityContext";
import { refreshSyncedData } from "./refreshSyncedData";
import { usePullToRefresh } from "./usePullToRefresh";
import { PullToRefreshStatus } from "./PullToRefreshStatus";

import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { TaskEditorDialog } from "./TaskEditorDialog";
import { TodayDailyNote, type TodayDailyNoteHandle } from "./TodayDailyNote";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { formatShortcut } from "./shortcutLabels";

type Schedule = ReturnType<typeof useCalendarSchedule>;

export type TodayWorkspaceProps = {
  /** The mail account in view, or null for all accounts; scopes mail and tasks. */
  accountId: string | null;
  /** How many mail accounts are configured; the account in view is named only when there is more than one. */
  mailAccountCount?: number;
  /** Changes when mail syncs, so the unread list is read again. */
  refreshKey: string;
  taskRefreshKey: number;
  calendarConnected: boolean;
  calendars: CalendarOption[];
  /** Connected calendar accounts, for where a booked block can go. */
  calendarAccounts?: CalendarAccount[];
  onOpenThread(threadId: string): void;
  onOpenEvent(event: ScheduleEvent): void;
  /** Add Note for an event: its line in that day's note. */
  onAddNote?(event: ScheduleEvent): void;
  onEditTask(task: ThreadTask): void;
  onTasksChanged(): void;
  onOpenCalendar(day: Date): void;
  onEventCreated?(): void;
  onOpenContact(contactId: string): void;
  /** Opens a note, such as today's note when a plan write was refused. */
  onOpenNote?(noteId: string): void;
  onOpenEntity?(href: string): Promise<void>;
  /** Says what a plan change or booking did, with Undo. */
  onNotice?(notice: Notice): void;
  /** The default availability profile's working hours, which bound Book's free time; unknown without them. */
  workingHours?: WorkingHours | null;
  /** Opens the task editor for a new task, resolving with the task once saved, or null when cancelled. */
  onCreateTask?(request: TaskFormRequest): Promise<ThreadTask | null>;
  mailAccounts?: readonly Account[];
};

const taskReference = (id: string): NoteReference => ({ version: 1, kind: "task", id });

/** A note task's owning note and marker, from its Tasks id (`note:<noteId>:<markerId>`), as Calendar reads it. */
function noteTaskParts(taskId: string | null): { noteId: string; markerId: string } | null {
  const source = bookedSource(taskId);
  return source?.kind === "noteTask" ? { noteId: source.noteId, markerId: source.markerId } : null;
}

export function TodayWorkspace(props: TodayWorkspaceProps) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  const start = startOfLocalDay(now);
  const today = dateInputValue(start);
  // The picked day outlives leaving Today for another workspace, but not the day it was picked on.
  const [choice, setChoice] = useState<DayChoice | null>(() => readDayChoice(today));
  const active = activeDayChoice(choice, today);
  useEffect(() => {
    if (choice && !active && choice.chosenOn !== today) { clearDayChoice(); setChoice(null); }
  }, [choice, active, today]);
  const chosenDay = active ? localDay(active.day) : null;
  const changeDay = useCallback((next: Date | null) => {
    const picked = next && dateInputValue(next) !== today ? dateInputValue(next) : null;
    if (!picked) { clearDayChoice(); setChoice(null); return; }
    const stored = { chosenOn: today, day: picked };
    writeDayChoice(stored); setChoice(stored);
  }, [today]);
  const day = chosenDay ?? start;
  return props.calendarConnected
    ? <ScheduledToday {...props} now={now} day={day} onChangeDay={changeDay} />
    : <TodayLayout {...props} now={now} schedule={null} scheduleDay={day} onChangeDay={changeDay} />;
}

function ScheduledToday({ now, day, onChangeDay, ...props }: TodayWorkspaceProps & { now: Date; day: Date; onChangeDay(day: Date | null): void }) {
  const start = startOfLocalDay(now);
  const showingToday = isSameDay(day, start);
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const schedule = useCalendarSchedule({ timeMin: start.toISOString(), timeMax: addDays(start, 1).toISOString(), timeZone });
  const otherDay = useCalendarSchedule({ timeMin: day.toISOString(), timeMax: addDays(day, 1).toISOString(), timeZone }, false, !showingToday);
  return <TodayLayout {...props} now={now} schedule={schedule} displaySchedule={showingToday ? schedule : otherDay} scheduleDay={day} onChangeDay={onChangeDay} />;
}

function TodayLayout({
  accountId, mailAccountCount = 1, refreshKey, taskRefreshKey, calendars, calendarAccounts = [], now, schedule,
  displaySchedule = schedule, scheduleDay, onChangeDay,
  onOpenThread, onOpenEvent, onAddNote, onEditTask, onTasksChanged, onOpenCalendar, onEventCreated, onOpenContact,
  onOpenNote, onOpenEntity, onNotice, workingHours = null, onCreateTask, mailAccounts = [],
}: TodayWorkspaceProps & {
  now: Date;
  schedule: Schedule | null;
  displaySchedule?: Schedule | null;
  scheduleDay?: Date;
  onChangeDay?(day: Date | null): void;
}) {
  const shownDay = scheduleDay ?? now;
  const showingToday = isSameDay(shownDay, now);
  const [panel, setPanel] = useLocalStorageState<"agenda" | "note">("threestrands.today.panel", () => readLocalStorage("threestrands.today.panel", (raw) => raw === "note" ? "note" : "agenda", "agenda"));
  const [noteVisited, setNoteVisited] = useState(panel === "note");
  const dailyNote = useRef<TodayDailyNoteHandle>(null);
  const agendaPanel = useRef<HTMLDivElement>(null);
  const notePanel = useRef<HTMLDivElement>(null);
  const focusShownPanel = useRef(false);
  // Switching never focuses a control. Focus moves only when it sat inside the panel being hidden, so it is not lost.
  const selectPanel = (next: "agenda" | "note") => {
    if (next === panel) return;
    if (next === "note") setNoteVisited(true);
    const hiding = (panel === "agenda" ? agendaPanel : notePanel).current;
    focusShownPanel.current = Boolean(hiding?.contains(document.activeElement));
    setPanel(next);
  };
  useEffect(() => {
    if (!focusShownPanel.current) return;
    focusShownPanel.current = false;
    (panel === "agenda" ? agendaPanel : notePanel).current?.focus();
  }, [panel]);
  const otherPanel = panel === "agenda" ? "note" : "agenda";
  const otherName = otherPanel === "note" ? "Daily Note" : "Agenda";

  const pageRef = useRef<HTMLElement>(null);
  const completion = useTodayCompletionTransition(pageRef, `${accountId}:${dateInputValue(shownDay)}`, showingToday);
  const planDate = todayTarget(now).date;
  const accountRef = useRef(accountId);
  accountRef.current = accountId;
  const loadVersion = useRef(0);
  const [mail, setMail] = useState<{ threads: TodayMail[]; total: number } | null>(null);
  const [inboxRevision, setInboxRevision] = useState(0);
  const [loadingSources, setLoadingSources] = useState(true);
  const [sourceErrors, setSourceErrors] = useState<{ tasks?: string; promises?: string; contacts?: string }>({});
  const [loadedTasks, setTasks] = useState<ThreadTask[] | null>(null);
  const tasks = loadedTasks ?? [];
  const [announcement, setAnnouncement] = useState("");
  const [reconnect, setReconnect] = useState<ContactProfile[]>([]);
  const [promises, setPromises] = useState<OpenPromise[]>([]);
  /** Every completed task with a completion time; the Done card keeps the ones finished on the shown day. */
  const [completedTasks, setCompletedTasks] = useState<ThreadTask[]>([]);
  const [dismissed, setDismissed] = useState(readDismissedPromises);
  const load = useCallback(async () => {
    const version = ++loadVersion.current;
    setLoadingSources(true);
    const results = await Promise.allSettled([
      mailClient.listTodayMail(accountId ?? undefined),
      mailClient.listTasks(accountId ?? undefined),
      mailClient.listKeepInTouch(),
      mailClient.listOpenPromises(accountId ?? undefined),
    ]);
    if (version !== loadVersion.current || accountRef.current !== accountId) return;
    setLoadingSources(false);
    const [mailResult, taskResult, contactResult, promiseResult] = results;
    setSourceErrors({
      tasks: taskResult.status === "rejected" ? errorMessage(taskResult.reason) : undefined,
      promises: promiseResult.status === "rejected" ? errorMessage(promiseResult.reason) : undefined,
      contacts: contactResult.status === "rejected" ? errorMessage(contactResult.reason) : undefined,
    });
    if (mailResult.status === "fulfilled") setMail(mailResult.value);
    if (taskResult.status === "fulfilled") {
      setTasks(taskResult.value.filter((task) => isActiveTaskStatus(task.status)).sort(compareTasksForDisplay));
      setCompletedTasks(taskResult.value.filter((task) => task.status === "completed" && task.completedAt));
    }
    if (contactResult.status === "fulfilled") setReconnect(contactResult.value.filter((profile) => isKeepInTouchDue(profile))
      .sort((left, right) => (left.keepInTouchDueAt ?? "").localeCompare(right.keepInTouchDueAt ?? "")));
    if (promiseResult.status === "fulfilled") setPromises(promiseResult.value);
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") throw failed.reason;
  }, [accountId]);
  const cancelLoads = useCallback(() => { loadVersion.current++; }, []);
  useEffect(() => {
    void load().catch(logBackgroundFailure("Today refresh"));
    return cancelLoads;
  }, [load, refreshKey, taskRefreshKey, cancelLoads]);

  const { plan, error: planError, reload: reloadPlan } = useDailyPlan(planDate, `${refreshKey}:${taskRefreshKey}`);

  const reloadSchedule = schedule?.refresh;
  const reloadShownSchedule = showingToday ? undefined : displaySchedule?.refresh;
  const refreshToday = useCallback(async () => {
    const results = await Promise.allSettled([
      mailClient.listAccounts().then((accounts) => accounts.length ? mailClient.sync(accountId ? { accountId, catchUp: false } : { catchUp: false }) : undefined),
      refreshSyncedData(),
      reloadSchedule?.(),
      reloadShownSchedule?.(),
    ]);
    if (accountRef.current !== accountId) return;
    try { await mailClient.reconcileTasks(); } finally {
      await Promise.all([load(), reloadPlan()]).finally(() => {
        if (accountRef.current === accountId) setInboxRevision((value) => value + 1);
      });
    }
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") throw failed.reason;
  }, [accountId, reloadSchedule, reloadShownSchedule, load, reloadPlan]);
  const pull = usePullToRefresh(pageRef, refreshToday);
  const openPromises = promises.filter((promise) => !dismissed.includes(promise.messageId));

  const notice = (next: Notice) => { if (onNotice) onNotice(next); else setAnnouncement(next.message); };
  const openNoteAction = plan && onOpenNote ? { label: "Open Note", run: () => onOpenNote(plan.noteId) } : undefined;
  const planWriteFailed = (reason: unknown) => notice({ message: errorMessage(reason), action: openNoteAction });

  /** The existing Make a Task flow: an action task from the promise, opened for editing. */
  const trackPromise = async (promise: OpenPromise, edit = true): Promise<ThreadTask | null> => {
    try {
      const created = await mailClient.createTask({
        accountId: promise.thread.accountId,
        threadId: promise.thread.id,
        sourceMessageId: promise.messageId,
        subjectSnapshot: promise.thread.subject || "(no subject)",
        title: promise.excerpt,
        kind: "action",
        evidenceText: promise.excerpt,
      });
      // The task now tracks it, so the native list leaves it out from here on.
      setPromises((current) => current.filter((candidate) => candidate.messageId !== promise.messageId));
      setTasks((current) => [...(current ?? []).filter((task) => task.id !== created.id), created].sort(compareTasksForDisplay));
      setAnnouncement(`Task added: ${created.title}`);
      onTasksChanged();
      if (edit) onEditTask(created);
      return created;
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
      return null;
    }
  };

  const applyTaskUpdate = (updated: ThreadTask) => {
    setTasks((current) => [...(current ?? []).filter((candidate) => candidate.id !== updated.id), updated].sort(compareTasksForDisplay));
    setCompletedTasks((current) => [...current.filter((candidate) => candidate.id !== updated.id), ...(updated.status === "completed" ? [updated] : [])]);
  };
  // Successful completion moves the task to Completed, where it can be reopened.
  const toggleDone = async (task: ThreadTask) => {
    const done = isActiveTaskStatus(task.status);
    try {
      const updated = await mailClient.setTaskStatus(task.id, done ? "completed" : "open");
      applyTaskUpdate(updated);
      setAnnouncement(`${done ? "Completed" : "Reopened"}: ${task.title}`);
      onTasksChanged();
      void reloadPlan();
      return done ? `task:${updated.id}` : null;
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
      return null;
    }
  };

  const everyEventToday = agendaEventsFor(schedule?.events ?? [], now, calendars);
  // A working location is not a meeting, so no view of the day lists it as an event.
  const todaysEvents = everyEventToday.filter((event) => !isWorkingLocation(event));
  const shownEvents = showingToday ? todaysEvents : agendaEventsFor(displaySchedule?.events ?? [], shownDay, calendars).filter((event) => !isWorkingLocation(event));
  // Meeting preparation lives with the linked note. Event titles open Calendar;
  // note actions and the Prepare shortcut open the meeting's note.
  const backdrop = useTodayBackdrop();
  const next = nextMeeting(shownEvents, now);
  const meetings = shownEvents.filter((event) => isPreparable(event) && !isDeclined(event));
  const chooseEvent = (event: ScheduleEvent) => onOpenEvent(event);
  // What is ready for each meeting still ahead, shown on the day view and the agenda.
  const ahead = meetingsToPrepare(shownEvents, now);
  // The hook keys its request by the references' content, so a new array each render doesn't reload.
  const readiness = useMeetingReadiness(ahead.flatMap((event) => { const reference = meetingEventReference(event); return reference ? [reference] : []; }), `${refreshKey}:${taskRefreshKey}`);
  const readinessRows = readinessByKey(readiness.rows);
  const readinessOf = (event: ScheduleEvent) => {
    const key = ahead.includes(event) ? preparationKey(event) : null;
    return key ? { key, row: readinessRows.get(key) } : null;
  };
  const describeReadiness = (event: ScheduleEvent) => {
    const found = readinessOf(event);
    return found?.row ? readinessSummary(found.row) : null;
  };
  // Unread conversations whose sender is invited to a meeting still ahead,
  // counted on that meeting in the day view (the mail rows used to say it).
  const unreadFromAttendees = new Map<ScheduleEvent, number>();
  for (const item of mail?.threads ?? []) {
    if (!item.sender) continue;
    for (const event of meetingsWith(parseAddress(item.sender).email, shownEvents, now)) unreadFromAttendees.set(event, (unreadFromAttendees.get(event) ?? 0) + 1);
  }
  // Finished events dim without selecting a preparation panel in Today.
  const state = (event: ScheduleEvent) => ({
    past: isPastEvent(event, now),
    selected: false,
  });
  const decorateEvent = (event: ScheduleEvent) => {
    if (!isPreparable(event)) return state(event);
    const found = readinessOf(event);
    const unread = unreadFromAttendees.get(event) ?? 0;
    const row = found?.row;
    const hasNote = Boolean(row?.hasOccurrenceNote);
    const commitments = row?.openCommitments ?? 0;
    // Visible at a glance: a note mark, open commitments, and unread mail from attendees, each with its count.
    const marks = [
      hasNote ? <NotebookPen key="note" size={ICON_SIZE.xs} /> : null,
      commitments ? <span key="commitments" className="calendar-event-mark-count" title="Open commitments"><ListChecks size={ICON_SIZE.xs} />{commitments}</span> : null,
      unread ? <span key="unread" className="calendar-event-mark-count" title="Unread from attendees"><Mail size={ICON_SIZE.xs} />{unread}</span> : null,
    ].filter(Boolean);
    return {
      action: onAddNote && meetingEventReference(event) ? <EventNoteAction occurrence={meetingEventReference(event)!} labelSuffix={event.title} onActivate={() => onAddNote(event)} /> : null,
      rowKey: `meeting:${preparationKey(event)}`,
      description: [row ? readinessSummary(row) : null, unread ? `${unread} unread from attendees` : null].filter(Boolean).join(" · ") || null,
      mark: !onAddNote && marks.length ? <>{marks}</> : null,
      ...state(event),
    };
  };
  useWorkspaceCommands({
    "today.nextView": () => selectPanel(otherPanel),
    "today.previousView": () => selectPanel(otherPanel),
    ...(panel === "note" ? { "today.write": () => dailyNote.current?.focus() } : {}),
    "today.previousDay": () => onChangeDay?.(addDays(shownDay, -1)),
    "today.nextDay": () => onChangeDay?.(addDays(shownDay, 1)),
    "meetings.prepare": () => {
      const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest("[data-row-key]")?.getAttribute("data-row-key") : null;
      const target = meetings.find((event) => focused === `meeting:${preparationKey(event)}`) ?? next ?? ahead[0] ?? meetings[0];
      if (target) onAddNote?.(target);
    },
  });
  // Booking: Book takes the next free half hour today; a drop takes the slot under the pointer.
  const booking = { accounts: calendarAccounts, calendars, onNotice: notice };
  const undoBooking = (undo: () => Promise<void>) => () => void undo().then(
    () => notice({ message: "Booked time removed" }),
    (reason: unknown) => notice({ message: errorMessage(reason) }),
  );
  // A drop is an explicit time, so it books wherever it lands, but says when that is outside working hours.
  const outsideHours = (start: Date) => withinWorkingHours(start, workingHours) === false ? " (outside working hours)" : "";
  // Another day's booking names the day, since the schedule on screen may not be the one the person expects.
  const onOtherDay = (start: Date) => isSameDay(start, now) ? "" : ` on ${start.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}`;
  const bookAt = async (target: BlockTarget, start: Date) => {
    const booked = await bookBlock(target, start, booking);
    if (booked) notice({ message: `Booked ${booked.when}${onOtherDay(start)}${outsideHours(start)} for “${target.title}”`, undo: undoBooking(booked.undo) });
    return booked;
  };
  const bookNext = async (target: BlockTarget) => {
    if (!schedule) { notice({ message: "Connect a calendar in Settings to book time" }); return; }
    // Unknown is not free: without today's schedule or working hours there is no free slot to offer.
    const known = schedule.error || schedule.loading ? null : todaysEvents;
    const start = nextFreeSlot(known, workingHours, new Date());
    if (start === "unknown") { notice({ message: "Today's calendar or working hours couldn't be read, so free time is unknown" }); return; }
    if (!start) { notice({ message: "No free half hour is left in today's working hours" }); return; }
    return bookAt(target, start);
  };

  const addCandidate = async (candidate: PlanCandidate) => {
    const resolved = await candidate.resolve();
    if (!resolved) return;
    try {
      const saved = await addToDailyPlan(planDate, [resolved.addition]);
      if (!saved.changed.length) { notice({ message: `“${resolved.title}” is already in today's plan` }); return; }
      notice({ message: `Added “${resolved.title}” to today's plan`, undo: () => void saved.undo().catch(planWriteFailed) });
    } catch (reason) {
      planWriteFailed(reason);
    } finally {
      void reloadPlan();
    }
  };
  const removeItem = async (item: DailyPlanItem) => {
    try {
      const saved = await removeFromDailyPlan(planDate, item.key);
      notice({ message: `Removed “${item.title}” from today's plan`, undo: () => void saved.undo().catch(planWriteFailed) });
    } catch (reason) {
      planWriteFailed(reason);
    } finally {
      void reloadPlan();
    }
  };
  const planBookedCandidate = async (resolved: NonNullable<Awaited<ReturnType<PlanCandidate["resolve"]>>>, booked: NonNullable<Awaited<ReturnType<typeof bookBlock>>>) => {
    try {
      const saved = await addToDailyPlan(planDate, [resolved.addition]);
      notice({
        message: `Booked ${booked.when}${outsideHours(new Date(booked.event.start))} for “${resolved.title}” and added it to today's plan`,
        undo: () => { undoBooking(booked.undo)(); if (saved.changed.length) void saved.undo().catch(planWriteFailed); },
      });
    } catch (reason) {
      notice({ message: `Booked ${booked.when} for “${resolved.title}”, but it couldn't be added to today's plan: ${errorMessage(reason)}`, undo: undoBooking(booked.undo), action: openNoteAction });
    } finally { void reloadPlan(); }
  };
  const scheduleCandidate = async (candidate: PlanCandidate) => {
    const resolved = await candidate.resolve();
    if (!resolved) return;
    const booked = await bookNext(resolved.block);
    if (booked) await planBookedCandidate(resolved, booked);
  };
  const dropOnDay = async (source: PlanDragSource, day: Date, startMinutes: number) => {
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, startMinutes);
    if (source.kind === "item") { await bookAt(itemBlock(source.item), start); return; }
    const resolved = await source.candidate.resolve();
    if (!resolved) return;
    const booked = await bookBlock(resolved.block, start, booking);
    if (!booked) return;
    // Only today's plan takes the item; a block on another day books the time and leaves the plan alone.
    if (isSameDay(start, now)) await planBookedCandidate(resolved, booked);
    else notice({ message: `Booked ${booked.when}${onOtherDay(start)}${outsideHours(start)} for “${resolved.title}”`, undo: undoBooking(booked.undo) });
  };
  const gridRef = useRef<HTMLDivElement>(null);
  const dayStart = startOfLocalDay(now).getTime();
  // Drops book on the day the schedule shows, today or later; a past day takes none.
  const shownDayStart = startOfLocalDay(shownDay).getTime();
  const canBookShownDay = panel === "agenda" && shownDayStart >= dayStart;
  const dragDays = useMemo(() => [new Date(shownDayStart)], [shownDayStart]);
  const drag = useTaskBlockDrag<PlanDragSource>({ gridRef, days: dragDays, blockMinutes: TASK_BLOCK_MINUTES, onDrop: (source, day, minutes) => { if (canBookShownDay) void dropOnDay(source, day, minutes); } });

  const togglePlanItem = async (item: DailyPlanItem) => {
    const finished = isFinished(item);
    try {
      const note = noteTaskParts(item.taskId);
      if (note) {
        const expected = note.noteId === plan?.noteId ? plan.revision : (await mailClient.getNote(note.noteId))?.updatedAt ?? null;
        if (!expected) throw new Error("That note isn't on this device");
        await changeNoteItem(note.noteId, note.markerId, expected, { checked: !finished });
      } else if (item.taskId) {
        const updated = await mailClient.setTaskStatus(item.taskId, finished ? "open" : "completed");
        applyTaskUpdate(updated);
      }
      setAnnouncement(`${finished ? "Reopened" : "Completed"}: ${item.title}`);
      onTasksChanged();
      return !finished && item.taskId ? `task:${linkKey(item.taskId) ?? item.taskId}` : null;
    } catch (reason) {
      notice({ message: errorMessage(reason), action: openNoteFor(item) });
      return null;
    } finally {
      void reloadPlan();
    }
  };
  // A refused note change offers the note it was refused in.
  const openNoteFor = (item: DailyPlanItem) => { const parts = noteTaskParts(item.taskId); return parts && onOpenNote ? { label: "Open Note", run: () => onOpenNote(parts.noteId) } : undefined; };
  const activatePlanItem = async (item: DailyPlanItem) => {
    const parts = noteTaskParts(item.taskId);
    if (parts) { onOpenNote?.(parts.noteId); return; }
    if (item.kind === "contact") { if (item.contactId) onOpenContact(item.contactId); return; }
    if (!item.taskId) return;
    try {
      const task = tasks.find((candidate) => candidate.id === item.taskId) ?? (await mailClient.listTasks()).find((candidate) => candidate.id === item.taskId);
      if (task) onEditTask(task);
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };

  // Sources: each a tab of cards that can be added to the plan or dragged onto the day.
  /** A task's due date for a row; an overdue one is marked in color and named for screen readers. */
  const dueRow = (task: ThreadTask | undefined): { date?: ReactNode; dateClassName?: string } => {
    const due = task ? formatDue(task) : null;
    if (!task || !due) return {};
    const overdue = isOverdue(task);
    return { date: <>{overdue ? <span className="sr-only">Overdue: </span> : null}Due {due}</>, dateClassName: overdue ? "task-due-overdue" : undefined };
  };
  const taskCandidate = (task: ThreadTask): PlanCandidate => {
    const done = !isActiveTaskStatus(task.status);
    const resolved = { addition: { kind: "link" as const, reference: taskReference(task.id), label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: task.accountId, sourcePlanKey: planItemKey(taskReference(task.id)) }, title: task.title };
    return {
      id: `task:${task.id}`,
      taskId: task.id,
      title: task.title,
      planKey: planItemKey(taskReference(task.id)),
      resolve: async () => resolved,
      row: {
        as: "article",
        className: done ? "context-task context-task-done" : "context-task",
        control: <HoverTooltip label={done ? "Mark not done" : "Mark done"}>
          <TodayCompletionCheckbox checked={done} transition={completion}
            label={done ? `Mark ${task.title} Not Done` : `Mark ${task.title} Done`} save={() => toggleDone(task)} />
        </HoverTooltip>,
        wrapTitle: true,
        ...dueRow(task),
        // Every task card names its account when Today shows them all.
        detail: [task.subjectSnapshot, accountId === null ? task.accountId : null].filter(Boolean).join(" · ") || undefined,
        onActivate: () => onEditTask(task),
      },
    };
  };
  const promiseCandidate = (promise: OpenPromise): PlanCandidate => ({
    id: `promise:${promise.messageId}`,
    title: promise.excerpt,
    // A promise isn't a task yet, so nothing in the plan can name it.
    planKey: null,
    resolve: async () => {
      const created = await trackPromise(promise);
      return created ? { addition: { kind: "link", reference: taskReference(created.id), label: created.title }, block: { title: created.title, sourceTaskId: created.id, accountId: created.accountId }, title: created.title } : null;
    },
    row: {
      as: "article",
      icon: <Send size={ICON_SIZE.sm} />,
      wrapTitle: true,
      date: formatArrival(promise.sentAt, now),
      detail: `To ${promiseRecipients(promise)} · Nothing sent to them since`,
      onActivate: () => onOpenThread(promise.thread.id),
      actions: [
        <button key="task" type="button" className="btn-icon btn-icon-sm" aria-label={`Make a task: ${promise.excerpt}`} title="Make a task" onClick={() => void trackPromise(promise)}>
          <ListPlus size={ICON_SIZE.sm} />
        </button>,
        <button key="dismiss" type="button" className="btn-icon btn-icon-sm" aria-label={`Dismiss: ${promise.excerpt}`} title="Dismiss" onClick={() => setDismissed(dismissPromise(promise.messageId))}>
          <X size={ICON_SIZE.sm} />
        </button>,
      ],
    },
  });
  const contactCandidate = (profile: ContactProfile): PlanCandidate => {
    const name = profile.displayName || profile.addresses[0] || "Unnamed contact";
    const address = profile.addresses[0] ? normalizeContactAddress(profile.addresses[0]) : null;
    const reference: NoteReference | null = address ? { version: 1, kind: "contact", id: address, ...(profile.id.startsWith("derived:") ? {} : { contact: profile.id }) } : null;
    return {
      id: `contact:${profile.id}`,
      title: name,
      planKey: reference ? planItemKey(reference) : null,
      resolve: async () => reference ? { addition: { kind: "link", reference, label: name }, block: { title: `Check in with ${name}`, sourceTaskId: null, accountId: null, sourcePlanKey: reference ? planItemKey(reference) : undefined }, title: name } : null,
      row: {
        icon: <ContactRound size={ICON_SIZE.sm} />,
        date: profile.keepInTouchDueAt ? describeDue(profile.keepInTouchDueAt) : undefined,
        detail: profile.displayName ? profile.addresses[0] : undefined,
        onActivate: () => onOpenContact(profile.id),
      },
    };
  };
  const inboxRefreshKey = `${refreshKey}:${taskRefreshKey}:${plan?.revision ?? ""}:${inboxRevision}`;
  const starredPage = usePlanSourcePage(planDate, "starredMail", accountId, inboxRefreshKey, true);
  const notesPage = usePlanSourcePage(planDate, "notes", null, inboxRefreshKey, true);
  const shownDate = dateInputValue(shownDay);
  const doneShown = startOfLocalDay(shownDay).getTime() <= startOfLocalDay(now).getTime();
  const completedNotes = useTodayCompletedNotes(shownDate, inboxRefreshKey);
  const doneOnDay = useDoneOnDay(shownDay, accountId, inboxRefreshKey, doneShown);
  const createForPlan = async (request: TaskFormRequest) => {
    if (!onCreateTask) return null;
    const created = await onCreateTask(request);
    if (created) onTasksChanged();
    return created;
  };
  const taskResolution = (task: Pick<ThreadTask, "id" | "title" | "accountId">) => ({ addition: { kind: "link" as const, reference: taskReference(task.id), label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: task.accountId, sourcePlanKey: planItemKey(taskReference(task.id)) }, title: task.title });
  const completingEmails = useRef(new Set<string>());
  const [busyEmails, setBusyEmails] = useState(new Set<string>());
  const completeEmail = async (key: string, resolve: () => Promise<{ taskId: string } | null>) => {
    if (completingEmails.current.has(key)) return null;
    completingEmails.current.add(key); setBusyEmails(new Set(completingEmails.current));
    try {
      const resolved = await resolve();
      if (!resolved) return null;
      const updated = await mailClient.setTaskStatus(resolved.taskId, "completed");
      applyTaskUpdate(updated);
      starredPage.remove(key);
      setAnnouncement(`Completed: ${updated.title}`);
      onTasksChanged(); setInboxRevision((value) => value + 1);
      return `task:${updated.id}`;
    } catch (reason) {
      notice({ message: errorMessage(reason) });
      return null;
    } finally {
      completingEmails.current.delete(key); setBusyEmails(new Set(completingEmails.current));
    }
  };
  const openCandidateTask = async (taskId: string) => {
    const parts = noteTaskParts(taskId);
    if (parts) { onOpenNote?.(parts.noteId); return; }
    try {
      const task = tasks.find((candidate) => candidate.id === taskId) ?? (await mailClient.listTasks()).find((candidate) => candidate.id === taskId);
      if (task) onEditTask(task);
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };
  const remoteCandidate = (remoteKind: PlanSourceKind | null) => (candidate: SourceCandidate): PlanCandidate => {
    const action = candidate.action;
    const resolve = async () => {
      if (action.kind === "link") {
        // A check-in books an event of its own, as Keep in Touch's cards do.
        const block = action.addition.kind === "link" && action.addition.reference.kind === "contact"
          ? { title: `Check in with ${candidate.title}`, sourceTaskId: null, accountId: null, sourcePlanKey: candidate.key }
          : { title: candidate.title, sourceTaskId: candidate.taskId, accountId: candidate.accountId, sourcePlanKey: candidate.key };
        return { addition: action.addition, block, title: candidate.title };
      }
      if (action.kind === "makeTask") {
        const promise = promises.find((item) => item.messageId === action.messageId);
        const created = promise ? await trackPromise(promise) : null;
        return created ? taskResolution(created) : null;
      }
      // Cancel in the editor writes nothing: no task, no plan line, no booking.
      if (action.kind !== "taskForm") return null;
      const created = await createForPlan({ kind: "conversation", threadId: action.threadId, accountId: action.accountId });
      return created ? taskResolution(created) : null;
    };
    return {
      id: `${remoteKind}:${candidate.key}`,
      taskId: candidate.taskId,
      title: candidate.title,
      planKey: candidate.key,
      inPlan: candidate.planned,
      resolve,
      row: {
        as: "article",
        icon: candidate.reason === "starred" ? <Star size={ICON_SIZE.sm} /> : candidate.reason === "noteTask" ? <NotebookPen size={ICON_SIZE.sm} />
          : candidate.reason === "promise" ? <Send size={ICON_SIZE.sm} /> : candidate.reason === "keepInTouch" ? <ContactRound size={ICON_SIZE.sm} /> : undefined,
        wrapTitle: true,
        ...dueRow(tasks.find((task) => task.id === candidate.taskId)),
        detail: [candidate.detail, candidate.goal?.title, accountId === null ? candidate.accountId : null].filter(Boolean).join(" · ") || undefined,
        // A card opens its task, its note, or the starred conversation; adding never unstars or archives it.
        onActivate: candidate.taskId ? () => void openCandidateTask(candidate.taskId!) : candidate.threadId ? () => onOpenThread(candidate.threadId!)
          : candidate.contactId ? () => onOpenContact(candidate.contactId!) : undefined,
      },
    };
  };
  // Promises dismissed on this device stay out of the inbox.
  const pageCandidates = (page: typeof starredPage, kind: PlanSourceKind | null) => page.candidates
    .filter((candidate) => candidate.action.kind !== "makeTask" || !dismissed.includes(candidate.action.messageId)).map(remoteCandidate(kind));
  // The plan and the day view cover every account; these sources follow the account in view, and say so beside the tabs.
  const scope = mailAccountCount > 1 ? accountId ?? "All accounts" : undefined;
  const identities = (key: string | null, taskId?: string | null) => [key ? `plan:${key}` : null, taskId ? `task:${linkKey(taskId) ?? taskId}` : null].filter((key): key is string => Boolean(key));
  const scheduled = (key: string | null, taskId?: string | null) => todaysEvents.some((event) => !isDeclined(event) && (
    (key !== null && event.sourcePlanKey === key) || (taskId && linkKey(event.sourceTaskId) === linkKey(taskId))
  ));
  const completedIds = new Set([...completedTasks.map((task) => task.id), ...completedNotes.tasks.map((task) => task.id)]);
  const planEntry = (item: DailyPlanItem): InboxEntry => {
    const done = item.state === "done";
    const checkable = item.taskId && ["task", "noteTask", "newTask"].includes(item.kind) && item.state !== "unavailable";
    const usable = item.state !== "unavailable" && item.kind !== "text";
    const source = { id: `plan:${item.key}`, title: item.title, kind: "item" as const, item };
    return {
      id: source.id, identities: identities(item.key, item.taskId), category: item.kind === "contact" ? "contacts" : "tasks", title: item.title,
      source: usable && !done ? source : null, schedule: usable && !done ? () => bookNext(itemBlock(item)) : undefined,
      actions: [{ label: "Remove from Today's Plan", onSelect: () => void removeItem(item) }],
      row: {
        rowKey: source.id, wrapTitle: true, className: "context-task today-plan-card",
        ...(done ? {} : dueRow(tasks.find((task) => task.id === item.taskId))),
        control: checkable ? <TodayCompletionCheckbox checked={done} transition={completion} label={`Mark ${item.title} ${done ? "Not Done" : "Done"}`} save={() => togglePlanItem(item)} /> : undefined,
        icon: item.kind === "contact" ? <ContactRound size={ICON_SIZE.sm} /> : item.kind === "noteTask" || item.kind === "newTask" ? <NotebookPen size={ICON_SIZE.sm} /> : undefined,
        detail: [item.state === "unavailable" ? "Unavailable on this device" : item.kind === "noteTask" || item.kind === "newTask" ? "Note" : (accountId === null || item.accountId !== accountId) ? item.accountId : null, item.goal?.title, carriedLabel(item.carryCount ?? 0)].filter(Boolean).join(" · ") || undefined,
        onActivate: item.kind === "text" ? undefined : () => void activatePlanItem(item),
      },
    };
  };
  const plannedEntries = (plan?.items ?? []).filter((item) => !isFinished(item) && !completedIds.has(item.taskId ?? "") && !scheduled(item.key, item.taskId)).map(planEntry);
  const candidateEntry = (candidate: PlanCandidate, category: InboxCategory, actions: InboxEntry["actions"] = []): InboxEntry | null => {
    if (completedIds.has(candidate.taskId ?? "") || scheduled(candidate.planKey, candidate.taskId)) return null;
    const planItem = (plan?.items ?? []).find((item) => item.key === candidate.planKey || (item.taskId && item.taskId === candidate.taskId));
    if (planItem && isFinished(planItem)) return null;
    const keys = identities(candidate.planKey, candidate.taskId);
    return {
      id: candidate.id, identities: keys.length ? keys : [candidate.id], category, title: candidate.title,
      row: candidate.row, source: { id: candidate.id, title: candidate.title, kind: "candidate", candidate },
      schedule: () => scheduleCandidate(candidate),
      actions: [...(!planItem && !candidate.inPlan ? [{ label: "Add to Today's Plan", onSelect: () => void addCandidate(candidate) }] : []), ...actions],
    };
  };
  const setNoteTaskDone = async (taskId: string | null | undefined, title: string, checked: boolean) => {
    const parts = noteTaskParts(taskId ?? null);
    if (!parts) return null;
    try {
      const note = await mailClient.getNote(parts.noteId);
      if (!note) throw new Error("That note isn't on this device");
      await changeNoteItem(parts.noteId, parts.markerId, note.updatedAt, { checked });
      setAnnouncement(`${checked ? "Completed" : "Reopened"}: ${title}`);
      onTasksChanged(); setInboxRevision((value) => value + 1);
      return checked && taskId ? `task:${linkKey(taskId) ?? taskId}` : null;
    } catch (reason) { notice({ message: errorMessage(reason) }); return null; }
  };
  const noteCandidates = pageCandidates(notesPage, "notes").map((candidate) => ({ ...candidate, row: {
    ...candidate.row,
    control: <TodayCompletionCheckbox checked={false} transition={completion} label={`Mark ${candidate.title} Done`} save={() => setNoteTaskDone(candidate.taskId, candidate.title, true)} />,
  } }));
  const candidates: (InboxEntry | null)[] = [
    ...tasks.filter((task) => isActiveTaskStatus(task.status) && (accountId === null || task.accountId === accountId)).map((task) => candidateEntry(taskCandidate(task), "tasks")),
    ...noteCandidates.map((candidate) => candidateEntry(candidate, "tasks")),
    ...pageCandidates(starredPage, "starredMail").map((candidate) => {
      const entry = candidateEntry(candidate, candidate.taskId ? "tasks" : "emails");
      if (!entry || candidate.taskId) return entry;
      const key = candidate.planKey ?? candidate.id;
      return { ...entry, busy: busyEmails.has(key), row: { ...entry.row, control: <TodayCompletionCheckbox checked={false} transition={completion} disabled={busyEmails.has(key)} label={`Mark ${candidate.title} Done`} save={() => completeEmail(key, async () => {
        const resolved = await candidate.resolve();
        return resolved?.block.sourceTaskId ? { taskId: resolved.block.sourceTaskId } : null;
      })} /> } };
    }),
    ...openPromises.filter((promise) => accountId === null || promise.thread.accountId === accountId).map((promise) => {
      const candidate = promiseCandidate(promise);
      candidate.row = { ...candidate.row, control: <TodayCompletionCheckbox checked={false} transition={completion} disabled={busyEmails.has(candidate.id)} label={`Mark ${candidate.title} Done`} save={() => completeEmail(candidate.id, async () => {
        const created = await trackPromise(promise, false);
        return created ? { taskId: created.id } : null;
      })} /> };
      const entry = candidateEntry(candidate, "emails", [
      { label: "Make a Task", onSelect: () => void trackPromise(promise) },
      { label: "Dismiss", onSelect: () => setDismissed(dismissPromise(promise.messageId)) },
      ]);
      return entry ? { ...entry, busy: busyEmails.has(candidate.id) } : null;
    }),
    ...reconnect.map((profile) => candidateEntry(contactCandidate(profile), "contacts")),
  ];
  const inboxEntries = uniqueInboxEntries([...plannedEntries, ...interleaveInboxCategories(candidates.filter((entry): entry is InboxEntry => entry !== null))]);
  const finishedOnShownDay = (at: string | null | undefined) => Boolean(at) && dateInputValue(new Date(at as string)) === shownDate;
  const doneEntries: DoneEntry[] = doneShown ? uniqueDoneEntries([
    ...completedTasks.filter((task) => finishedOnShownDay(task.completedAt) && (accountId === null || task.accountId === accountId)).map((task): DoneEntry => ({
      id: `task:${task.id}`, identities: identities(planItemKey(taskReference(task.id)), task.id), category: "tasks", title: task.title, at: task.completedAt ?? null,
      row: { wrapTitle: true, onActivate: () => onEditTask(task), detail: accountId === null ? task.accountId : undefined,
        control: <input type="checkbox" className="context-task-checkbox" checked aria-label={`Mark ${task.title} Not Done`} onChange={() => void toggleDone(task)} /> },
    })),
    ...completedNotes.tasks.map((task): DoneEntry => ({
      id: task.id, identities: identities(planItemKey({ version: 1, kind: "task", id: task.markerId, note: task.noteId }), task.id), category: "tasks", title: task.title, at: task.completedAt ?? null,
      row: { wrapTitle: true, detail: task.noteTitle, onActivate: () => onOpenNote?.(task.noteId),
        control: <input type="checkbox" className="context-task-checkbox" checked aria-label={`Mark ${task.title} Not Done`} onChange={() => void setNoteTaskDone(task.id, task.title, false)} /> },
    })),
    // Plan items marked done carry no completion time; they come after the timed rows, and only for today.
    ...(showingToday ? (plan?.items ?? []).filter((item) => item.state === "done" && item.kind !== "contact").map((item): DoneEntry => {
      const { identities: keys, title, row } = planEntry(item);
      return { id: `plan:${item.key}`, identities: keys, category: "tasks", title, at: null, row };
    }) : []),
    ...doneOnDay.sent.map((sent): DoneEntry => ({
      id: `mail:${sent.thread.id}`, identities: [`mail:${sent.thread.id}`], category: "emails", title: sent.thread.subject || "(no subject)", at: sent.lastSentAt,
      row: { wrapTitle: true, icon: <Send size={ICON_SIZE.sm} />, onActivate: () => onOpenThread(sent.thread.id),
        detail: [sent.recipients.map((recipient) => parseAddress(recipient).name || parseAddress(recipient).email).join(", "), accountId === null && mailAccountCount > 1 ? sent.thread.accountId : null].filter(Boolean).join(" · ") || undefined },
    })),
    // A calendar account is named by its address, so a mail account filter keeps the meetings on the calendar of the same address.
    ...doneOnDay.meetings.filter((meeting) => accountId === null || meeting.accountId.toLowerCase() === accountId.toLowerCase()).map((meeting): DoneEntry => {
      const key = `meeting:${meeting.uid ?? meeting.id}`;
      const when = meeting.allDay ? formatEventDate(meeting) : `${formatEventDate(meeting)} · ${formatClockTime(eventDate(meeting))}`;
      const people = (meeting.attendees ?? []).map((address) => parseAddress(address).name || parseAddress(address).email).join(", ");
      return { id: key, identities: [key], category: "meetings", title: meeting.title, at: meeting.created,
        row: { wrapTitle: true, icon: <CalendarPlus size={ICON_SIZE.sm} />, onActivate: () => onOpenEvent(meeting), detail: [when, people].filter(Boolean).join(" · ") } };
    }),
    ...doneOnDay.notes.map((note): DoneEntry => ({
      id: `note:${note.id}`, identities: [`note:${note.id}`], category: "notes", title: note.title, at: note.at,
      row: { wrapTitle: true, icon: <NotebookPen size={ICON_SIZE.sm} />, onActivate: () => onOpenNote?.(note.id), detail: [note.edited ? "Edited" : null, note.preview].filter(Boolean).join(" · ") || undefined },
    })),
  ]) : [];
  const doneErrors = { tasks: sourceErrors.tasks, noteTasks: completedNotes.error, ...doneOnDay.errors };
  const doneTitle = showingToday ? "Done Today" : "Done";
  const inboxPages = (filter: InboxFilter) => filter === "contacts" ? [] : filter === "emails" ? [starredPage] : filter === "tasks" ? [notesPage, starredPage] : [notesPage, starredPage];
  const inboxUnavailable = (filter: InboxFilter) => Boolean(planError
    || ((filter === "all" || filter === "tasks") && sourceErrors.tasks)
    || ((filter === "all" || filter === "emails") && sourceErrors.promises)
    || ((filter === "all" || filter === "contacts") && sourceErrors.contacts)
    || inboxPages(filter).some((page) => page.error));
  const inboxStatus = (filter: InboxFilter) => <>
    {planError ? <p className="form-error" role="alert">{planError}</p> : null}
    {(filter === "all" || filter === "tasks") && sourceErrors.tasks ? <p className="form-error" role="alert">Tasks: {sourceErrors.tasks}</p> : null}
    {(filter === "all" || filter === "emails") && sourceErrors.promises ? <p className="form-error" role="alert">Promises: {sourceErrors.promises}</p> : null}
    {(filter === "all" || filter === "contacts") && sourceErrors.contacts ? <p className="form-error" role="alert">Contacts: {sourceErrors.contacts}</p> : null}
    {inboxPages(filter).map((page, index) => page.error ? <p key={index} className="form-error" role="alert">{page.error}</p> : null)}
  </>;

  /** The editor, opened with a title typed in the inline field when no single account can take the task. */
  const [addingTask, setAddingTask] = useState<string | null>(null);
  const [newEventRange, setNewEventRange] = useState<{ start: Date; end: Date } | null>(null);
  const taskAccounts = mailAccounts.map((account) => account.email);
  const defaultTaskAccount = accountId && taskAccounts.includes(accountId) ? accountId : taskAccounts.length === 1 ? taskAccounts[0] : undefined;
  const startNewEvent = () => {
    const start = new Date(shownDay);
    const current = new Date();
    const hour = isSameDay(start, current) ? Math.min(current.getHours() + (current.getMinutes() ? 1 : 0), 23) : 9;
    start.setHours(hour, 0, 0, 0);
    setNewEventRange({ start, end: new Date(start.getTime() + 60 * 60_000) });
  };
  const createStandaloneTask = async (values: Omit<Parameters<typeof mailClient.createTask>[0], "accountId" | "threadId" | "subjectSnapshot">, taskAccount: string) => {
    const created = await mailClient.createTask({ ...values, accountId: taskAccount, threadId: null, subjectSnapshot: null });
    applyTaskUpdate(created);
    onTasksChanged();
    return created;
  };
  const canCreateEvents = Boolean(schedule) && calendarAccounts.some((account) => account.status === "connected");

  return <section ref={pageRef} className="today-workspace" data-backdrop={backdrop ? skyPhase(now) : undefined} aria-label="Today" aria-busy={pull.refreshing}>
    <div className="today-page today-page-planning">
    <PullToRefreshStatus {...pull} />
    <header className="today-header" data-sky={backdrop && skyPhase(now) !== "day" ? "dark" : undefined}>
      {backdrop ? <TodayBackdrop now={now} /> : null}
      <div className="today-heading">
        <span className="eyebrow">{now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" })}</span>
        <h1>Plan Your Day</h1>
        {mailAccountCount > 1 ? <AccountScopeChip accountId={accountId} /> : null}
      </div>
    </header>
    <div className="today-columns">
      <div className="today-column today-stack">
      <div className="today-work">
        <PlanningInbox onAddTask={taskAccounts.length ? async (title) => {
          if (!defaultTaskAccount) { setAddingTask(title); return; }
          await createStandaloneTask({ title, kind: "action", dueKind: "none", dueValue: null }, defaultTaskAccount);
          setAnnouncement(`Task added: ${title}`);
        } : undefined} entries={inboxEntries} scope={scope} status={inboxStatus} unavailable={inboxUnavailable}
          loading={(filter) => loadingSources || inboxPages(filter).some((page) => page.loading)}
          hasMore={(filter) => inboxPages(filter).some((page) => Boolean(page.next))}
          onLoadMore={(filter) => { for (const page of inboxPages(filter)) if (page.next) void page.loadMore(); }} drag={{ startDrag: drag.startDrag, clickSuppressed: drag.clickSuppressed, draggingId: drag.draggingId, enabled: canBookShownDay }} />
      </div>
      {doneShown ? <div className="today-work today-done-card">
        <DoneCard title={doneTitle} today={showingToday} dayLabel={showingToday ? undefined : shownDay.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}
          entries={doneEntries} errors={doneErrors} loading={doneOnDay.loading || (loadingSources && loadedTasks === null)} />
      </div> : null}
      </div>
      <div className="today-column today-time" role="group" aria-label="Day Planning">
        <div className="today-panel-heading">
          <div className="today-panel-title">
            <h2>{panel === "agenda" ? <CalendarDays size={ICON_SIZE.md} aria-hidden="true" /> : <NotebookPen size={ICON_SIZE.md} aria-hidden="true" />}{panel === "agenda" ? "Agenda" : "Daily Note"}</h2>
          </div>
          <div className="today-schedule-actions">
            {panel === "agenda" ? <button type="button" className="btn-link" onClick={() => onOpenCalendar(shownDay)}>Open Calendar</button>
              : onOpenNote ? <button type="button" className="btn-link" onClick={() => void dailyNote.current?.openInNotes()}>Open in Notes</button> : null}
          </div>
        </div>
        <div className="today-schedule-navigation">
          <div className="today-schedule-actions" role="group" aria-label="Schedule Date">
            <button type="button" className="btn btn-sm" aria-label="Previous Day" aria-keyshortcuts="-" onClick={() => onChangeDay?.(addDays(shownDay, -1))}><ChevronLeft size={ICON_SIZE.sm} aria-hidden="true" /></button>
            <button type="button" className="btn btn-sm" onClick={() => onChangeDay?.(null)}>Today</button>
            <button type="button" className="btn btn-sm" aria-label="Next Day" aria-keyshortcuts="=" onClick={() => onChangeDay?.(addDays(shownDay, 1))}><ChevronRight size={ICON_SIZE.sm} aria-hidden="true" /></button>
            {panel === "agenda" && canCreateEvents ? <button type="button" className="btn btn-sm" aria-label="New Event" title="New event" onClick={startNewEvent}><Plus size={ICON_SIZE.sm} aria-hidden="true" /></button> : null}
          </div>
          {/* Keep controls on the left for every day. The page heading already names today. */}
          <span className={showingToday ? "sr-only" : undefined} aria-live="polite">{shownDay.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" })}</span>
          <button type="button" className="today-panel-switch" data-mailbox-tab-shortcut aria-keyshortcuts="Tab" aria-label={`Switch to ${otherName}`} onClick={() => selectPanel(otherPanel)}>
            <kbd aria-hidden="true">{formatShortcut("Tab")}</kbd> for {otherName}
          </button>
        </div>
        <div ref={agendaPanel} role="region" aria-label="Agenda" data-mailbox-tab-shortcut className="today-panel-content today-agenda-panel" hidden={panel !== "agenda"} tabIndex={0}>
        {displaySchedule ? <>
          <TaskDragChip title={drag.draggingTitle} pointerRef={drag.pointerRef} onPointer={drag.onPointer} />
          <TodayDayView events={shownEvents} day={showingToday ? undefined : shownDay} now={now} loading={displaySchedule.loading} error={displaySchedule.error} onRetry={displaySchedule.reload}
            selectedEvent={null} onChooseEvent={chooseEvent} eventDecoration={decorateEvent}
            gridRef={gridRef} preview={drag.preview} dragging={drag.draggingId !== null} workingHours={workingHours} onCreateEvent={canCreateEvents ? setNewEventRange : undefined} />
          {/* Below Today's breakpoint the agenda list takes the day view's place (a container query in CSS). */}
          <div className="today-agenda-fallback">
            {/* The page heading already names today, so its agenda leaves the date out. */}
            <CalendarAgenda pullToRefresh={false} today={shownDay} day={showingToday ? "today" : "chosen"} now={now} showDate={false} workingLocation={null} markConflicts calendars={calendars} schedule={displaySchedule} onOpenEvent={chooseEvent} taskFor={(event) => bookedTaskFor(event, [...tasks, ...completedTasks])} onOpenTask={onEditTask} onAddNote={onAddNote} onPrepare={onAddNote} selectedKey={null} describe={describeReadiness} />
          </div>
        </>
          : <ContextSection id="calendar-agenda" title="Today's agenda" rows={[]}
            footer={<p className="context-status">Connect a calendar in Settings to see today's agenda here.</p>} />}
        </div>
        <div ref={notePanel} role="region" aria-label="Daily Note" data-mailbox-tab-shortcut className="today-panel-content" hidden={panel !== "note"} tabIndex={-1}>
          {noteVisited ? <TodayDailyNote key={dateInputValue(shownDay)} ref={dailyNote} date={dateInputValue(shownDay)} active={panel === "note"} refreshKey={refreshKey}
            onLeave={() => notePanel.current?.focus()} onOpenNote={onOpenNote} onOpenEntity={onOpenEntity} /> : null}
        </div>
      </div>
    </div>
    </div>
    <p className="sr-only" aria-live="polite">{announcement}</p>
    {addingTask !== null ? <TaskEditorDialog
      initial={{ title: addingTask }}
      accountId={defaultTaskAccount}
      accountOptions={defaultTaskAccount ? undefined : taskAccounts}
      onClose={() => setAddingTask(null)}
      onSubmit={async (values, selectedAccount) => {
        const taskAccount = defaultTaskAccount ?? selectedAccount;
        if (!taskAccount || !taskAccounts.includes(taskAccount)) throw new Error("Choose a connected account for this task");
        await createStandaloneTask(values, taskAccount);
        setAddingTask(null);
        notice({ message: "Task added" });
      }}
    /> : null}
    {newEventRange ? <CreateCalendarEventDialog
      start={newEventRange.start}
      end={newEventRange.end}
      accounts={calendarAccounts}
      calendars={calendars}
      onClose={() => setNewEventRange(null)}
      onCreated={(event) => {
        revalidateScheduleCache(event);
        setNewEventRange(null);
        onEventCreated?.();
        notice({ message: "Event added" });
      }}
    /> : null}
  </section>;
}

/** The timed meeting under way, or else the next one to start today; declined ones are skipped. */
export function nextMeeting(events: readonly ScheduleEvent[], now: Date): ScheduleEvent | null {
  return events
    .filter((event) => !event.allDay && !isDeclined(event) && new Date(event.end).getTime() > now.getTime())
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime())[0] ?? null;
}

function formatArrival(iso: string, now: Date): string {
  const date = new Date(iso);
  return startOfLocalDay(date).getTime() === startOfLocalDay(now).getTime()
    ? formatClockTime(date)
    : formatCompactDate(date, now);
}
