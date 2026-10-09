import { useState } from "react";
import { responseLabel } from "./calendarResponse";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";

const RESPONSES = [["accepted", "Yes"], ["declined", "No"], ["tentative", "Maybe"]] as const;

/** The user's RSVP to a calendar event, with Yes / No / Maybe when they can answer it. */
export function CalendarResponseControls({ event, onUpdated }: { event: ScheduleEvent; onUpdated(event: ScheduleEvent): void }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const label = responseLabel(event);
  if (!label) return null;
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
  return (
    <div className="calendar-event-response">
      <span>Your response: <strong>{label}</strong></span>
      {event.canRespond ? <div className="segmented calendar-response-actions" role="group" aria-label="Going?">
        {RESPONSES.map(([status, title]) => (
          <button key={status} type="button" className="segment" aria-pressed={event.responseStatus === status} disabled={pending} onClick={() => void respond(status)}>{title}</button>
        ))}
      </div> : null}
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
