import { useMemo, useState, type FormEvent } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, CreateCalendarEventRequest } from "./domain";
import { errorMessage } from "./errors";

function localDateTimeValue(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function CreateCalendarEventDialog({
  start,
  end,
  accounts,
  calendars,
  initialTitle = "",
  initialInvitees = [],
  initialDescription = "",
  onClose,
  onCreated,
}: {
  start: Date;
  end: Date;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  /** Prefills from a meeting suggestion; the user still reviews and creates. */
  initialTitle?: string;
  initialInvitees?: string[];
  initialDescription?: string;
  onClose(): void;
  onCreated(): void;
}) {
  const writable = useMemo(() => calendars.filter((calendar) =>
    calendar.writable && accounts.some((account) => account.email === calendar.accountId && account.status === "connected"),
  ), [accounts, calendars]);
  const defaultCalendar = writable.find((calendar) => calendar.primary) ?? writable[0];
  const [calendarKey, setCalendarKey] = useState(() => defaultCalendar ? `${defaultCalendar.accountId}\n${defaultCalendar.id}` : "");
  const selectedCalendarKey = writable.some((calendar) => `${calendar.accountId}\n${calendar.id}` === calendarKey)
    ? calendarKey : defaultCalendar ? `${defaultCalendar.accountId}\n${defaultCalendar.id}` : "";
  const [title, setTitle] = useState(initialTitle);
  const [startValue, setStartValue] = useState(() => localDateTimeValue(start));
  const [endValue, setEndValue] = useState(() => localDateTimeValue(end));
  const [invitees, setInvitees] = useState(() => initialInvitees.join(", "));
  const [description, setDescription] = useState(initialDescription);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    const calendar = writable.find((option) => `${option.accountId}\n${option.id}` === selectedCalendarKey);
    const startsAt = new Date(startValue);
    const endsAt = new Date(endValue);
    const attendees = invitees.split(/[\s,;]+/).map((email) => email.trim()).filter(Boolean);
    if (!calendar) { setError("Choose a calendar where you can create events."); return; }
    if (!title.trim()) { setError("Enter an event title."); return; }
    if (!Number.isFinite(startsAt.getTime()) || !Number.isFinite(endsAt.getTime()) || endsAt <= startsAt) {
      setError("The end time must be after the start time."); return;
    }
    if (attendees.some((email) => !/^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/.test(email))) {
      setError("Enter valid invitee email addresses, separated by commas."); return;
    }
    const request: CreateCalendarEventRequest = {
      accountId: calendar.accountId,
      calendarId: calendar.id,
      title: title.trim(),
      start: startsAt.toISOString(),
      end: endsAt.toISOString(),
      description,
      attendees: [...new Set(attendees.map((email) => email.toLowerCase()))],
    };
    setSaving(true);
    try {
      await mailClient.createCalendarEvent(request);
      onCreated();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="New event" className="calendar-event-modal" onClose={onClose} dismissible={!saving}>
      <form className="modal-form" onSubmit={(event) => { void submit(event); }}>
        <label>Title<input value={title} onChange={(event) => setTitle(event.target.value)} maxLength={1024} required placeholder="Add title" /></label>
        <div className="calendar-event-time-fields">
          <label>Starts<input type="datetime-local" value={startValue} onChange={(event) => setStartValue(event.target.value)} required /></label>
          <label>Ends<input type="datetime-local" value={endValue} onChange={(event) => setEndValue(event.target.value)} required /></label>
        </div>
        <label>Calendar
          <select value={selectedCalendarKey} onChange={(event) => setCalendarKey(event.target.value)} required>
            {writable.length === 0 ? <option value="">No writable calendars</option> : null}
            {writable.map((calendar) => (
              <option key={`${calendar.accountId}:${calendar.id}`} value={`${calendar.accountId}\n${calendar.id}`}>
                {calendar.name} · {calendar.accountId}
              </option>
            ))}
          </select>
        </label>
        {writable.length === 0 ? <p className="form-error">Connect or reconnect a Google Calendar account with event access to create meetings.</p> : null}
        <label>Invite people<input type="text" value={invitees} onChange={(event) => setInvitees(event.target.value)} placeholder="name@example.com, colleague@example.com" /></label>
        <label>Description<textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={5} maxLength={32768} placeholder="Add meeting details" /></label>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions">
          <button type="button" onClick={onClose} disabled={saving}>Cancel</button>
          <button type="submit" disabled={saving || writable.length === 0}>{saving ? "Creating…" : "Create event"}</button>
        </div>
      </form>
    </Modal>
  );
}
