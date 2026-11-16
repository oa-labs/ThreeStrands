import { CalendarCheck, CircleSlash, ListPlus, Undo2 } from "lucide-react";
import { useMemo, useState } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatEventTime } from "./calendarTime";
import type { ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { defaultMeetingEntry, noPrepKey, readinessByKey, readinessSummary, readNoPrepNeeded, setNoPrepNeeded } from "./meetingPrep";
import { meetingDate, meetingEventReference } from "./meetingNoteEntry";
import { reserveMeetingPrep } from "./noteCapture";
import { dailyNoteName } from "./noteDates";
import { noteReferenceHref, noteReferenceKey } from "./noteEntities";
import type { Notice } from "./useNotice";
import { useMeetingReadiness } from "./useMeetingPreparation";

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
};

/**
 * A day's meetings with what is ready for each: notes, earlier meetings,
 * commitments, and recent mail, as facts rather than a score. Each meeting
 * offers Prepare, Reserve Prep Time (a prep task under its line in its day's
 * note), and No Prep Needed (this device only, undoable).
 */
export function MeetingReadinessList({ events, ownAddresses, refreshKey, onPrepare, onOpenNote, onNotice }: Props) {
  const references = useMemo(() => events.flatMap((event) => { const reference = meetingEventReference(event); return reference ? [reference] : []; }), [events]);
  const readiness = useMeetingReadiness(references, refreshKey);
  const byKey = readinessByKey(readiness.rows);
  const [skipped, setSkipped] = useState(readNoPrepNeeded);
  const [busy, setBusy] = useState<string | null>(null);

  const reserve = async (event: ScheduleEvent) => {
    const key = noPrepKey(event);
    if (!key || busy) return;
    setBusy(key);
    try {
      const date = meetingDate(event);
      const reserved = await reserveMeetingPrep(date, await defaultMeetingEntry(event, ownAddresses));
      const action = { label: "Open Note", run: () => onOpenNote(reserved.note.id, noteReferenceHref(meetingEventReference(event)!)) };
      if (!reserved.added) onNotice({ message: "This meeting already has a prep task", action });
      else onNotice({
        message: `Prep task added to ${dailyNoteName(date)}`,
        action,
        undo: () => void reserved.undo().catch((reason) => onNotice({ message: errorMessage(reason), action })),
      });
    } catch (reason) {
      onNotice({ message: errorMessage(reason) });
    } finally {
      setBusy(null);
    }
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
    return [<ContextRow
      key={key}
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
