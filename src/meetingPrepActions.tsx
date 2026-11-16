import { CalendarDays, ListPlus, NotebookPen, Video } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { defaultMeetingEntry } from "./meetingPrep";
import { meetingDate, meetingEventReference, primaryMeetingNote } from "./meetingNoteEntry";
import { reserveMeetingPrep } from "./noteCapture";
import { dailyNoteName } from "./noteDates";
import { noteReferenceHref } from "./noteEntities";
import { useNoteBacklinks } from "./NoteEntityContext";
import { meetingPlace } from "./ThreadAssist";
import type { Notice } from "./useNotice";

/** Reserve Prep Time for `event`, saying what happened with Open Note and Undo. */
export async function reservePrepTime(event: ScheduleEvent, ownAddresses: readonly string[], onNotice: (notice: Notice) => void, onOpenNote: (noteId: string, href?: string) => void) {
  const reference = meetingEventReference(event);
  if (!reference) return;
  try {
    const date = meetingDate(event);
    const reserved = await reserveMeetingPrep(date, await defaultMeetingEntry(event, ownAddresses));
    const action = { label: "Open Note", run: () => onOpenNote(reserved.note.id, noteReferenceHref(reference)) };
    if (!reserved.added) onNotice({ message: "This meeting already has a prep task", action });
    else onNotice({
      message: `Prep task added to ${dailyNoteName(date)}`,
      action,
      undo: () => void reserved.undo().catch((reason) => onNotice({ message: errorMessage(reason), action })),
    });
  } catch (reason) {
    onNotice({ message: errorMessage(reason) });
  }
}

/**
 * The next steps for a meeting being prepared, as labelled buttons: Add Note
 * (Open Note once the meeting has one), Reserve Prep Time, Join for an
 * online meeting, and Open in Calendar where preparation isn't in Calendar.
 */
export function MeetingPrepActions({ event, onAddNote, onReservePrep, onOpenInCalendar, showJoin = true }: {
  event: ScheduleEvent;
  onAddNote?(): void;
  onReservePrep?(): void;
  onOpenInCalendar?(): void;
  showJoin?: boolean;
}) {
  const reference = meetingEventReference(event);
  const { rows } = useNoteBacklinks(reference ? [reference] : []);
  const { joinUrl } = meetingPlace(event.location ?? null, event.conferenceUrl);
  const note = primaryMeetingNote(rows) ? "Open Note" : "Add Note";
  return <>
    {onAddNote && reference ? <button type="button" className="btn btn-sm" onClick={onAddNote}><NotebookPen size={ICON_SIZE.sm} aria-hidden="true" />{note}</button> : null}
    {onReservePrep && reference ? <button type="button" className="btn btn-sm" onClick={onReservePrep}><ListPlus size={ICON_SIZE.sm} aria-hidden="true" />Reserve Prep Time</button> : null}
    {showJoin && joinUrl ? <button type="button" className="btn btn-sm" onClick={() => void openUrl(joinUrl)}><Video size={ICON_SIZE.sm} aria-hidden="true" />Join</button> : null}
    {onOpenInCalendar ? <button type="button" className="btn btn-sm" onClick={onOpenInCalendar}><CalendarDays size={ICON_SIZE.sm} aria-hidden="true" />Open in Calendar</button> : null}
  </>;
}
