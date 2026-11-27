import type { MeetingRecording, RecordingStatus, RecordingStopReason, ScheduleEvent } from "./domain";
import { formatClockTime, isDeclined } from "./agendaEvents";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceKey, type NoteReference } from "./noteEntities";
import { isWorkingLocation } from "./WorkingLocation";

/** How long before a meeting starts that Record is offered for it. Matches the tray menu's "starts within 10 min". */
export const RECORD_LEAD_MS = 10 * 60_000;

export const RECORDING_REMINDER = "Let everyone know this meeting is being recorded.";

/**
 * Whether Record is offered for an event at `now`: a timed, undeclined meeting
 * a note can link, that is under way or starts within the lead window. An
 * ended meeting, an all-day event, and one further out are not.
 */
export function canRecordEvent(event: ScheduleEvent, now: Date): boolean {
  if (event.allDay || isDeclined(event) || isWorkingLocation(event) || !meetingEventReference(event)) return false;
  const start = new Date(event.start).getTime();
  const end = new Date(event.end).getTime();
  if (Number.isNaN(start) || Number.isNaN(end)) return false;
  const at = now.getTime();
  return at < end && at >= start - RECORD_LEAD_MS;
}

/**
 * What Record offers for an event: Stop Recording while this occurrence is the
 * one being recorded, Record when it is close enough and nothing else is being
 * recorded, otherwise nothing.
 */
export function recordOffer(status: RecordingStatus, event: ScheduleEvent, now: Date): "record" | "stop" | null {
  const reference = meetingEventReference(event);
  if (!reference) return null;
  if (isRecordingOf(status, reference)) return "stop";
  return status.state === "idle" && canRecordEvent(event, now) ? "record" : null;
}

/** A recording is of this occurrence when its event reference names it. */
export function isRecordingOf(status: RecordingStatus, reference: NoteReference | null): boolean {
  return status.state !== "idle" && Boolean(reference) && Boolean(status.eventRef) && noteReferenceKey(status.eventRef!) === noteReferenceKey(reference!);
}

/** The title an ad-hoc recording gets, such as "Meeting at 3:04 PM". */
export function adHocRecordingTitle(now: Date): string {
  return `Meeting at ${formatClockTime(now)}`;
}

/** `m:ss`, or `h:mm:ss` from an hour on. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = String(total % 60).padStart(2, "0");
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

const minutesLabel = (ms: number) => `${Math.max(1, Math.round(ms / 60_000))} min`;

/** The detail line under a Recording row. */
export function recordingDetail(recording: MeetingRecording): string {
  const minutes = minutesLabel(recording.durationMs);
  const parts: string[] = [];
  switch (recording.state) {
    case "recording": parts.push("Recording…"); break;
    case "interrupted": parts.push("Interrupted", `${minutes} saved`); break;
    case "failed": parts.push("Failed"); break;
    case "transcribing": parts.push(minutes, "Transcribing…"); break;
    case "transcribed": parts.push(minutes, "Transcribed"); break;
    case "recorded": parts.push(minutes, "Recorded"); break;
  }
  const system = recording.tracks.find((track) => track.track === "system");
  if (recording.state !== "recording" && recording.state !== "failed" && system?.state !== "active") parts.push("Microphone only");
  return parts.join(" · ");
}

/** The notice for a recording the app ended on its own, or null for a normal stop. */
export function stopReasonNotice(reason: RecordingStopReason | null): string | null {
  switch (reason) {
    case "lengthLimit": return "Recording stopped because it reached the length limit. The audio was saved.";
    case "lowDisk": return "Recording stopped because this Mac is low on disk space. The audio was saved.";
    case "allTracksFailed": return "Recording stopped because no audio could be captured.";
    default: return null;
  }
}
