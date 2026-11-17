import { formatCompactDate } from "./dateFormatting";
import { ClipboardList, ContactRound, ListChecks, ListPlus, Mail, NotebookPen, Send, Star, Target, Video, X } from "lucide-react";
import { meetingEventReference } from "./meetingNoteEntry";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { isPreparable, meetingsToPrepare, preparationKey, readinessByKey, readinessSummary } from "./meetingPrep";
import { useMeetingReadiness } from "./useMeetingPreparation";
import { useWorkspaceCommands } from "./workspaceCommands";
import type { Notice } from "./useNotice";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isWorkingLocation, workingLocationFor, WorkingLocationIcon } from "./WorkingLocation";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ContactProfile, DailyPlanItem, OpenPromise, PlanCompletedTask, PlanCandidate as SourceCandidate, PlanSourceKind, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { agendaEventsFor, formatClockTime, isDeclined, isPastEvent } from "./agendaEvents";
import { addDays, eventDate, startOfLocalDay } from "./calendarTime";
import { errorMessage, logBackgroundFailure } from "./errors";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { meetingPlace } from "./ThreadAssist";
import { dismissPromise, promiseRecipients, readDismissedPromises } from "./openPromises";
import { compareTasksForDisplay, dueView, formatDue, isActiveTaskStatus, isOverdue } from "./taskViews";
import { daySummary, eveningOpens, meetingsWith, offersPlanTomorrow } from "./todayInsights";
import { groupTodayMail } from "./todayMail";
import { carriedToTomorrow, readViewChoice, resolveView, writeViewChoice, type TodayView } from "./eveningView";
import { EveningReview, EveningTomorrow } from "./TodayEvening";
import { TodayBackdrop } from "./TodayBackdrop";
import { useTodayBackdrop } from "./backdropPreference";
import { parseAddress } from "./emailAddress";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { normalizeContactAddress, type NoteReference } from "./noteEntities";
import { addToDailyPlan, removeFromDailyPlan } from "./noteCapture";
import { planItemKey } from "./dailyPlan";
import { changeNoteItem } from "./noteMutations";
import { todayTarget } from "./noteDrafts";
import { TASK_BLOCK_MINUTES } from "./taskBooking";
import { bookBlock, nextFreeSlot, withinWorkingHours, type BlockTarget } from "./todayBooking";
import { bookedSource } from "./taskBooking";
import type { WorkingHours } from "./todayInsights";
import { usePlanSourcePage } from "./usePlanSourcePage";
import type { TaskFormRequest } from "./useTaskActions";
import { useDailyPlan } from "./useDailyPlan";
import { useTaskBlockDrag } from "./useTaskBlockDrag";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { isFinished, itemBlock, linkKey, PlanCards, planProgress, PlanSourceSwitcher, type PlanCandidate, type PlanDragSource, type PlanSource } from "./TodayPlan";
import { TodayDayView } from "./TodayDayView";
import { TodayMailSignal } from "./TodayMailSignal";
import { PlanTodayDialog } from "./PlanTodayDialog";
import type { PlanRequest } from "./useWorkspaces";

import { refreshSyncedData } from "./refreshSyncedData";
import { usePullToRefresh } from "./usePullToRefresh";
import { PullToRefreshStatus } from "./PullToRefreshStatus";

type Schedule = ReturnType<typeof useCalendarSchedule>;

export type TodayWorkspaceProps = {
  /** The mail account in view, or null for all accounts; scopes mail and tasks. */
  accountId: string | null;
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
  onOpenInbox(): void;
  onOpenCalendar(): void;
  onOpenTasks(): void;
  onOpenContact(contactId: string): void;
  onOpenKeepInTouch(): void;
  /** Opens a note, such as today's note when a plan write was refused. */
  onOpenNote?(noteId: string): void;
  /** Says what a plan change or booking did, with Undo. */
  onNotice?(notice: Notice): void;
  /** The default availability profile's working hours, which bound Book's free time; unknown without them. */
  workingHours?: WorkingHours | null;
  /** Opens the task editor for a new task, resolving with the task once saved, or null when cancelled. */
  onCreateTask?(request: TaskFormRequest): Promise<ThreadTask | null>;
  /** Meeting preparation on Today: the meeting being prepared, if any, and
   * what the readiness list needs to act. */
  preparation?: TodayPreparation;
  /** Plan Today or Plan Tomorrow asked for from the palette; consumed once the routine opens. */
  planRequest?: PlanRequest | null;
  onPlanRequestConsumed?(id: number): void;
};

export type TodayPreparation = {
  event: ScheduleEvent | null;
  onPrepare(event: ScheduleEvent): void;
  render(event: ScheduleEvent, close: () => void): ReactNode;
  onClose(): void;
  ownAddresses: readonly string[];
  onOpenNote(noteId: string, href?: string): void;
  onNotice(notice: Notice): void;
};


export const PLAN_SOURCE_KEY = "threestrands.today.planSource";
/** Sources read a page at a time from `planSources`, by tab id. */
const REMOTE_SOURCES: Record<string, PlanSourceKind> = { suggested: "suggested", "starred-mail": "starredMail", goals: "goals", notes: "notes" };
const SOURCE_IDS = ["suggested", "also-due", "tasks", "promises", "keep-in-touch", "starred-mail", "goals", "notes"];
/** Why Suggested offers a card, as its detail line says. */
const SUGGESTED_REASONS: Partial<Record<SourceCandidate["reason"], string>> = {
  overdue: "Overdue", dueToday: "Due today", inProgress: "In progress", waiting: "Waiting past its date", promise: "You said you would", keepInTouch: "Keep in Touch",
};

const taskReference = (id: string): NoteReference => ({ version: 1, kind: "task", id });

/** A note task's owning note and marker, from its Tasks id (`note:<noteId>:<markerId>`), as Calendar reads it. */
function noteTaskParts(taskId: string | null): { noteId: string; markerId: string } | null {
  const source = bookedSource(taskId);
  return source?.kind === "noteTask" ? { noteId: source.noteId, markerId: source.markerId } : null;
}

/** "Planned Mon, Tue" for a goal's task, from the days it was planned in the goal's period. */
function plannedDaysLabel(days: readonly string[]): string | null {
  if (!days.length) return null;
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" });
  return `Planned ${days.map((day) => weekday.format(new Date(`${day}T12:00:00`))).join(", ")}`;
}

/**
 * The day at work: today's plan (or one meeting's preparation) beside a day
 * view of today's schedule, with a one-line mail signal in the header. The
 * day view needs a connected calendar, since calendars are not tied to a
 * mail account; mail and task sources follow the account in view.
 */
export function TodayWorkspace(props: TodayWorkspaceProps) {
  return props.calendarConnected ? <ScheduledToday {...props} /> : <TodayLayout {...props} schedule={null} />;
}

function ScheduledToday(props: TodayWorkspaceProps) {
  const start = startOfLocalDay(new Date());
  const schedule = useCalendarSchedule({
    timeMin: start.toISOString(),
    // Through tomorrow, for the Tomorrow section under the day view.
    timeMax: addDays(start, 2).toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  });
  return <TodayLayout {...props} schedule={schedule} />;
}

function TodayLayout({
  accountId, refreshKey, taskRefreshKey, calendars, calendarAccounts = [], schedule,
  onOpenThread, onOpenEvent, onAddNote, onEditTask, onTasksChanged, onOpenInbox, onOpenCalendar, onOpenTasks, onOpenContact, onOpenKeepInTouch,
  onOpenNote, onNotice, workingHours = null, onCreateTask, preparation, planRequest = null, onPlanRequestConsumed,
}: TodayWorkspaceProps & { schedule: Schedule | null }) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const pageRef = useRef<HTMLElement>(null);
  const accountRef = useRef(accountId);
  accountRef.current = accountId;
  const loadVersion = useRef(0);
  const [mail, setMail] = useState<{ threads: TodayMail[]; total: number } | null>(null);
  const [mailError, setMailError] = useState<string | null>(null);
  const [loadedTasks, setTasks] = useState<ThreadTask[] | null>(null);
  const tasks = loadedTasks ?? [];
  const [announcement, setAnnouncement] = useState("");
  const [reconnect, setReconnect] = useState<ContactProfile[]>([]);
  const [promises, setPromises] = useState<OpenPromise[]>([]);
  // Tasks completed today, for the evening view's Done when today has no plan to review.
  const [completedToday, setCompletedToday] = useState<ThreadTask[]>([]);
  const [dayClosedState, setDayClosed] = useState(false);
  const [dismissed, setDismissed] = useState(readDismissedPromises);
  const load = useCallback(async () => {
    const version = ++loadVersion.current;
    const results = await Promise.allSettled([
      mailClient.listTodayMail(accountId ?? undefined),
      mailClient.listTasks(accountId ?? undefined),
      mailClient.listKeepInTouch(),
      mailClient.listOpenPromises(accountId ?? undefined),
    ]);
    if (version !== loadVersion.current || accountRef.current !== accountId) return;
    const [mailResult, taskResult, contactResult, promiseResult] = results;
    if (mailResult.status === "fulfilled") { setMail(mailResult.value); setMailError(null); }
    else setMailError(errorMessage(mailResult.reason));
    if (taskResult.status === "fulfilled") {
      setTasks(taskResult.value.filter((task) => isActiveTaskStatus(task.status)).sort(compareTasksForDisplay));
      const today = startOfLocalDay(new Date()).getTime();
      setCompletedToday(taskResult.value.filter((task) => task.status === "completed" && task.completedAt && startOfLocalDay(new Date(task.completedAt)).getTime() === today));
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

  // The plan date is today's, read once per day rather than per render.
  const planDate = todayTarget(now).date;
  const { plan, error: planError, reload: reloadPlan } = useDailyPlan(planDate, `${refreshKey}:${taskRefreshKey}`);

  const reloadSchedule = schedule?.refresh;
  const refreshToday = useCallback(async () => {
    const results = await Promise.allSettled([
      mailClient.listAccounts().then((accounts) => accounts.length ? mailClient.sync(accountId ? { accountId, catchUp: false } : { catchUp: false }) : undefined),
      refreshSyncedData(),
      reloadSchedule?.(),
    ]);
    if (accountRef.current !== accountId) return;
    try { await mailClient.reconcileTasks(); } finally { await Promise.all([load(), reloadPlan()]); }
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") throw failed.reason;
    setNow(new Date());
  }, [accountId, reloadSchedule, load, reloadPlan]);
  const pull = usePullToRefresh(pageRef, refreshToday);
  const openPromises = promises.filter((promise) => !dismissed.includes(promise.messageId));

  const notice = (next: Notice) => { if (onNotice) onNotice(next); else setAnnouncement(next.message); };
  const openNoteAction = plan && onOpenNote ? { label: "Open Note", run: () => onOpenNote(plan.noteId) } : undefined;
  const planWriteFailed = (reason: unknown) => notice({ message: errorMessage(reason), action: openNoteAction });

  /** The existing Make a Task flow: an action task from the promise, opened for editing. */
  const trackPromise = async (promise: OpenPromise): Promise<ThreadTask | null> => {
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
      setAnnouncement(`Task added: ${created.title}`);
      onTasksChanged();
      onEditTask(created);
      return created;
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
      return null;
    }
  };

  // Checking a task off keeps its row in place, struck through, until Today is read again.
  const toggleDone = async (task: ThreadTask) => {
    const done = isActiveTaskStatus(task.status);
    try {
      const updated = await mailClient.setTaskStatus(task.id, done ? "completed" : "open");
      setTasks((current) => current?.map((candidate) => candidate.id === task.id ? updated : candidate) ?? null);
      setAnnouncement(`${done ? "Completed" : "Reopened"}: ${task.title}`);
      onTasksChanged();
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };

  const everyEventToday = agendaEventsFor(schedule?.events ?? [], now, calendars);
  // Where the user works is stated once, in the header, so no view of the day lists it as an event.
  const todaysEvents = everyEventToday.filter((event) => !isWorkingLocation(event));
  // Today's two faces: the evening view opens by itself once the day's meetings and working hours are over,
  // or when chosen with Review Today; the choice lasts the rest of the day on this device.
  const [choice, setChoice] = useState<TodayView | null>(() => readViewChoice(planDate));
  useEffect(() => { setChoice(readViewChoice(planDate)); }, [planDate]);
  const scheduleKnown = !schedule ? [] : schedule.error || (schedule.loading && schedule.events.length === 0) ? null : todaysEvents;
  const view = resolveView(choice, eveningOpens(scheduleKnown, workingHours, now));
  const evening = view === "evening";
  const viewRef = useRef(view);
  viewRef.current = view;
  const switchView = useCallback((next: TodayView) => { writeViewChoice(planDate, next); setChoice(next); }, [planDate]);
  const toggleView = useCallback(() => switchView(viewRef.current === "evening" ? "day" : "evening"), [switchView]);
  const backdrop = useTodayBackdrop();
  const tomorrowDate = addDays(startOfLocalDay(now), 1);
  const tomorrowKey = todayTarget(tomorrowDate).date;
  const tomorrowList = schedule ? agendaEventsFor(schedule.events, tomorrowDate, calendars).filter((event) => !isWorkingLocation(event)) : [];
  const locationScope = { accountId, calendars, accountOrder: calendarAccounts.map((account) => account.email) };
  const todayLocation = schedule ? workingLocationFor(schedule.events, now, locationScope) : null;
  const tomorrowLocation = schedule ? workingLocationFor(schedule.events, tomorrowDate, locationScope) : null;
  // Preparation takes the plan's place beside the day view. Choosing the
  // meeting being prepared again closes it, and closing returns focus to
  // whatever opened it.
  const selectedKey = preparation?.event ? preparationKey(preparation.event) : null;
  const prepareTrigger = useRef<HTMLElement | null>(null);
  const closePreparation = () => {
    if (!preparation?.event) return;
    preparation.onClose();
    requestAnimationFrame(() => (prepareTrigger.current?.isConnected ? prepareTrigger.current : pageRef.current)?.focus());
  };
  const prepare = preparation ? (event: ScheduleEvent, trigger?: HTMLElement) => {
    if (selectedKey !== null && preparationKey(event) === selectedKey) { closePreparation(); return; }
    if (selectedKey === null) prepareTrigger.current = trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    preparation.onPrepare(event);
  } : undefined;
  const next = nextMeeting(todaysEvents, now);
  // The meetings arrow keys move between while preparing, in agenda order.
  const meetings = [...todaysEvents, ...(evening ? tomorrowList : [])].filter((event) => isPreparable(event) && !isDeclined(event));
  // Choosing an event prepares it; one that can't be prepared, such as a
  // working location, opens in Calendar as before.
  const chooseEvent = (event: ScheduleEvent, trigger?: HTMLElement) => (prepare && isPreparable(event) ? prepare(event, trigger) : onOpenEvent(event));
  // What is ready for each meeting still ahead, shown on the day view and the agenda.
  const ahead = [...meetingsToPrepare(todaysEvents, now), ...(evening ? meetingsToPrepare(tomorrowList, now) : [])];
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
    for (const event of meetingsWith(parseAddress(item.sender).email, evening ? [...todaysEvents, ...tomorrowList] : todaysEvents, now)) unreadFromAttendees.set(event, (unreadFromAttendees.get(event) ?? 0) + 1);
  }
  // A plan card and the block booked for its task highlight each other on hover or focus.
  const [linked, setLinked] = useState<string | null>(null);
  const plannedTasks = new Set((plan?.items ?? []).flatMap((item) => { const key = linkKey(item.taskId); return key ? [key] : []; }));
  const blockLink = (event: ScheduleEvent) => {
    const key = linkKey(event.sourceTaskId);
    return key && plannedTasks.has(key) ? { linked: key === linked, onLinkHover: (active: boolean) => setLinked(active ? key : null) } : {};
  };
  // Finished events dim, as the agenda's rows did; selected means only the meeting being prepared.
  const state = (event: ScheduleEvent) => ({
    past: isPastEvent(event, now),
    selected: selectedKey !== null && preparationKey(event) === selectedKey,
  });
  const decorateEvent = (event: ScheduleEvent) => {
    if (!isPreparable(event)) return { ...state(event), ...(event.sourceTaskId ? blockLink(event) : {}) };
    const found = readinessOf(event);
    const unread = unreadFromAttendees.get(event) ?? 0;
    const row = preparation ? found?.row : undefined;
    const hasNote = Boolean(row?.hasOccurrenceNote);
    const commitments = row?.openCommitments ?? 0;
    // Visible at a glance: a note mark, open commitments, and unread mail from attendees, each with its count.
    const marks = [
      hasNote ? <NotebookPen key="note" size={ICON_SIZE.xs} /> : null,
      commitments ? <span key="commitments" className="calendar-event-mark-count" title="Open commitments"><ListChecks size={ICON_SIZE.xs} />{commitments}</span> : null,
      unread ? <span key="unread" className="calendar-event-mark-count" title="Unread from attendees"><Mail size={ICON_SIZE.xs} />{unread}</span> : null,
    ].filter(Boolean);
    return {
      rowKey: `meeting:${preparationKey(event)}`,
      description: [row ? readinessSummary(row) : null, unread ? `${unread} unread from attendees` : null].filter(Boolean).join(" · ") || null,
      mark: marks.length ? <>{marks}</> : null,
      ...state(event),
      ...blockLink(event),
    };
  };
  const moveMeeting = (step: number) => {
    if (!preparation?.event || meetings.length === 0) return;
    const at = meetings.findIndex((event) => preparationKey(event) === selectedKey);
    const target = meetings[Math.min(meetings.length - 1, Math.max(0, at < 0 ? 0 : at + step))];
    if (target && preparationKey(target) !== selectedKey) preparation.onPrepare(target);
  };
  useWorkspaceCommands({
    // The meeting in focus (an agenda row or a day-view event), else the next meeting, else the first still to prepare.
    "meetings.prepare": () => {
      if (!preparation || !prepare) return;
      if (preparation.event) { closePreparation(); return; }
      const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest("[data-row-key]")?.getAttribute("data-row-key") : null;
      const target = meetings.find((event) => focused === `meeting:${preparationKey(event)}`) ?? next ?? ahead[0] ?? meetings[0];
      if (target) prepare(target);
    },
    "meetings.closePrepare": closePreparation,
    "meetings.next": () => moveMeeting(1),
    "meetings.previous": () => moveMeeting(-1),
  });
  const overdue = tasks.filter((task) => isActiveTaskStatus(task.status) && dueView(task, now) === "Overdue");
  const dueToday = tasks.filter((task) => dueView(task, now) === "Today");

  // Plans, tasks, and mail have their own places, so the summary line speaks only to the day's meetings.
  const dayLine = schedule ? daySummary(todaysEvents, now, { omitNext: next !== null }) : "";
  const progress = planProgress(plan);

  // Booking: Book takes the next free half hour today; a drop takes the slot under the pointer.
  const booking = { accounts: calendarAccounts, calendars, onNotice: notice };
  const undoBooking = (undo: () => Promise<void>) => () => void undo().then(
    () => notice({ message: "Booked time removed" }),
    (reason: unknown) => notice({ message: errorMessage(reason) }),
  );
  // A drop is an explicit time, so it books wherever it lands, but says when that is outside working hours.
  const outsideHours = (start: Date) => withinWorkingHours(start, workingHours) === false ? " (outside working hours)" : "";
  const bookAt = async (target: BlockTarget, start: Date) => {
    const booked = await bookBlock(target, start, booking);
    if (booked) notice({ message: `Booked ${booked.when}${outsideHours(start)} for “${target.title}”`, undo: undoBooking(booked.undo) });
  };
  const bookNext = async (target: BlockTarget) => {
    if (!schedule) { notice({ message: "Connect a calendar in Settings to book time" }); return; }
    // Unknown is not free: without today's schedule or working hours there is no free slot to offer.
    const known = schedule.error || schedule.loading ? null : todaysEvents;
    const start = nextFreeSlot(known, workingHours, new Date());
    if (start === "unknown") { notice({ message: "Today's calendar or working hours couldn't be read, so free time is unknown" }); return; }
    if (!start) { notice({ message: "No free half hour is left in today's working hours" }); return; }
    await bookAt(target, start);
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
  const dropOnDay = async (source: PlanDragSource, day: Date, startMinutes: number) => {
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, startMinutes);
    if (source.kind === "item") { await bookAt(itemBlock(source.item), start); return; }
    const resolved = await source.candidate.resolve();
    if (!resolved) return;
    // Book first: the time is what the user aimed at, and it stays if the plan can't take the item.
    const booked = await bookBlock(resolved.block, start, booking);
    if (!booked) return;
    try {
      const saved = await addToDailyPlan(planDate, [resolved.addition]);
      notice({
        message: `Booked ${booked.when}${outsideHours(start)} for “${resolved.title}” and added it to today's plan`,
        undo: () => { undoBooking(booked.undo)(); if (saved.changed.length) void saved.undo().catch(planWriteFailed); },
      });
    } catch (reason) {
      notice({
        message: `Booked ${booked.when} for “${resolved.title}”, but it couldn't be added to today's plan: ${errorMessage(reason)}`,
        undo: undoBooking(booked.undo),
        action: openNoteAction,
      });
    } finally {
      void reloadPlan();
    }
  };
  const gridRef = useRef<HTMLDivElement>(null);
  const dayStart = startOfLocalDay(now).getTime();
  const dragDays = useMemo(() => [new Date(dayStart)], [dayStart]);
  const drag = useTaskBlockDrag<PlanDragSource>({ gridRef, days: dragDays, blockMinutes: TASK_BLOCK_MINUTES, onDrop: (source, day, minutes) => void dropOnDay(source, day, minutes) });

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
        setTasks((current) => current?.map((candidate) => candidate.id === updated.id ? updated : candidate) ?? null);
      }
      setAnnouncement(`${finished ? "Reopened" : "Completed"}: ${item.title}`);
      onTasksChanged();
    } catch (reason) {
      notice({ message: errorMessage(reason), action: openNoteFor(item) });
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
  const [sourceId, setSourceId] = useLocalStorageState(PLAN_SOURCE_KEY, () => readLocalStorage(PLAN_SOURCE_KEY, (raw) => raw && SOURCE_IDS.includes(raw) ? raw : "suggested", "suggested"));
  const planned = new Set((plan?.items ?? []).map((item) => item.key));
  const taskCandidate = (task: ThreadTask): PlanCandidate => {
    const due = formatDue(task);
    const done = !isActiveTaskStatus(task.status);
    const resolved = { addition: { kind: "link" as const, reference: taskReference(task.id), label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: task.accountId }, title: task.title };
    return {
      id: `task:${task.id}`,
      title: task.title,
      planKey: planItemKey(taskReference(task.id)),
      resolve: async () => resolved,
      row: {
        as: "article",
        className: done ? "context-task context-task-done" : "context-task",
        control: <HoverTooltip label={done ? "Mark not done" : "Mark done"}>
          <input type="checkbox" className="context-task-checkbox" checked={done}
            aria-label={done ? `Mark ${task.title} not done` : `Mark ${task.title} done`} onChange={() => void toggleDone(task)} />
        </HoverTooltip>,
        wrapTitle: true,
        date: due ? `Due ${due}` : undefined,
        dateClassName: due && isOverdue(task) ? "task-due-overdue" : undefined,
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
      resolve: async () => reference ? { addition: { kind: "link", reference, label: name }, block: { title: `Check in with ${name}`, sourceTaskId: null, accountId: null }, title: name } : null,
      row: {
        icon: <ContactRound size={ICON_SIZE.sm} />,
        date: profile.keepInTouchDueAt ? describeDue(profile.keepInTouchDueAt) : undefined,
        detail: profile.displayName ? profile.addresses[0] : undefined,
        onActivate: () => onOpenContact(profile.id),
      },
    };
  };
  // Suggested, Starred Mail, Goals, and Notes come a page at a time from planSources, read only while their tab shows.
  const remoteKind = REMOTE_SOURCES[sourceId] ?? null;
  const remote = usePlanSourcePage(planDate, remoteKind, remoteKind === "notes" ? null : accountId, `${refreshKey}:${taskRefreshKey}:${plan?.revision ?? ""}`);
  const createForPlan = async (request: TaskFormRequest) => {
    if (!onCreateTask) return null;
    const created = await onCreateTask(request);
    if (created) onTasksChanged();
    return created;
  };
  const taskResolution = (task: Pick<ThreadTask, "id" | "title" | "accountId">) => ({ addition: { kind: "link" as const, reference: taskReference(task.id), label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: task.accountId }, title: task.title });
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
          ? { title: `Check in with ${candidate.title}`, sourceTaskId: null, accountId: null }
          : { title: candidate.title, sourceTaskId: candidate.taskId, accountId: candidate.accountId };
        return { addition: action.addition, block, title: candidate.title };
      }
      if (action.kind === "makeTask") {
        const promise = promises.find((item) => item.messageId === action.messageId);
        const created = promise ? await trackPromise(promise) : null;
        return created ? taskResolution(created) : null;
      }
      // Cancel in the editor writes nothing: no task, no plan line, no booking.
      const created = await createForPlan(action.kind === "taskForm"
        ? { kind: "conversation", threadId: action.threadId, accountId: action.accountId }
        : { kind: "goal", goalId: action.goalId, accountId: action.accountId });
      return created ? taskResolution(created) : null;
    };
    const isGoal = action.kind === "newTaskForGoal";
    const account = accountId === null ? candidate.accountId : null;
    const detail = isGoal
      ? [candidate.detail, candidate.openTasks !== null ? `${candidate.openTasks} open ${candidate.openTasks === 1 ? "task" : "tasks"}` : null, account]
      : [remoteKind === "suggested" ? SUGGESTED_REASONS[candidate.reason] : null, candidate.detail, candidate.reason === "goalTask" ? plannedDaysLabel(candidate.plannedDays) : candidate.goal?.title, account];
    return {
      id: `${remoteKind}:${candidate.key}`,
      title: candidate.title,
      planKey: isGoal ? null : candidate.key,
      inPlan: candidate.planned,
      addLabel: isGoal ? "New Task for Goal" : undefined,
      resolve,
      row: {
        as: "article",
        icon: candidate.reason === "starred" ? <Star size={ICON_SIZE.sm} /> : isGoal ? <Target size={ICON_SIZE.sm} /> : candidate.reason === "noteTask" ? <NotebookPen size={ICON_SIZE.sm} />
          : candidate.reason === "promise" ? <Send size={ICON_SIZE.sm} /> : candidate.reason === "keepInTouch" ? <ContactRound size={ICON_SIZE.sm} /> : undefined,
        wrapTitle: true,
        className: isGoal ? "today-goal-row" : undefined,
        detail: detail.filter(Boolean).join(" · ") || undefined,
        // A card opens its task, its note, or the starred conversation; adding never unstars or archives it.
        onActivate: candidate.taskId ? () => void openCandidateTask(candidate.taskId!) : candidate.threadId ? () => onOpenThread(candidate.threadId!)
          : candidate.contactId ? () => onOpenContact(candidate.contactId!) : undefined,
      },
    };
  };
  // Promises dismissed on this device stay out of Suggested too.
  const pageCandidates = (page: typeof remote, kind: PlanSourceKind | null) => page.candidates
    .filter((candidate) => candidate.action.kind !== "makeTask" || !dismissed.includes(candidate.action.messageId)).map(remoteCandidate(kind));
  // The plan and the day view cover every account; these sources follow the account in view, and say so beside the tabs.
  const scope = accountId ?? "All accounts";
  /** The switcher's tabs with `selected` showing, its remote cards from `page`. Choose leaves out Also Due, which Suggested covers. */
  const sourcesFor = (selected: string, page: typeof remote, choose: boolean): PlanSource[] => {
    const cards = pageCandidates(page, REMOTE_SOURCES[selected] ?? null);
    const remoteSource = (id: string, label: string, title: string, empty: string, extra: Partial<PlanSource> = {}): PlanSource => ({
      id, label, empty, ...extra,
      status: id !== selected ? undefined
        : page.error ? <p className="form-error" role="alert">{page.error}</p>
          : page.loading && page.candidates.length === 0 ? <p className="context-status">Reading…</p> : undefined,
      groups: id === selected ? [{
        id: `today-source-${id}`, title, candidates: cards, limit: cards.length,
        footer: page.next ? <button type="button" className="btn-link" disabled={page.loading} onClick={() => void page.loadMore()}>Show More</button> : undefined,
      }] : [],
    });
    return [
      remoteSource("suggested", "Suggested", "Suggested", "Nothing to suggest right now.", { scope }),
      ...(choose ? [] : [{
        id: "also-due", label: "Also Due", hidePlanned: true, scope, empty: loadedTasks ? "Nothing else is overdue or due today." : "",
        groups: [
          { id: "today-tasks-overdue", title: "Overdue", candidates: overdue.map(taskCandidate) },
          { id: "today-tasks-due", title: "Due today", candidates: dueToday.map(taskCandidate) },
        ],
      }]),
      {
        id: "tasks", label: "Tasks", empty: loadedTasks ? "No open tasks." : "", scope,
        footer: choose ? undefined : <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenTasks}>Open Tasks</button></p>,
        groups: [{ id: "today-tasks-open", title: "Open tasks", limit: 8, candidates: tasks.map(taskCandidate) }],
      },
      {
        id: "promises", label: "You Said You Would", scope, empty: "Nothing you said you would do is waiting.",
        groups: [{ id: "today-promises", title: "You said you would", limit: 5, candidates: openPromises.map(promiseCandidate) }],
      },
      {
        id: "keep-in-touch", label: "Keep in Touch", empty: "No one is due for a check-in.",
        groups: [{ id: "today-keep-in-touch", title: "Keep in touch", candidates: reconnect.map(contactCandidate),
          closing: reconnect.length && !choose ? <button type="button" className="btn-link" onClick={onOpenKeepInTouch}>Open Keep in Touch</button> : undefined }],
      },
      remoteSource("starred-mail", "Starred Mail", "Starred mail", "No starred conversations.", { overflow: true, scope }),
      remoteSource("goals", "Goals", "Goals", "No goals for this period.", { overflow: true, scope }),
      remoteSource("notes", "Notes", "Open note tasks", "No open tasks in other notes.", { overflow: true }),
    ];
  };
  const sources = sourcesFor(sourceId, remote, false);

  // Plan Today: the routine for today or tomorrow, its date fixed when it opens.
  const [routine, setRoutine] = useState<{ date: string; dayName: "Today" | "Tomorrow" } | null>(null);
  const openRoutine = useCallback((which: "today" | "tomorrow") => {
    const at = new Date();
    setRoutine({ date: todayTarget(which === "today" ? at : addDays(startOfLocalDay(at), 1)).date, dayName: which === "today" ? "Today" : "Tomorrow" });
  }, []);
  useEffect(() => {
    if (!planRequest) return;
    onPlanRequestConsumed?.(planRequest.id);
    if (planRequest.day === "review") toggleView();
    else openRoutine(planRequest.day);
  }, [planRequest, onPlanRequestConsumed, openRoutine, toggleView]);
  const [routineSourceId, setRoutineSourceId] = useState("suggested");
  const routineKind = routine ? REMOTE_SOURCES[routineSourceId] ?? null : null;
  const routinePage = usePlanSourcePage(routine?.date ?? planDate, routineKind, routineKind === "notes" ? null : accountId, `${refreshKey}:${taskRefreshKey}`);
  const routineEvents = schedule && routine ? agendaEventsFor(schedule.events ?? [], new Date(`${routine.date}T12:00:00`), calendars) : null;
  const eveningOffer = schedule && !schedule.loading && !schedule.error ? offersPlanTomorrow(todaysEvents, workingHours, now) : offersPlanTomorrow([], workingHours, now);
  const hasPlan = (plan?.items.length ?? 0) > 0;

  const selectedEvent = [...todaysEvents, ...tomorrowList].find((event) => selectedKey !== null && preparationKey(event) === selectedKey) ?? null;

  const needsReply = mail ? groupTodayMail(mail.threads).reply.length : null;
  const openCompletedTask = (task: PlanCompletedTask) => { if (task.noteId) onOpenNote?.(task.noteId); else void openCandidateTask(task.taskId); };

  return <section ref={pageRef} className="today-workspace" aria-label="Today" aria-busy={pull.refreshing}>
    <div className="today-page">
    <PullToRefreshStatus {...pull} />
    <header className="today-header">
      {backdrop ? <TodayBackdrop view={view} now={now} /> : null}
      <div className="today-heading">
        {/* Mail and tasks follow the account in view; the day view shows every selected calendar, as Calendar does. */}
        {/* The plan and the day view cover every account, so the account in view is named beside what it scopes: the mail line and the sources. */}
        <span className="eyebrow">Today</span>
        <h1>{now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" })}</h1>
        {dayLine || todayLocation ? <p className="today-day-shape">
          {todayLocation ? <span className="today-working-location"><WorkingLocationIcon type={todayLocation.workingLocationType} />{todayLocation.title}</span> : null}
          {todayLocation && dayLine ? " · " : null}{dayLine}
        </p> : null}
        {/* While a meeting is being prepared, its actions are in preparation, so the row is only a link. */}
        {next ? <NextMeeting event={next} now={now} onOpenEvent={onOpenEvent} {...(selectedKey === null ? { onAddNote, onPrepare: prepare, showJoin: true } : { showJoin: false })} /> : null}
      </div>
      <div className="today-routines">
        <div className="today-plan-status">
          {hasPlan
            ? <>{progress ? <span className="today-plan-progress">{progress}</span> : null}<button type="button" className="btn btn-sm" onClick={() => openRoutine("today")}>Edit Plan</button></>
            : evening ? null : <button type="button" className="btn btn-sm" onClick={() => openRoutine("today")}>Plan Today</button>}
          {/* In the evening view, Plan Tomorrow sits in the tomorrow column. */}
          {eveningOffer && !evening ? <button type="button" className="btn btn-sm" onClick={() => openRoutine("tomorrow")}>Plan Tomorrow</button> : null}
          {evening && dayClosedState ? <span className="today-day-closed" role="status">Day closed</span> : null}
          <button type="button" className="btn btn-sm" onClick={toggleView}>{evening ? "Back to Today's Plan" : "Review Today"}</button>
        </div>
      </div>
      <TodayMailSignal mail={mail} error={mailError} scope={scope} onOpenThread={onOpenThread} onOpenInbox={onOpenInbox} />
    </header>
    <div className="today-columns">
      {preparation?.event ? <div className="today-column today-preparation" role="group" aria-label="Preparation">
        {preparation.render(preparation.event, closePreparation)}
      </div> : evening ? <EveningReview
        today={planDate} tomorrow={tomorrowKey} now={now} plan={plan} reloadPlan={reloadPlan} refreshKey={`${refreshKey}:${taskRefreshKey}`}
        events={schedule ? todaysEvents : null} completedToday={completedToday} promises={openPromises} needsReply={needsReply}
        onNotice={notice} onOpenNote={(noteId) => onOpenNote?.(noteId)} onOpenInbox={onOpenInbox} onOpenThread={onOpenThread}
        onOpenPlanItem={(item) => void activatePlanItem(item)} onOpenCompletedTask={openCompletedTask} onOpenEvent={onOpenEvent}
        onPrepareMeeting={(event) => chooseEvent(event)} onAddMeetingNote={onAddNote}
        onMakeTask={(promise) => void trackPromise(promise)} onDismissPromise={(promise) => setDismissed(dismissPromise(promise.messageId))}
        onClosedChange={setDayClosed} />
      : <div className="today-column today-work" role="group" aria-label="Plan">
        <PlanCards plan={plan} error={planError} events={todaysEvents} now={now} drag={drag} linked={linked} onLink={setLinked}
          onToggle={(item) => void togglePlanItem(item)} onActivate={(item) => void activatePlanItem(item)}
          onBook={(item) => void bookNext(itemBlock(item))} onRemove={(item) => void removeItem(item)} />
        <PlanSourceSwitcher sources={sources} selected={sourceId} onSelect={setSourceId} planned={planned} drag={drag} onAdd={(candidate) => void addCandidate(candidate)} />
      </div>}
      {evening ? <EveningTomorrow
        tomorrow={tomorrowDate} now={now} events={tomorrowList} schedule={schedule} calendars={calendars} workingLocation={tomorrowLocation} carried={carriedToTomorrow(plan, tomorrowKey)}
        workingHours={workingHours} selectedEvent={selectedEvent} eventDecoration={decorateEvent} gridRef={gridRef} preview={null}
        onChooseEvent={chooseEvent} onOpenPlanItem={(item) => void activatePlanItem(item)} onPlanTomorrow={() => openRoutine("tomorrow")}
        onOpenCalendar={onOpenCalendar} onAddNote={onAddNote} onPrepare={prepare} selectedKey={selectedKey} />
      : <div className="today-column today-time" role="group" aria-label="Schedule">
        {schedule ? <>
          <TodayDayView events={todaysEvents} now={now} loading={schedule.loading} error={schedule.error} onRetry={schedule.reload}
            selectedEvent={selectedEvent} onChooseEvent={(event, trigger) => chooseEvent(event, trigger)} eventDecoration={decorateEvent}
            gridRef={gridRef} preview={drag.preview} dragging={drag.draggingId !== null} workingHours={workingHours} />
          {/* Below Today's breakpoint the agenda list takes the day view's place (a container query in CSS). */}
          <div className="today-agenda-fallback">
            {/* The page heading already names today, so its agenda leaves the date out. */}
            <CalendarAgenda pullToRefresh={false} today={now} now={now} showDate={false} workingLocation={null} markConflicts calendars={calendars} schedule={schedule} onOpenEvent={chooseEvent} onAddNote={onAddNote} onPrepare={prepare} selectedKey={selectedKey} describe={preparation ? describeReadiness : undefined} />
          </div>
          <CalendarAgenda pullToRefresh={false} today={addDays(startOfLocalDay(now), 1)} day="tomorrow" workingLocation={tomorrowLocation} calendars={calendars} schedule={schedule} onOpenEvent={chooseEvent} onAddNote={onAddNote} onPrepare={prepare} selectedKey={selectedKey} />
        </>
          : <ContextSection id="calendar-agenda" title="Today's agenda" rows={[]}
            footer={<p className="context-status">Connect a calendar in Settings to see today's agenda here.</p>} />}
        <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenCalendar}>Open Calendar</button></p>
      </div>}
    </div>
    </div>
    <p className="sr-only" aria-live="polite">{announcement}</p>
    {routine ? <PlanTodayDialog
      key={routine.date}
      date={routine.date}
      dayName={routine.dayName}
      sources={sourcesFor(routineSourceId, routinePage, true)}
      sourceId={routineSourceId}
      onSelectSource={setRoutineSourceId}
      events={routineEvents}
      scheduleLoading={schedule?.loading ?? false}
      scheduleError={schedule?.error ?? null}
      onRetrySchedule={() => schedule?.reload()}
      workingHours={workingHours}
      booking={booking}
      preparation={preparation ? { render: preparation.render, ownAddresses: preparation.ownAddresses } : undefined}
      onNotice={notice}
      onOpenNote={(noteId, href) => (preparation ? preparation.onOpenNote(noteId, href) : onOpenNote?.(noteId))}
      onSaved={() => { void reloadPlan(); onTasksChanged(); }}
      onClose={() => setRoutine(null)}
    /> : null}
  </section>;
}

function NextMeeting({ event, now, onOpenEvent, onAddNote, onPrepare, showJoin }: { event: ScheduleEvent; now: Date; onOpenEvent(event: ScheduleEvent): void; onAddNote?(event: ScheduleEvent): void; onPrepare?(event: ScheduleEvent): void; showJoin: boolean }) {
  const { joinUrl } = meetingPlace(event.location ?? null, event.conferenceUrl);
  const minutes = Math.round((eventDate(event).getTime() - now.getTime()) / 60_000);
  const label = minutes <= 0
    ? `Now: ${event.title} · until ${formatClockTime(new Date(event.end))}`
    : `Next: ${event.title} ${minutes < 60 ? `in ${minutes} min` : `at ${formatClockTime(eventDate(event))}`}`;
  return <div className="today-next">
    <button type="button" className="btn-link" onClick={() => onOpenEvent(event)}>{label}</button>
    {showJoin && joinUrl ? <button type="button" className="btn btn-sm" onClick={() => void openUrl(joinUrl)}><Video size={ICON_SIZE.sm} />Join</button> : null}
    {onPrepare && isPreparable(event) ? <button type="button" className="btn btn-sm" onClick={() => onPrepare(event)}><ClipboardList size={ICON_SIZE.sm} />Prepare</button> : null}
    {onAddNote && isPreparable(event) ? <button type="button" className="btn btn-sm" onClick={() => onAddNote(event)}><NotebookPen size={ICON_SIZE.sm} />Add Note</button> : null}
  </div>;
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
