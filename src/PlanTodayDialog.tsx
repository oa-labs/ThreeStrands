import { ArrowDown, ArrowUp, CalendarClock, CircleMinus, Copy } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { Modal } from "./AppChrome";
import { formatClockTime, isDeclined } from "./agendaEvents";
import { planItemKey, type PlanAddition } from "./dailyPlan";
import type { PlanReview, PlanReviewItem, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { formatShortcut } from "./shortcutLabels";
import { isPreparable } from "./meetingPrep";
import { MeetingReadinessList } from "./MeetingReadinessList";
import { addItem, addNewTask, carryAll, decide, hasChoices, isDirty, moveItem, PLAN_CARRY_HINT, PLAN_ITEMS_NOTE, PLAN_STEP_LABELS, PLAN_STEPS, planText, previewLines, removeItem, type PlanStep, type RoutineItem, type RoutineState } from "./planRoutine";
import { TASK_BLOCK_MINUTES } from "./taskBooking";
import { bookBlock, nextFreeSlot, type BlockTarget, type BookingContext } from "./todayBooking";
import { TodayDayView } from "./TodayDayView";
import { planCapacity, type WorkingHours } from "./todayInsights";
import { carriedLabel, PlanSourceSwitcher, type PlanCandidate, type PlanSource } from "./TodayPlan";
import type { Notice } from "./useNotice";
import { reviewHasContent, usePlanRoutine, type SearchableTask } from "./usePlanRoutine";
import { useTaskBlockDrag } from "./useTaskBlockDrag";
import { errorMessage } from "./errors";

/** Most search results the Choose step lists at once. */
export const PLAN_SEARCH_RESULTS = 8;

export type PlanTodayDialogProps = {
  /** The plan date, resolved once when the routine opened. */
  date: string;
  /** "Today" or "Tomorrow", for the title and messages. */
  dayName: "Today" | "Tomorrow";
  /** The source switcher's tabs for this date, as Today builds them. */
  sources: PlanSource[];
  sourceId: string;
  onSelectSource(id: string): void;
  /** The plan date's events, or null without a connected calendar. */
  events: ScheduleEvent[] | null;
  scheduleLoading: boolean;
  scheduleError: string | null;
  onRetrySchedule(): void;
  workingHours: WorkingHours | null;
  booking: BookingContext;
  /** Meeting preparation, shown inside the dialog's Meetings step. */
  preparation?: { render(event: ScheduleEvent, close: () => void): ReactNode; ownAddresses: readonly string[] };
  onNotice(notice: Notice): void;
  onOpenNote(noteId: string, href?: string): void;
  /** After Save Plan, with the save's Undo already offered. */
  onSaved(): void;
  onClose(): void;
};

const day = (date: string) => new Date(`${date}T12:00:00`);
const weekday = (date: string) => new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(day(date));
const shortDate = (date: string) => new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(day(date));
const nextDay = (date: string) => { const at = day(date); at.setDate(at.getDate() + 1); return `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`; };
/** "Yesterday" for the day before the plan date, else its weekday. */
const reviewedName = (reviewed: string, date: string) => nextDay(reviewed) === date ? "Yesterday" : weekday(reviewed);
const minutesLabel = (minutes: number) => minutes < 60 ? `${minutes} min` : minutes % 60 ? `${Math.floor(minutes / 60)} h ${minutes % 60} min` : `${minutes / 60} h`;

/**
 * Plan Today: one modal form with five steps (Review, Choose, Meetings, Fit,
 * Save). Steps can be skipped from the step list; nothing is written to the
 * note until Save Plan, except calendar bookings made in Fit, which happen at
 * once with Undo, as on Today. Escape asks whether to keep or discard the
 * routine's choices, which are otherwise kept on this device and restored
 * when the routine opens again.
 */
export function PlanTodayDialog(props: PlanTodayDialogProps) {
  const { date, dayName, onClose, onNotice, onSaved } = props;
  const routine = usePlanRoutine(date);
  const { state, update } = routine;
  const [confirmClose, setConfirmClose] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const steps = useMemo(() => PLAN_STEPS.filter((step) => step !== "review" || reviewHasContent(routine.review) || Boolean(routine.reviewError)), [routine.review, routine.reviewError]);
  const step = state?.step ?? "choose";
  const at = steps.indexOf(step);

  // Focus goes to the step's first control whenever a step shows.
  useEffect(() => {
    if (!state || confirmClose) return;
    const first = bodyRef.current?.querySelector<HTMLElement>("button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex='-1'])");
    first?.focus();
  }, [step, Boolean(state), confirmClose]); // eslint-disable-line react-hooks/exhaustive-deps

  const go = (target: PlanStep) => update((current) => ({ ...current, step: target }));
  const requestClose = () => {
    if (confirmClose) { setConfirmClose(false); return; }
    if (state && hasChoices(state)) setConfirmClose(true);
    else { routine.leave(false); onClose(); }
  };
  const leave = (keep: boolean) => { routine.leave(keep); onClose(); };
  const save = async () => {
    if (!state || routine.saving) return;
    if (!isDirty(state)) { routine.leave(false); onClose(); return; }
    const result = await routine.save();
    if (!result) return;
    const openNote = { label: "Open Note", run: () => props.onOpenNote(result.note.id) };
    onNotice({
      message: result.already.length && !result.added.length ? `${dayName}'s plan was already up to date` : `Saved ${dayName.toLowerCase()}'s plan`,
      undo: () => void result.undo().catch((reason: unknown) => onNotice({ message: errorMessage(reason), action: openNote })),
    });
    onSaved();
    onClose();
  };
  const submitShortcut = (event: ReactKeyboardEvent<HTMLFormElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      void save();
    }
  };

  const title = `Plan ${dayName}`;
  return <Modal title={title} className="plan-routine-modal" onClose={requestClose}>
    <form ref={formRef} className="modal-form plan-routine" onKeyDown={submitShortcut} onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <p className="modal-form-hint">{dayName === "Tomorrow" ? `Planning ${shortDate(date)}.` : `Planning today, ${shortDate(date)}.`}</p>
      {routine.loadError ? <p className="form-error" role="alert">{routine.loadError}</p> : null}
      {state ? <>
        <nav aria-label="Steps">
          <ol className="plan-routine-steps">
            {steps.map((candidate, index) => <li key={candidate}>
              <button type="button" className={`plan-routine-step${candidate === step ? " active" : ""}`} aria-current={candidate === step ? "step" : undefined}
                disabled={confirmClose} onClick={() => go(candidate)}>{index + 1}. {PLAN_STEP_LABELS[candidate]}</button>
            </li>)}
          </ol>
        </nav>
        {routine.restored ? <p className="plan-routine-restored" role="status">
          Restored the choices you hadn't saved ({new Date(routine.restored).toLocaleString(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" })}).{" "}
          <button type="button" className="btn-link" onClick={routine.discard}>Discard</button>
        </p> : null}
        {confirmClose ? <div className="plan-routine-confirm" role="group" aria-label="Close Plan" ref={bodyRef}>
          <p>Keep the choices you made for later? They stay on this device until you save the plan or discard them.</p>
          <div className="modal-form-actions">
            <button type="button" className="btn" onClick={() => setConfirmClose(false)}>Go Back</button>
            <button type="button" className="btn" onClick={() => leave(false)}>Discard Choices</button>
            <button type="button" className="btn btn-primary" onClick={() => leave(true)}>Keep for Later</button>
          </div>
        </div> : <div className="plan-routine-body" ref={bodyRef} role="group" aria-label={PLAN_STEP_LABELS[step]}>
          {step === "review" ? <ReviewStep state={state} review={routine.review} error={routine.reviewError} date={date} update={update} />
            : step === "choose" ? <ChooseStep {...props} state={state} tasks={routine.tasks} update={update} />
              : step === "meetings" ? <MeetingsStep {...props} />
                : step === "fit" ? <FitStep {...props} state={state} update={update} />
                  : <SaveStep state={state} dayName={dayName} onNotice={onNotice} />}
        </div>}
        {routine.saveError && !confirmClose ? <p className="form-error" role="alert">
          {routine.saveError}{" "}
          {routine.plan ? <button type="button" className="btn-link" onClick={() => props.onOpenNote(routine.plan!.noteId)}>Open Note</button> : null}
        </p> : null}
        {confirmClose ? null : <div className="modal-form-actions plan-routine-actions">
          <button type="button" className="btn" onClick={requestClose}>Cancel</button>
          <button type="button" className="btn" disabled={at <= 0} onClick={() => go(steps[at - 1])}>Back</button>
          {at < steps.length - 1
            ? <button type="button" className="btn btn-primary" onClick={() => go(steps[at + 1])}>Next</button>
            : <button type="submit" className="btn btn-primary" disabled={routine.saving}>{routine.saving ? "Saving…" : "Save Plan"}</button>}
        </div>}
      </> : routine.loadError ? <div className="modal-form-actions"><button type="button" className="btn" onClick={onClose}>Close</button></div>
        : <p className="context-status" role="status">Reading the plan…</p>}
    </form>
  </Modal>;
}

type Update = (change: (state: RoutineState) => RoutineState) => void;

function ReviewStep({ state, review, error, date, update }: { state: RoutineState; review: PlanReview | null; error: string | null; date: string; update: Update }) {
  const [deferring, setDeferring] = useState<string | null>(null);
  if (!review) return <p className="form-error" role="alert">{error ?? "The last plan couldn't be reviewed."}</p>;
  const name = review.reviewed ? reviewedName(review.reviewed, date) : null;
  const undecided = review.didntGetTo.filter((row) => !state.decisions[row.key] && row.state !== "unavailable");
  const groups: { plannedOn: string; rows: PlanReviewItem[] }[] = [];
  for (const row of review.didntGetTo) {
    const last = groups.at(-1);
    if (last?.plannedOn === row.plannedOn) last.rows.push(row);
    else groups.push({ plannedOn: row.plannedOn, rows: [row] });
  }
  const earliestDefer = nextDay(date);
  const row = (item: PlanReviewItem) => {
    const decision = state.decisions[item.key];
    const carries = item.carryCount ?? 0;
    const unavailable = item.state === "unavailable";
    const detail = [unavailable ? "Unavailable on this device" : null, carriedLabel(carries)].filter(Boolean).join(" · ");
    const choose = (next: Parameters<typeof decide>[2]) => update((current) => decide(current, item, decision?.kind === next?.kind && next?.kind !== "defer" ? null : next));
    return <li key={item.key} className="plan-review-row">
      <div className="plan-review-text">
        <strong>{item.title}</strong>
        {detail ? <small>{detail}</small> : null}
        {carries >= PLAN_CARRY_HINT ? <small className="plan-review-hint">Carried {carries} times. Defer or drop it?</small> : null}
        {decision?.kind === "defer" ? <small>Deferred to {shortDate(decision.date)}</small> : null}
      </div>
      <div className="plan-review-actions" role="group" aria-label={`Decide: ${item.title}`}>
        <button type="button" className="btn btn-sm" aria-pressed={decision?.kind === "carry"} disabled={unavailable} onClick={() => choose({ kind: "carry" })}>Carry</button>
        <button type="button" className="btn btn-sm" aria-pressed={decision?.kind === "defer"} aria-expanded={deferring === item.key} onClick={() => setDeferring(deferring === item.key ? null : item.key)}>Defer…</button>
        <button type="button" className="btn btn-sm" aria-pressed={decision?.kind === "drop"} onClick={() => choose({ kind: "drop" })}>Drop</button>
      </div>
      {deferring === item.key ? <label className="plan-review-defer">Defer Until
        <input type="date" min={earliestDefer} defaultValue={decision?.kind === "defer" ? decision.date : earliestDefer}
          onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); const value = event.currentTarget.value; if (value >= earliestDefer) { choose({ kind: "defer", date: value }); setDeferring(null); } } }}
          onChange={(event) => { const value = event.currentTarget.value; if (value >= earliestDefer) choose({ kind: "defer", date: value }); }} />
      </label> : null}
    </li>;
  };
  return <div className="plan-review-columns">
    <section className="plan-review-column" aria-labelledby="plan-review-done">
      <h3 id="plan-review-done">Done</h3>
      {name ? <p className="context-status">{name}'s plan</p> : null}
      <ul className="plan-review-list">
        {review.done.map((item) => <li key={item.key}>{item.title}</li>)}
        {(review.meetings ?? []).map((event) => <li key={`${event.id}:${event.start}`}>{event.title} <small>{formatClockTime(new Date(event.start))}</small></li>)}
      </ul>
      {review.alsoDone.length ? <>
        <h4>Also done</h4>
        <ul className="plan-review-list">{review.alsoDone.map((task) => <li key={task.key}>{task.title}</li>)}</ul>
      </> : null}
      {!review.done.length && !review.alsoDone.length && !review.meetings?.length ? <p className="context-status">Nothing finished from the last plan.</p> : null}
    </section>
    <section className="plan-review-column" aria-labelledby="plan-review-left">
      <h3 id="plan-review-left">Didn't Get To</h3>
      {review.didntGetTo.length ? <>
        <div className="plan-review-all-actions"><button type="button" className="btn btn-sm" disabled={!undecided.length} onClick={() => update((current) => carryAll(current, review.didntGetTo))}>Carry All</button></div>
        {groups.map((group) => <div key={group.plannedOn}>
          <p className="context-status">Planned {shortDate(group.plannedOn)}</p>
          <ul className="plan-review-list">{group.rows.map(row)}</ul>
        </div>)}
        <p className="context-status">Items left undecided come back next time.</p>
      </> : <p className="context-status">Nothing left over.</p>}
    </section>
  </div>;
}

function ChooseStep({ state, tasks, update, sources, sourceId, onSelectSource }: PlanTodayDialogProps & { state: RoutineState; tasks: SearchableTask[]; update: Update }) {
  const [newTitle, setNewTitle] = useState("");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const chosen = new Set(state.items.flatMap((item) => item.key ? [item.key] : []));
  // Inside the routine a card adds to the chosen list; opening its source would leave the dialog.
  const routineSources = sources.map((source) => ({ ...source, groups: source.groups.map((group) => ({ ...group, candidates: group.candidates.map((candidate) => ({ ...candidate, row: { ...candidate.row, onActivate: undefined } })) })) }));
  const add = async (candidate: PlanCandidate) => {
    setBusy(candidate.id);
    setError(null);
    try {
      const resolved = await candidate.resolve();
      if (resolved) update((current) => addItem(current, itemFor(resolved.addition, resolved.title, resolved.block)));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  };
  const needle = query.trim().toLowerCase();
  const results = needle ? tasks.filter((task) => task.title.toLowerCase().includes(needle)).slice(0, PLAN_SEARCH_RESULTS) : [];
  const focusRow = (id: string) => requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>(`[data-item="${CSS.escape(id)}"]`)?.focus());
  const move = (id: string, step: number) => { update((current) => moveItem(current, id, step)); focusRow(id); };
  const rowKeys = (event: ReactKeyboardEvent<HTMLLIElement>, id: string) => {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    event.stopPropagation();
    move(id, event.key === "ArrowUp" ? -1 : 1);
  };
  const addNew = () => { if (!newTitle.trim()) return; update((current) => addNewTask(current, newTitle)); setNewTitle(""); };
  const shown = state.items;
  return <div className="plan-choose">
    <div className="plan-choose-sources">
      <PlanSourceSwitcher sources={routineSources} selected={sourceId} onSelect={onSelectSource} planned={chosen}
        drag={{ startDrag: () => {}, clickSuppressed: () => false }} onAdd={(candidate) => { if (!busy) void add(candidate); }} />
      <label className="plan-choose-search">Search Tasks
        <input type="search" value={query} placeholder="Any open task" onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter") event.preventDefault(); }} />
      </label>
      {needle ? <ul className="plan-choose-results" aria-label="Matching Tasks">
        {results.map((task) => <li key={task.key}>
          <span>{task.title}{task.detail ? <small> · {task.detail}</small> : null}</span>
          {chosen.has(task.key) ? <span className="today-in-plan">In plan</span>
            : <button type="button" className="btn btn-sm" aria-label={`Add: ${task.title}`} onClick={() => update((current) => addItem(current, itemFor(task.addition, task.title, task.block)))}>Add</button>}
        </li>)}
        {results.length ? null : <li className="context-status">No open task matches.</li>}
      </ul> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </div>
    <section className="plan-choose-selected" aria-labelledby="plan-chosen">
      <h3 id="plan-chosen">Chosen</h3>
      {shown.length > PLAN_ITEMS_NOTE ? <p className="context-status">Most people finish three to five planned items.</p> : null}
      <ol ref={listRef} className="plan-chosen-list">
        {shown.map((item, index) => <li key={item.id} data-item={item.id} tabIndex={-1} className={item.missing ? "plan-chosen-missing" : undefined} onKeyDown={(event) => rowKeys(event, item.id)}>
          <span className="plan-chosen-text">
            <strong>{item.title}</strong>
            <small>{itemTag(item)}</small>
          </span>
          <span className="plan-chosen-actions">
            <button type="button" className="btn-icon btn-icon-sm" aria-label={`Move Up: ${item.title}`} title={`Move up (${formatShortcut("Alt+ArrowUp")})`} disabled={index === 0} onClick={() => move(item.id, -1)}><ArrowUp size={ICON_SIZE.sm} /></button>
            <button type="button" className="btn-icon btn-icon-sm" aria-label={`Move Down: ${item.title}`} title={`Move down (${formatShortcut("Alt+ArrowDown")})`} disabled={index === shown.length - 1} onClick={() => move(item.id, 1)}><ArrowDown size={ICON_SIZE.sm} /></button>
            <button type="button" className="btn-icon btn-icon-sm" aria-label={`Remove: ${item.title}`} title="Remove from the plan" onClick={() => update((current) => removeItem(current, item.id))}><CircleMinus size={ICON_SIZE.sm} /></button>
          </span>
        </li>)}
      </ol>
      {shown.length ? null : <p className="context-status">Nothing chosen yet. Add cards from the sources, search, or type a new task.</p>}
      <div className="plan-choose-new">
        <label>New Task
          <input value={newTitle} onChange={(event) => setNewTitle(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); addNew(); } }} />
        </label>
        <button type="button" className="btn btn-sm" disabled={!newTitle.trim()} onClick={addNew}>Add New Task</button>
      </div>
    </section>
  </div>;
}

/** A chosen item from what a card or search result adds. */
function itemFor(addition: PlanAddition, title: string, block: BlockTarget): Omit<RoutineItem, "id" | "existing"> {
  return { key: addition.kind === "link" ? keyOf(addition) : null, title, addition, carried: null, block };
}
const keyOf = (addition: Extract<PlanAddition, { kind: "link" }>) => planItemKey(addition.reference);

function itemTag(item: RoutineItem): string {
  if (item.missing) return "No longer on this device; it won't be saved";
  if (item.existing) return "In the plan";
  if (item.carried) return `Carried from ${shortDate(item.carried)}`;
  return item.key ? "New" : "New task";
}

function MeetingsStep({ events, preparation, onNotice, onOpenNote }: PlanTodayDialogProps) {
  const [preparing, setPreparing] = useState<ScheduleEvent | null>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  const meetings = (events ?? []).filter((event) => isPreparable(event) && !isDeclined(event) && !event.allDay);
  useEffect(() => { if (preparing) backRef.current?.focus(); }, [preparing]);
  if (!events) return <p className="context-status">Connect a calendar in Settings to prepare for meetings here.</p>;
  // Preparation shows inside the routine, so the routine's choices never leave the dialog.
  if (preparing && preparation) return <div className="plan-meetings-prepare">
    <button ref={backRef} type="button" className="btn btn-sm" onClick={() => setPreparing(null)}>Back to Meetings</button>
    {preparation.render(preparing, () => setPreparing(null))}
  </div>;
  if (!meetings.length) return <p className="context-status">No meetings to prepare for.</p>;
  return <MeetingReadinessList events={meetings} ownAddresses={preparation?.ownAddresses ?? []} onPrepare={(event) => setPreparing(event)}
    onOpenNote={onOpenNote} onNotice={onNotice} />;
}

function FitStep({ date, state, update, events, scheduleLoading, scheduleError, onRetrySchedule, workingHours, booking, onNotice }: PlanTodayDialogProps & { state: RoutineState; update: Update }) {
  const gridRef = useRef<HTMLDivElement>(null);
  const [now] = useState(() => new Date());
  const planDay = useMemo(() => day(date), [date]);
  const midnight = useMemo(() => new Date(planDay.getFullYear(), planDay.getMonth(), planDay.getDate()), [planDay]);
  const days = useMemo(() => [midnight], [midnight]);
  const known = events && !scheduleLoading && !scheduleError ? events : null;
  const capacity = planCapacity(known, workingHours, now, date);
  const bookable = state.items.filter((item) => item.block && !item.missing);
  const record = async (item: RoutineItem, start: Date) => {
    const booked = await bookBlock(item.block!, start, booking);
    if (!booked) return;
    update((current) => ({ ...current, bookings: [...current.bookings, { title: item.block!.title, start: booked.event.start, eventId: booked.event.id }] }));
    onNotice({
      message: `Booked ${booked.when} for “${item.block!.title}”`,
      undo: () => void booked.undo().then(
        () => { update((current) => ({ ...current, bookings: current.bookings.filter((entry) => entry.eventId !== booked.event.id) })); onNotice({ message: "Booked time removed" }); },
        (reason: unknown) => onNotice({ message: errorMessage(reason) }),
      ),
    });
  };
  const bookNext = (item: RoutineItem) => {
    if (!events) { onNotice({ message: "Connect a calendar in Settings to book time" }); return; }
    const start = nextFreeSlot(known, workingHours, now, TASK_BLOCK_MINUTES, date);
    if (start === "unknown") { onNotice({ message: "The calendar or working hours couldn't be read, so free time is unknown" }); return; }
    if (!start) { onNotice({ message: "No free half hour is left in that day's working hours" }); return; }
    void record(item, start);
  };
  const drag = useTaskBlockDrag<{ id: string; title: string; item: RoutineItem }>({
    gridRef, days, blockMinutes: TASK_BLOCK_MINUTES,
    onDrop: (source, dropDay, minutes) => void record(source.item, new Date(dropDay.getFullYear(), dropDay.getMonth(), dropDay.getDate(), 0, minutes)),
  });
  const bookedTitles = new Set(state.bookings.map((entry) => entry.title));
  const facts = capacity
    ? [`${minutesLabel(capacity.freeMinutes)} free in working hours`, `${bookable.filter((item) => bookedTitles.has(item.block!.title)).length} booked`, `${bookable.filter((item) => !bookedTitles.has(item.block!.title)).length} not booked`].join(" · ")
    : null;
  return <div className="plan-fit">
    <div className="plan-fit-cards">
      <p className="context-status">{facts ?? (events ? "Free time is unknown until the calendar and working hours can be read." : "The calendar isn't connected, so free time is unknown. You can still save the plan.")}</p>
      <ul className="plan-fit-list">
        {bookable.map((item) => <li key={item.id} className="plan-fit-card" onPointerDown={events ? (event) => drag.startDrag({ id: item.id, title: item.block!.title, item }, event) : undefined}>
          <span>{item.title}{bookedTitles.has(item.block!.title) ? <small> · Booked</small> : null}</span>
          <button type="button" className="btn-icon btn-icon-sm" aria-label={`Book: ${item.title}`} title="Book the next free half hour" onClick={() => bookNext(item)}><CalendarClock size={ICON_SIZE.sm} /></button>
        </li>)}
      </ul>
      {bookable.length ? null : <p className="context-status">Choose items first; then book time for them here.</p>}
    </div>
    {events ? <div className="plan-fit-day">
      <TodayDayView events={events} now={new Date()} day={midnight} label="Plan Day" loading={scheduleLoading} error={scheduleError} onRetry={onRetrySchedule}
        selectedEvent={null} onChooseEvent={() => {}} eventDecoration={() => null} gridRef={gridRef} preview={drag.preview} dragging={drag.draggingId !== null} workingHours={workingHours} />
    </div> : null}
  </div>;
}

function SaveStep({ state, dayName, onNotice }: { state: RoutineState; dayName: string; onNotice(notice: Notice): void }) {
  const lines = previewLines(state);
  const decisions = Object.values(state.decisions).filter((decision) => decision.kind !== "carry");
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(planText(state));
      onNotice({ message: "Plan copied" });
    } catch (reason) {
      onNotice({ message: errorMessage(reason) });
    }
  };
  return <div className="plan-save">
    <section aria-labelledby="plan-save-items">
      <h3 id="plan-save-items">{dayName}'s Plan</h3>
      {state.items.length ? <ol className="plan-save-list">{state.items.filter((item) => !item.missing).map((item) => <li key={item.id}>{item.title} <small>{itemTag(item)}</small></li>)}</ol>
        : <p className="context-status">No items.</p>}
      {state.removed.length ? <p className="context-status">Taken off the plan: {state.removed.map((item) => item.title).join(", ")}</p> : null}
      {decisions.length ? <>
        <h4>Not today</h4>
        <ul className="plan-save-list">{decisions.map((decision) => <li key={decision.key}>{decision.label} <small>{decision.kind === "defer" ? `Deferred to ${shortDate(decision.date)}` : "Dropped"}</small></li>)}</ul>
      </> : null}
    </section>
    <section aria-labelledby="plan-save-lines">
      <h4 id="plan-save-lines">Lines Added to the Note</h4>
      {lines.plan.length || lines.notToday.length
        ? <pre className="plan-save-preview">{[...lines.plan, ...(lines.notToday.length ? ["", "### Not today", "", ...lines.notToday] : [])].join("\n")}</pre>
        : <p className="context-status">{isDirty(state) ? "Only the order changes, or items are taken off." : "Nothing changes in the note."}</p>}
    </section>
    {state.bookings.length ? <section aria-labelledby="plan-save-bookings">
      <h4 id="plan-save-bookings">Already Booked</h4>
      <ul className="plan-save-list">{state.bookings.map((entry) => <li key={entry.eventId}>{formatClockTime(new Date(entry.start))} {entry.title}</li>)}</ul>
      <p className="context-status">These are on the calendar already; cancelling the plan doesn't remove them.</p>
    </section> : null}
    <div className="plan-save-actions"><button type="button" className="btn btn-sm" onClick={() => void copy()}><Copy size={ICON_SIZE.sm} />Copy Plan</button></div>
  </div>;
}
