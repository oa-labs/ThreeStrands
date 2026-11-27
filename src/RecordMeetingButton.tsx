import { useEffect, useState } from "react";
import { Mic, Square } from "lucide-react";
import type { ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { meetingEventReference } from "./meetingNoteEntry";
import { recordOffer } from "./recordingPolicy";
import { useRecording } from "./recordingContext";

/** How often a button reconsiders whether its meeting is close enough to record. */
const CLOCK_MS = 30_000;

function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), CLOCK_MS);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

/**
 * Record for a meeting under way or starting within 10 minutes, beside Add
 * Note; Stop Recording while that occurrence is the one being recorded. It
 * passes the same event reference Add Note links. Nothing renders when
 * recording is off, unsupported, or busy with another recording.
 */
export function RecordMeetingButton({ event, labelSuffix }: { event: ScheduleEvent; labelSuffix?: string }) {
  const recording = useRecording();
  const now = useNow();
  const reference = meetingEventReference(event);
  const offer = recording.enabled ? recordOffer(recording.status, event, now) : null;
  if (!offer || !reference) return null;
  const active = offer === "stop";
  const label = active ? "Stop Recording" : "Record";
  return (
    <button
      type="button"
      className="btn-icon btn-icon-sm"
      aria-label={labelSuffix ? `${label}: ${labelSuffix}` : label}
      title={label}
      onClick={() => void (active ? recording.stop() : recording.start({ eventRef: reference, title: event.title, ...(event.attendees?.length ? { attendees: event.attendees } : {}) }))}
    >
      {active ? <Square size={ICON_SIZE.sm} aria-hidden="true" /> : <Mic size={ICON_SIZE.sm} aria-hidden="true" />}
    </button>
  );
}
