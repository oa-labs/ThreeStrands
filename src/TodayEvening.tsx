import { Mail, Send } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { Modal } from "./AppChrome";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatClockTime, isDeclined } from "./agendaEvents";
import { MAX_SHUTDOWN_NOTE_CHARS } from "./dailyPlan";
import { mailClient } from "./data/client";
import type { CalendarOption, DailyPlan, DailyPlanItem, OpenPromise, PlanCompletedTask, PlanReview, PlanReviewItem, ScheduleEvent, ThreadTask } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { MeetingFollowThroughList } from "./MeetingReadinessList";
import { promiseRecipients } from "./openPromises";
import { saveDailyPlan } from "./noteCapture";
import { useNoteChanges } from "./noteMutations";
import { newOperationId } from "./planRoutine";
import { ReviewDecisionRow, ReviewDone, shortPlanDate, type DecisionKind } from "./PlanReviewColumns";
import { dayClosed, decisionChanges, decisionEntry, looseEnds, reviewsToday } from "./eveningView";
import { WorkingLocationIcon } from "./WorkingLocation";
import { TodayDayView } from "./TodayDayView";
import type { DayGridEventDecoration, DayGridPreview } from "./DayTimeGrid";
import type { WorkingHours } from "./todayInsights";
import type { Notice } from "./useNotice";

/**
 * The review of today's plan, read for tomorrow (the plans before tomorrow,
 * newest first, with deferrals due by then), again whenever a note changes or
 * `refreshKey` does. A failed read keeps the review last read and reports it.
 */
export function useEveningReview(tomorrow: string, refreshKey: string) {
  const [review, setReview] = useState<PlanReview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const version = useRef(0);
  const reload = useCallback(async () => {
    const current = ++version.current;
    try {
      const next = await mailClient.planReview(tomorrow);
      if (current !== version.current) return;
      setReview(next);
      setError(null);
    } catch (reason) {
      if (current !== version.current) return;
      setError(errorMessage(reason));
    }
  }, [tomorrow]);
  const cancel = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void reload().catch(logBackgroundFailure("Evening review"));
    return cancel;
  }, [reload, refreshKey, cancel]);
  useNoteChanges(useCallback(() => { void reload().catch(logBackgroundFailure("Evening review")); }, [reload]));
  return { review: review?.date === tomorrow ? review : null, error, reload };
}

/** A meeting that counts as attended: timed, not declined, not a working location, and over. */
const attended = (event: ScheduleEvent, now: Date) => !event.allDay && !isDeclined(event) && event.eventType !== "workingLocation" && new Date(event.end).getTime() <= now.getTime();

const sameDay = (iso: string, now: Date) => { const at = new Date(iso); return at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate(); };

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

export type EveningReviewProps = {
  /** Today's and tomorrow's dates, `YYYY-MM-DD`. */
  today: string;
  tomorrow: string;
  now: Date;
  /** Today's plan, and any error reading it. */
  plan: DailyPlan | null;
  reloadPlan(): Promise<void> | void;
  refreshKey: string;
  /** Today's meetings from the calendar, or null without a connected calendar. */
  events: ScheduleEvent[] | null;
  /** Tasks completed today, for a day without a plan to review. */
  completedToday: ThreadTask[];
  /** Today's open promises, dismissed ones already left out. */
  promises: OpenPromise[];
  /** Mail that needs a reply, or null when mail couldn't be read. */
  needsReply: number | null;
  onNotice(notice: Notice): void;
  onOpenNote(noteId: string): void;
  onOpenInbox(): void;
  onOpenThread(threadId: string): void;
  onOpenPlanItem(item: DailyPlanItem): void;
  onOpenCompletedTask(task: PlanCompletedTask): void;
  onOpenEvent(event: ScheduleEvent): void;
  onPrepareMeeting(event: ScheduleEvent): void;
  onAddMeetingNote?(event: ScheduleEvent): void;
  onMakeTask(promise: OpenPromise): void;
  onDismissPromise(promise: OpenPromise): void;
  /** The day is closed (true) or not; reported whenever it changes. */
  onClosedChange(closed: boolean): void;
};

/**
 * Today in review, the evening view's left column: what was done, the loose
 * ends with a decision each, and a note on the day. Nothing here is a form:
 * Defer… and Add Notes on the Day… open small dialogs, and every decision is
 * one guarded write to today's note with Undo.
 */
export function EveningReview(props: EveningReviewProps) {
  const { today, tomorrow, now, plan, events, completedToday, promises, needsReply, onNotice, onClosedChange } = props;
  const { review, error: reviewError, reload: reloadReview } = useEveningReview(tomorrow, props.refreshKey);
  const [deferring, setDeferring] = useState<PlanReviewItem | null>(null);
  const [noting, setNoting] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [outstanding, setOutstanding] = useState<number | null>(null);

  const rows = looseEnds(review, plan);
  // Without a calendar there are no meetings to review.
  const meetingsOutstanding = events ? outstanding : 0;
  useEffect(() => { onClosedChange(dayClosed(meetingsOutstanding, review ? rows.length : null)); }, [meetingsOutstanding, review, rows.length, onClosedChange]);

  const openNote = plan ? { label: "Open Note", run: () => props.onOpenNote(plan.noteId) } : undefined;
  const refreshAfterWrite = () => { void Promise.resolve(props.reloadPlan()).catch(logBackgroundFailure("Daily plan")); void reloadReview().catch(logBackgroundFailure("Evening review")); };
  const failed = (reason: unknown) => onNotice({ message: errorMessage(reason), action: openNote });

  /** One loose-end decision: a single item-level write to today's note, with Undo. */
  const decide = async (row: PlanReviewItem, kind: DecisionKind, deferUntil?: string) => {
    const entry = decisionEntry(row, kind, tomorrow, deferUntil);
    if (!entry || busy) return;
    const { operationId, changes } = decisionChanges(entry);
    setBusy(row.key);
    try {
      const saved = await saveDailyPlan(today, plan?.revision ?? null, operationId, changes);
      const message = kind === "carry" ? `Carried “${row.title}” to tomorrow`
        : kind === "drop" ? `Dropped “${row.title}” from planning`
          : `Deferred “${row.title}” to ${shortPlanDate(deferUntil!)}`;
      onNotice({ message, undo: () => void saved.undo().then(refreshAfterWrite, failed) });
    } catch (reason) {
      failed(reason);
    } finally {
      setBusy(null);
      refreshAfterWrite();
    }
  };

  const saveNotes = async (text: string) => {
    const saved = await saveDailyPlan(today, plan?.revision ?? null, newOperationId(), { items: [], remove: [], notToday: [], shutdown: text });
    onNotice({ message: "Added notes on the day to today's note", undo: () => void saved.undo().then(refreshAfterWrite, failed) });
    refreshAfterWrite();
  };

  // Done today. Without today's own plan to review, tasks completed today and the day's meetings still show.
  const planned = reviewsToday(review, today);
  const meetings = events ? events.filter((event) => attended(event, now)) : planned ? review?.meetings ?? [] : [];
  const alsoDone: PlanCompletedTask[] = planned ? review!.alsoDone : completedToday.map((task) => ({
    key: `task:${task.id}`, taskId: task.id, title: task.title, accountId: task.accountId, noteId: null, noteTitle: null, completedAt: task.completedAt ?? "",
  }));

  const todaysPromises = promises.filter((promise) => sameDay(promise.sentAt, now));
  const nothingLoose = rows.length === 0 && todaysPromises.length === 0 && !needsReply;

  return <div className="today-column today-evening" role="group" aria-label="Today in review">
    <h2 className="today-evening-title">Today in review</h2>
    <section className="today-evening-section" aria-labelledby="evening-done">
      <h3 id="evening-done">Done today</h3>
      {review === null && reviewError ? <p className="form-error" role="alert">{reviewError}</p> : null}
      <ReviewDone done={planned ? review!.done : []} meetings={meetings} alsoDone={alsoDone} empty="Nothing finished yet today."
        onOpenItem={props.onOpenPlanItem} onOpenTask={props.onOpenCompletedTask} onOpenMeeting={props.onOpenEvent} />
    </section>
    <section className="today-evening-section" aria-labelledby="evening-loose">
      <h3 id="evening-loose">Loose ends</h3>
      {rows.length ? <>
        <h4>Unfinished planned items</h4>
        <ul className="plan-review-list">
          {rows.map((row) => <ReviewDecisionRow key={row.key} item={row} busy={busy !== null} carryLabel="Carry to Tomorrow" onDecide={(kind) => { if (kind === "defer") setDeferring(row); else void decide(row, kind); }} />)}
        </ul>
        <p className="context-status">Items left undecided come back in tomorrow morning's review.</p>
      </> : null}
      {events ? <MeetingFollowThroughList events={events} now={now} refreshKey={props.refreshKey} onPrepare={props.onPrepareMeeting} onNotice={onNotice}
        onAddNote={props.onAddMeetingNote} onOutstanding={setOutstanding} /> : null}
      {todaysPromises.length ? <ContextSection id="evening-promises" title="You said you would" count={todaysPromises.length} rows={todaysPromises.map((promise) => <ContextRow
        key={promise.messageId}
        as="article"
        icon={<Send size={ICON_SIZE.sm} />}
        title={promise.excerpt}
        wrapTitle
        date={formatClockTime(new Date(promise.sentAt))}
        detail={`To ${promiseRecipients(promise)} · Nothing sent to them since`}
        onActivate={() => props.onOpenThread(promise.thread.id)}
        trailing={<>
          <button type="button" className="btn btn-sm" onClick={() => props.onMakeTask(promise)}>Make a Task</button>
          <button type="button" className="btn btn-sm" onClick={() => props.onDismissPromise(promise)}>Dismiss</button>
        </>}
      />)} /> : null}
      {needsReply ? <ContextRow icon={<Mail size={ICON_SIZE.sm} />} title={`${plural(needsReply, "conversation needs", "conversations need")} a reply`} detail="Open Inbox" onActivate={props.onOpenInbox} /> : null}
      {nothingLoose && review ? <p className="context-status">No loose ends.</p> : null}
      {review === null && !reviewError ? <p className="context-status" role="status">Reading today's plan…</p> : null}
    </section>
    <section className="today-evening-section" aria-labelledby="evening-notes">
      <h3 id="evening-notes">Notes on the day</h3>
      <div><button type="button" className="btn btn-sm" onClick={() => setNoting(true)}>Add Notes on the Day…</button></div>
    </section>
    {deferring ? <DeferDialog item={deferring} min={tomorrow} onClose={() => setDeferring(null)}
      onSubmit={(date) => { const row = deferring; setDeferring(null); void decide(row, "defer", date); }} /> : null}
    {noting ? <ShutdownNotesDialog onClose={() => setNoting(false)} onSave={saveNotes} onOpenNote={plan ? () => props.onOpenNote(plan.noteId) : undefined} /> : null}
  </div>;
}

/** Defer…: when the item should come back, from tomorrow on. Nothing is written until Defer. */
export function DeferDialog({ item, min, onClose, onSubmit }: { item: PlanReviewItem; min: string; onClose(): void; onSubmit(date: string): void }) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [date, setDate] = useState(min);
  const valid = /^\d{4}-\d{2}-\d{2}$/.test(date) && date >= min;
  return <Modal title="Defer" onClose={onClose} initialFocusRef={inputRef}>
    <form className="modal-form" onSubmit={(event) => { event.preventDefault(); if (valid) onSubmit(date); }}
      onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); event.currentTarget.requestSubmit(); } }}>
      <p className="modal-form-context">{item.title}</p>
      <label>Defer Until<input ref={inputRef} type="date" min={min} value={date} onChange={(event) => setDate(event.target.value)} aria-describedby="defer-hint" /></label>
      <p className="modal-form-hint" id="defer-hint">It comes back in the review on that day. Tomorrow is the earliest.</p>
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!valid}>Defer</button>
      </div>
    </form>
  </Modal>;
}

/** Add Notes on the Day…: one paragraph for today's note, kept under Shutdown at the end of its plan. */
export function ShutdownNotesDialog({ onClose, onSave, onOpenNote }: { onClose(): void; onSave(text: string): Promise<void>; onOpenNote?(): void }) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const length = [...text.trim()].length;
  const over = length - MAX_SHUTDOWN_NOTE_CHARS;
  const submit = async () => {
    if (!length || over > 0 || saving) return;
    setSaving(true); setError(null);
    try { await onSave(text); onClose(); } catch (reason) { setError(errorMessage(reason)); setSaving(false); }
  };
  return <Modal title="Notes on the Day" onClose={onClose} initialFocusRef={inputRef}>
    <form className="modal-form" onSubmit={(event) => { event.preventDefault(); void submit(); }}
      onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); event.currentTarget.requestSubmit(); } }}>
      <label>Notes<textarea ref={inputRef} rows={5} value={text} disabled={saving} onChange={(event) => setText(event.target.value)} aria-describedby="shutdown-hint" /></label>
      <p className="modal-form-hint" id="shutdown-hint">Added as one paragraph under Shutdown, at the end of today's plan in your daily note.</p>
      {over > 0 ? <p className="form-error" role="alert">Shorten the notes by {plural(over, "character", "characters")}.</p> : null}
      {error ? <p className="form-error" role="alert">{error}{onOpenNote ? <> <button type="button" className="btn-link" onClick={onOpenNote}>Open Note</button></> : null}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={saving || !length || over > 0}>{saving ? "Adding…" : "Add to Today's Note"}</button>
      </div>
    </form>
  </Modal>;
}

export type EveningTomorrowProps = {
  tomorrow: Date;
  now: Date;
  events: readonly ScheduleEvent[];
  /** Today's schedule through tomorrow, or null without a connected calendar. */
  schedule: { events: ScheduleEvent[]; loading: boolean; error: string | null; reload(): void } | null;
  calendars: CalendarOption[];
  carried: DailyPlanItem[];
  /** The one working location for tomorrow, stated beside its date. */
  workingLocation?: ScheduleEvent | null;
  workingHours: WorkingHours | null;
  selectedEvent: ScheduleEvent | null;
  eventDecoration(event: ScheduleEvent): DayGridEventDecoration | null;
  gridRef: RefObject<HTMLDivElement | null>;
  preview: DayGridPreview | null;
  onChooseEvent(event: ScheduleEvent, trigger?: HTMLElement): void;
  onOpenPlanItem(item: DailyPlanItem): void;
  onPlanTomorrow(): void;
  onOpenCalendar(): void;
  onAddNote?(event: ScheduleEvent): void;
  onPrepare?(event: ScheduleEvent): void;
  selectedKey: string | null;
};

/**
 * Tomorrow, the evening view's right column: what was carried to tomorrow
 * (the start of tomorrow's plan), Plan Tomorrow, and tomorrow's day view with
 * working hours shaded and readiness marks. Tomorrow's agenda list is not
 * repeated; it appears only where the day view gives way to it in a narrow window.
 */
export function EveningTomorrow(props: EveningTomorrowProps) {
  const { tomorrow, now, schedule, carried } = props;
  const name = tomorrow.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });
  return <div className="today-column today-time today-tomorrow" role="group" aria-label="Tomorrow">
    <h2 className="today-evening-title">{name}</h2>
    {props.workingLocation ? <p className="today-day-shape"><span className="today-working-location"><WorkingLocationIcon type={props.workingLocation.workingLocationType} />{props.workingLocation.title}</span></p> : null}
    <section className="today-evening-section" aria-labelledby="tomorrow-carried">
      <h3 id="tomorrow-carried">Carried to tomorrow</h3>
      {carried.length
        ? <ul className="plan-review-list">{carried.map((item) => <li key={item.key}><button type="button" className="btn-link" onClick={() => props.onOpenPlanItem(item)}>{item.title}</button></li>)}</ul>
        : <p className="context-status">Nothing carried to tomorrow yet.</p>}
      <div><button type="button" className="btn btn-sm" onClick={props.onPlanTomorrow}>Plan Tomorrow</button></div>
    </section>
    {schedule ? <>
      <TodayDayView events={[...props.events]} now={now} day={tomorrow} label="Tomorrow's day view" loading={schedule.loading} error={schedule.error} onRetry={schedule.reload}
        selectedEvent={props.selectedEvent} onChooseEvent={(event, trigger) => props.onChooseEvent(event, trigger)} eventDecoration={props.eventDecoration}
        gridRef={props.gridRef} preview={props.preview} dragging={false} workingHours={props.workingHours} />
      <div className="today-agenda-fallback">
        <CalendarAgenda pullToRefresh={false} today={tomorrow} day="tomorrow" workingLocation={props.workingLocation} calendars={props.calendars} schedule={schedule} onOpenEvent={props.onChooseEvent} onAddNote={props.onAddNote} onPrepare={props.onPrepare} selectedKey={props.selectedKey} />
      </div>
    </> : <p className="context-status">Connect a calendar in Settings to see tomorrow's day here.</p>}
    <p className="today-column-link"><button type="button" className="btn-link" onClick={props.onOpenCalendar}>Open Calendar</button></p>
  </div>;
}
