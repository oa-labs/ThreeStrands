import { CalendarDays, NotebookPen, Video } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { meetingEventReference, primaryMeetingNote } from "./meetingNoteEntry";
import { useNoteBacklinks } from "./NoteEntityContext";
import { meetingPlace } from "./ThreadAssist";

/**
 * The next steps for a meeting being prepared, as labelled buttons: Add Note
 * (Open Note once the meeting has one), Join for an online meeting, and Open
 * in Calendar where preparation isn't in Calendar.
 */
export function MeetingPrepActions({ event, onAddNote, onOpenInCalendar, showJoin = true }: {
  event: ScheduleEvent;
  onAddNote?(): void;
  onOpenInCalendar?(): void;
  showJoin?: boolean;
}) {
  const reference = meetingEventReference(event);
  const { rows } = useNoteBacklinks(reference ? [reference] : []);
  const { joinUrl } = meetingPlace(event.location ?? null, event.conferenceUrl);
  const note = primaryMeetingNote(rows) ? "Open Note" : "Add Note";
  const hasActions = Boolean((onAddNote && reference) || (showJoin && joinUrl) || onOpenInCalendar);
  if (!hasActions) return null;
  return <>
    {onAddNote && reference ? <button type="button" className="btn btn-sm" onClick={onAddNote}><NotebookPen size={ICON_SIZE.sm} aria-hidden="true" />{note}</button> : null}
    {showJoin && joinUrl ? <button type="button" className="btn btn-sm" onClick={() => void openUrl(joinUrl)}><Video size={ICON_SIZE.sm} aria-hidden="true" />Join</button> : null}
    {onOpenInCalendar ? <button type="button" className="btn btn-sm" onClick={onOpenInCalendar}><CalendarDays size={ICON_SIZE.sm} aria-hidden="true" />Open in Calendar</button> : null}
  </>;
}
