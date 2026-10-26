import { useId, useState } from "react";
import { responseLabel } from "./calendarResponse";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";

const RESPONSES = [["accepted", "Yes"], ["declined", "No"], ["tentative", "Maybe"]] as const;
const INVITATION_RESPONSES = [["accepted", "Going"], ["tentative", "Maybe"], ["declined", "Not going"]] as const;

/** The user's RSVP to a calendar event, with Yes / No / Maybe when they can answer it. */
export function CalendarResponseControls({ event, onUpdated, invitation = false }: { event: ScheduleEvent; onUpdated(event: ScheduleEvent): void; invitation?: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const questionId = useId();
  const label = responseLabel(event);
  if (!label) return null;
  const unanswered = event.responseStatus === "needsAction" || event.responseStatus == null;
  const respond = async (status: (typeof RESPONSES)[number][0]) => {
    setPending(true);
    setError(null);
    try {
      const updated = await mailClient.updateCalendarResponse(event, status);
      onUpdated(updated);
      revalidateScheduleCache(updated);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setPending(false);
    }
  };
  if (!invitation && event.canRespond) {
    // The pressed segment already shows the answer, so the question labels the control instead of a status line.
    return (
      <div className="calendar-event-response">
        <div className="calendar-response-row">
          <span id={questionId}>Going?</span>
          <div className="segmented calendar-response-actions" role="group" aria-labelledby={questionId}>
            {RESPONSES.map(([status, title]) => (
              <button key={status} type="button" className="segment" aria-pressed={event.responseStatus === status} disabled={pending} onClick={() => void respond(status)}>{title}</button>
            ))}
          </div>
        </div>
        {error ? <p role="alert">{error}</p> : null}
      </div>
    );
  }
  return (
    <div className="calendar-event-response">
      {invitation && unanswered ? <span className="context-status">Not responded.</span> : <span>Your response: <strong>{label}</strong></span>}
      {event.canRespond ? <div className="calendar-invitation-response-actions" role="group" aria-label="Going?">
        {INVITATION_RESPONSES.map(([status, title]) => (
          <button key={status} type="button" className="btn" aria-pressed={event.responseStatus === status} disabled={pending || event.responseStatus === status} onClick={() => void respond(status)}>{title}</button>
        ))}
      </div> : null}
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
