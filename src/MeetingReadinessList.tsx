import { CalendarCheck, CircleSlash, ClipboardCheck, ListPlus, Undo2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatEventTime } from "./calendarTime";
import type { ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { noPrepKey, preparationKey, readinessByKey, readinessSummary, readNoPrepNeeded, setNoPrepNeeded } from "./meetingPrep";
import { reservePrepTime } from "./meetingPrepActions";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceKey } from "./noteEntities";
import type { Notice } from "./useNotice";
import { useMeetingPreparation, useMeetingReadiness } from "./useMeetingPreparation";
import { Modal } from "./AppChrome";
import { errorMessage } from "./errors";
import { MeetingFollowThroughDialog } from "./MeetingFollowThroughDialog";
import { localDay, meetingsToFollowThrough, noFollowThroughKey, readNoFollowThrough, setNoFollowThrough } from "./meetingFollowThrough";

type Props = {
  /** The meetings to list, such as today's still to come. */
  events: readonly ScheduleEvent[];
  /** The user's own addresses, left out of a meeting line's attendees. */
  ownAddresses: readonly string[];
  refreshKey?: unknown;
  onPrepare(event: ScheduleEvent): void;
  /** Opens a day's note, focused on the meeting's link. */
  onOpenNote(noteId: string, href?: string): void;
  onNotice(notice: Notice): void;
  /** The meeting being prepared (`preparationKey`), shown selected. */
  selectedKey?: string | null;
};

/**
 * A day's meetings with what is ready for each: notes, earlier meetings,
 * commitments, and recent mail, as facts rather than a score. Each meeting
 * offers Prepare, Reserve Prep Time (a prep task under its line in its day's
 * note), and No Prep Needed (this device only, undoable).
 */
export function MeetingReadinessList({ events, ownAddresses, refreshKey, onPrepare, onOpenNote, onNotice, selectedKey = null }: Props) {
  const references = useMemo(() => events.flatMap((event) => { const reference = meetingEventReference(event); return reference ? [reference] : []; }), [events]);
  const readiness = useMeetingReadiness(references, refreshKey);
  const byKey = readinessByKey(readiness.rows);
  const [skipped, setSkipped] = useState(readNoPrepNeeded);
  const [busy, setBusy] = useState<string | null>(null);

  const reserve = async (event: ScheduleEvent) => {
    const key = noPrepKey(event);
    if (!key || busy) return;
    setBusy(key);
    try { await reservePrepTime(event, ownAddresses, onNotice, onOpenNote); } finally { setBusy(null); }
  };
  const toggleSkip = (event: ScheduleEvent, chosen: boolean) => {
    const key = noPrepKey(event);
    if (!key) return;
    setSkipped(setNoPrepNeeded(key, chosen));
    if (chosen) onNotice({ message: `No prep needed for ${event.title}`, undo: () => setSkipped(setNoPrepNeeded(key, false)) });
  };

  const rows = events.flatMap((event) => {
    const reference = meetingEventReference(event);
    const key = noPrepKey(event);
    if (!reference || !key) return [];
    const skip = skipped.includes(key);
    const row = byKey.get(noteReferenceKey(reference));
    const prepKey = preparationKey(event);
    return [<ContextRow
      key={key}
      rowKey={prepKey ? `meeting:${prepKey}` : undefined}
      pressed={selectedKey !== null ? prepKey === selectedKey : undefined}
      className={skip ? "meeting-readiness-skipped" : undefined}
      icon={<CalendarCheck size={ICON_SIZE.sm} />}
      title={event.title}
      date={formatEventTime(event)}
      detail={skip ? "No prep needed" : readiness.loading && !row ? "Checking…" : readinessSummary(row)}
      onActivate={() => onPrepare(event)}
      actions={skip
        ? [<button key="undo" type="button" className="btn-icon btn-icon-sm" aria-label={`Prep needed for ${event.title}`} title="Prep Needed" onClick={() => toggleSkip(event, false)}><Undo2 size={ICON_SIZE.sm} aria-hidden="true" /></button>]
        : [
          <button key="reserve" type="button" className="btn-icon btn-icon-sm" aria-label={`Reserve prep time for ${event.title}`} title="Reserve Prep Time" disabled={busy === key} onClick={() => void reserve(event)}><ListPlus size={ICON_SIZE.sm} aria-hidden="true" /></button>,
          <button key="skip" type="button" className="btn-icon btn-icon-sm" aria-label={`No prep needed for ${event.title}`} title="No Prep Needed" onClick={() => toggleSkip(event, true)}><CircleSlash size={ICON_SIZE.sm} aria-hidden="true" /></button>,
        ]}
    />];
  });
  const outstanding = events.filter((event) => { const key = noPrepKey(event); return key && !skipped.includes(key); }).length;
  return <ContextSection
    id="meeting-readiness"
    title="Meeting prep"
    count={outstanding}
    rows={rows}
    footer={<>
      {readiness.error ? <p className="context-status">Meeting prep couldn't be checked. <button type="button" className="btn-link" onClick={readiness.reload}>Try Again</button></p> : null}
      {rows.length === 0 ? <p className="context-status">No meetings left to prepare for.</p> : null}
    </>}
  />;
}

/** Review Follow-through for one meeting outside its preparation view:
 * loads its context, then shows the review form over the current surface. */
export function MeetingFollowThroughLauncher({ event, onClose, onNotice, onAddNote, today }: {
  event: ScheduleEvent;
  onClose(): void;
  onNotice(notice: Notice): void;
  onAddNote?(): void;
  today?: string;
}) {
  const prep = useMeetingPreparation(meetingEventReference(event));
  if (prep.error) return <Modal title="Review Follow-through" onClose={onClose}><p className="modal-form-hint" role="alert">{prep.error}</p></Modal>;
  if (!prep.context) return <Modal title="Review Follow-through" onClose={onClose}><p className="modal-form-hint" role="status">Loading this meeting's notes…</p></Modal>;
  return <MeetingFollowThroughDialog event={event} context={prep.context} today={today ?? localDay(new Date())} onClose={onClose} onAddNote={onAddNote}
    onSaved={(saved) => { onClose(); onNotice({ message: `Follow-through saved for ${event.title}`, undo: () => void saved.undo().catch((reason) => onNotice({ message: errorMessage(reason) })) }); }} />;
}

/**
 * Shut Down's meetings: the day's meetings that have ended, each with its
 * follow-through state from saved notes, the same on every synced device.
 * An unreviewed meeting offers Review Follow-through and No Follow-through
 * (this device only, undoable). Choosing a meeting prepares it.
 */
export function MeetingFollowThroughList({ events, now, refreshKey, onPrepare, onNotice, onAddNote, onOutstanding }: {
  events: readonly ScheduleEvent[];
  now: Date;
  refreshKey?: unknown;
  onPrepare(event: ScheduleEvent): void;
  onNotice(notice: Notice): void;
  onAddNote?(event: ScheduleEvent): void;
  /** Reports how many ended meetings are neither reviewed nor set aside, or null while that isn't known (loading, or couldn't be checked). */
  onOutstanding?(count: number | null): void;
}) {
  const ended = useMemo(() => meetingsToFollowThrough(events, now), [events, now]);
  const references = useMemo(() => ended.flatMap((event) => { const reference = meetingEventReference(event); return reference ? [reference] : []; }), [ended]);
  const readiness = useMeetingReadiness(references, refreshKey);
  const byKey = readinessByKey(readiness.rows);
  const [setAside, setSetAside] = useState(readNoFollowThrough);
  const [reviewing, setReviewing] = useState<ScheduleEvent | null>(null);
  const toggle = (event: ScheduleEvent, chosen: boolean) => {
    const key = noFollowThroughKey(event);
    if (!key) return;
    setSetAside(setNoFollowThrough(key, chosen));
    if (chosen) onNotice({ message: `No follow-through for ${event.title}`, undo: () => setSetAside(setNoFollowThrough(key, false)) });
  };
  const outstanding = ended.filter((event) => {
    const key = noFollowThroughKey(event);
    const reference = meetingEventReference(event);
    return key && reference && !setAside.includes(key) && byKey.get(noteReferenceKey(reference))?.followThrough !== "reviewed";
  });
  const known = readiness.loading || readiness.error ? null : outstanding.length;
  useEffect(() => { onOutstanding?.(known); }, [known, onOutstanding]);
  const rows = ended.flatMap((event) => {
    const reference = meetingEventReference(event);
    const key = noFollowThroughKey(event);
    if (!reference || !key) return [];
    const row = byKey.get(noteReferenceKey(reference));
    const aside = setAside.includes(key);
    const reviewed = row?.followThrough === "reviewed";
    return [<ContextRow
      key={key}
      className={aside ? "meeting-readiness-skipped" : undefined}
      icon={<ClipboardCheck size={ICON_SIZE.sm} />}
      title={event.title}
      date={formatEventTime(event)}
      detail={reviewed ? "Reviewed" : aside ? "No follow-through" : readiness.loading && !row ? "Checking…" : row?.followThrough === "unknown" ? "Couldn't check this meeting" : "Not reviewed yet"}
      onActivate={() => onPrepare(event)}
      actions={reviewed ? [] : aside
        ? [<button key="undo" type="button" className="btn-icon btn-icon-sm" aria-label={`Follow-through needed for ${event.title}`} title="Follow-through Needed" onClick={() => toggle(event, false)}><Undo2 size={ICON_SIZE.sm} aria-hidden="true" /></button>]
        : [
          <button key="review" type="button" className="btn-icon btn-icon-sm" aria-label={`Review follow-through for ${event.title}`} title="Review Follow-through" onClick={() => setReviewing(event)}><ClipboardCheck size={ICON_SIZE.sm} aria-hidden="true" /></button>,
          <button key="skip" type="button" className="btn-icon btn-icon-sm" aria-label={`No follow-through for ${event.title}`} title="No Follow-through" onClick={() => toggle(event, true)}><CircleSlash size={ICON_SIZE.sm} aria-hidden="true" /></button>,
        ]}
    />];
  });
  return <>
    <ContextSection
      id="meeting-follow-through"
      title="Meeting follow-through"
      count={outstanding.length}
      rows={rows}
      footer={<>
        {readiness.error ? <p className="context-status">Follow-through couldn't be checked. <button type="button" className="btn-link" onClick={readiness.reload}>Try Again</button></p> : null}
        {rows.length === 0 ? <p className="context-status">No meetings have ended yet.</p> : null}
      </>}
    />
    {reviewing ? <MeetingFollowThroughLauncher event={reviewing} onClose={() => setReviewing(null)} onNotice={onNotice}
      onAddNote={onAddNote ? () => onAddNote(reviewing) : undefined} /> : null}
  </>;
}
