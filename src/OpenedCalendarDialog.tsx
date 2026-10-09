import { CalendarPlus } from "lucide-react";
import { useEffect, useState } from "react";
import { Modal } from "./AppChrome";
import { CalendarEvent, calendarEventRange } from "./CalendarAttachment";
import { CalendarResponseControls } from "./CalendarResponseControls";
import { responseLabel } from "./calendarResponse";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, OpenedCalendarFile, ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

type Props = {
  file: OpenedCalendarFile;
  /** Files still waiting after this one, so the close button can say so. */
  waiting: number;
  calendarConnected: boolean;
  onAddToCalendar(event: CalendarEventPreview): void;
  onClose(): void;
};

/**
 * A `.ics` file macOS opened with ThreeStrands, shown as the same invitation
 * card a message attachment gets. When a connected calendar already holds the
 * event, the user answers it there; otherwise they can add it to a calendar.
 */
export function OpenedCalendarDialog({ file, waiting, calendarConnected, onAddToCalendar, onClose }: Props) {
  return (
    <Modal title={file.name} className="calendar-invitation-modal" onClose={onClose}>
      <div className="modal-form">
        {file.preview ? (
          <>
            {file.preview.events.map((event, index) => (
              <section className="calendar-card" aria-label="Calendar invitation" key={`${event.uid ?? event.start ?? "event"}-${index}`}>
                <CalendarEvent event={event} />
                <footer className="calendar-card-actions">
                  <InvitationActions event={event} calendarConnected={calendarConnected} onAddToCalendar={onAddToCalendar} />
                </footer>
              </section>
            ))}
            {file.preview.truncated ? <p className="modal-form-context">Additional events in this file are not shown.</p> : null}
          </>
        ) : (
          <p className="form-error" role="alert">{file.error ?? "This calendar file could not be read."}</p>
        )}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose}>{waiting > 0 ? `Next Invitation (${waiting} more)` : "Done"}</button>
        </div>
      </div>
    </Modal>
  );
}

type Lookup =
  | { kind: "checking" }
  | { kind: "found"; event: ScheduleEvent }
  | { kind: "missing"; error: string | null };

function InvitationActions({ event, calendarConnected, onAddToCalendar }: {
  event: CalendarEventPreview;
  calendarConnected: boolean;
  onAddToCalendar(event: CalendarEventPreview): void;
}) {
  const uid = event.uid?.trim() ?? "";
  const [lookup, setLookup] = useState<Lookup>(() => calendarConnected && uid ? { kind: "checking" } : { kind: "missing", error: null });

  useEffect(() => {
    if (!calendarConnected || !uid) {
      setLookup({ kind: "missing", error: null });
      return;
    }
    let active = true;
    setLookup({ kind: "checking" });
    void mailClient.findCalendarInvitation(uid)
      .then((found) => { if (active) setLookup(found ? { kind: "found", event: found } : { kind: "missing", error: null }); })
      .catch((reason: unknown) => { if (active) setLookup({ kind: "missing", error: `Could not check your calendar: ${errorMessage(reason)}` }); });
    return () => { active = false; };
  }, [calendarConnected, uid]);

  if (!calendarConnected) return <span>Connect a Google Calendar account in Settings to answer or add this invitation.</span>;
  if (lookup.kind === "checking") return <span role="status">Checking your calendar…</span>;
  if (lookup.kind === "found") {
    return responseLabel(lookup.event)
      ? <CalendarResponseControls event={lookup.event} onUpdated={(updated) => setLookup({ kind: "found", event: updated })} />
      : <span>Already on your calendar · {lookup.event.accountId}</span>;
  }
  if (event.status?.toLocaleLowerCase() === "cancelled") return <span>This event was cancelled.</span>;
  const range = calendarEventRange(event);
  return (
    <>
      <span>
        {lookup.error ? <span className="form-error" role="alert">{lookup.error}</span>
          : event.timeZone ? `Its time zone (${event.timeZone}) isn’t recognized, so it can’t be added automatically.`
          : "Not on your calendar yet."}
      </span>
      <button type="button" className="btn btn-sm" disabled={!range} onClick={() => onAddToCalendar(event)}>
        <CalendarPlus size={ICON_SIZE.sm} /> Add to Calendar
      </button>
    </>
  );
}
