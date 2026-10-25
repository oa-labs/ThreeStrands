import { useMemo, useState, type FormEvent } from "react";
import { Modal } from "./AppChrome";
import { calendarDescriptionText } from "./calendarDescription";
import { addDays, dateInputValue } from "./calendarTime";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, CreateCalendarEventRequest, ScheduleEvent, UpdateCalendarEventRequest } from "./domain";
import { errorMessage } from "./errors";
import { EmailBadgeField } from "./ContactAddressField";
import { isEmailAddress } from "./emailAddress";

function localDateTimeValue(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Reads a `YYYY-MM-DD` value as a local calendar day, not UTC midnight. */
function localDate(value: string): Date {
  const [year, month, day] = value.slice(0, 10).split("-").map(Number);
  return new Date(year, month - 1, day);
}

/**
 * Text left in Invite People that is not an address blocks saving; the field
 * already says why, so focus it to fix.
 */
function blockedByPendingInvitee(form: HTMLFormElement, pending: string): boolean {
  if (!pending.trim()) return false;
  form.querySelector<HTMLInputElement>('input[aria-label="Invite People"]')?.focus();
  return true;
}

export function CreateCalendarEventDialog({
  start,
  end,
  accounts,
  calendars,
  initialTitle = "",
  initialInvitees = [],
  initialDescription = "",
  initialLocation = "",
  initialAllDay = false,
  sourceMessageId,
  onClose,
  onCreated,
}: {
  /** For an all-day event, `start` and `end` are the first and last local days. */
  start: Date;
  end: Date;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  /** Prefills from a meeting suggestion; the user still reviews and creates. */
  initialTitle?: string;
  initialInvitees?: string[];
  initialDescription?: string;
  initialLocation?: string;
  initialAllDay?: boolean;
  /** The email the event comes from, so it can be recognized on the calendar later. */
  sourceMessageId?: string;
  onClose(): void;
  onCreated(event: ScheduleEvent): void;
}) {
  const writable = useMemo(() => calendars.filter((calendar) =>
    calendar.writable && accounts.some((account) => account.email === calendar.accountId && account.status === "connected"),
  ), [accounts, calendars]);
  const visible = writable.filter((calendar) => calendar.selected);
  const defaultCalendar = visible.find((calendar) => calendar.primary) ?? visible[0]
    ?? writable.find((calendar) => calendar.primary) ?? writable[0];
  const [calendarKey, setCalendarKey] = useState(() => defaultCalendar ? `${defaultCalendar.accountId}\n${defaultCalendar.id}` : "");
  const selectedCalendarKey = writable.some((calendar) => `${calendar.accountId}\n${calendar.id}` === calendarKey)
    ? calendarKey : defaultCalendar ? `${defaultCalendar.accountId}\n${defaultCalendar.id}` : "";
  const [title, setTitle] = useState(initialTitle);
  const [allDay, setAllDay] = useState(initialAllDay);
  const [startValue, setStartValue] = useState(() => initialAllDay ? dateInputValue(start) : localDateTimeValue(start));
  const [endValue, setEndValue] = useState(() => initialAllDay ? dateInputValue(end) : localDateTimeValue(end));
  const [location, setLocation] = useState(initialLocation);
  const [invitees, setInvitees] = useState<string[]>(initialInvitees);
  const [pendingInvitee, setPendingInvitee] = useState("");
  const [description, setDescription] = useState(initialDescription);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function changeAllDay(next: boolean) {
    setAllDay(next);
    if (next) {
      setStartValue(startValue.slice(0, 10));
      setEndValue(endValue.slice(0, 10) < startValue.slice(0, 10) ? startValue.slice(0, 10) : endValue.slice(0, 10));
    } else {
      setStartValue(`${startValue.slice(0, 10)}T09:00`);
      setEndValue(`${startValue.slice(0, 10)}T10:00`);
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (blockedByPendingInvitee(event.currentTarget, pendingInvitee)) return;
    const calendar = writable.find((option) => `${option.accountId}\n${option.id}` === selectedCalendarKey);
    const attendees = invitees;
    if (!calendar) { setError("Choose a calendar where you can create events."); return; }
    if (!title.trim()) { setError("Enter an event title."); return; }
    let startAt: string;
    let endAt: string;
    if (allDay) {
      const startDay = localDate(startValue);
      const lastDay = localDate(endValue);
      if (!Number.isFinite(startDay.getTime()) || !Number.isFinite(lastDay.getTime()) || lastDay < startDay) {
        setError("The end date must be on or after the start date."); return;
      }
      startAt = dateInputValue(startDay);
      endAt = dateInputValue(addDays(lastDay, 1));
    } else {
      const startsAt = new Date(startValue);
      const endsAt = new Date(endValue);
      if (!Number.isFinite(startsAt.getTime()) || !Number.isFinite(endsAt.getTime()) || endsAt <= startsAt) {
        setError("The end time must be after the start time."); return;
      }
      startAt = startsAt.toISOString();
      endAt = endsAt.toISOString();
    }
    const invalid = attendees.filter((email) => !isEmailAddress(email));
    if (invalid.length) {
      setError(`Check these invitee email addresses: ${invalid.join(", ")}.`); return;
    }
    const request: CreateCalendarEventRequest = {
      accountId: calendar.accountId,
      calendarId: calendar.id,
      title: title.trim(),
      start: startAt,
      end: endAt,
      allDay,
      location: location.trim(),
      description,
      attendees: [...new Set(attendees.map((email) => email.toLowerCase()))],
      ...(sourceMessageId ? { sourceMessageId } : {}),
    };
    setSaving(true);
    try {
      // A new event must be visible in the schedule. Do this before creating
      // it, so a selection failure cannot leave an event that a retry duplicates.
      if (!calendar.selected) {
        await mailClient.setCalendarSelection(calendar.accountId, [
          ...calendars.filter((option) => option.accountId === calendar.accountId && option.selected).map((option) => option.id),
          calendar.id,
        ]);
      }
      onCreated(await mailClient.createCalendarEvent(request));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="New event" className="calendar-event-modal" onClose={onClose} dismissible={!saving}>
      <form className="modal-form" onSubmit={(event) => { void submit(event); }} onKeyDown={(keyEvent) => {
        if ((keyEvent.metaKey || keyEvent.ctrlKey) && keyEvent.key === "Enter") {
          keyEvent.preventDefault();
          keyEvent.currentTarget.requestSubmit();
        }
      }}>
        <label>Title<input value={title} onChange={(event) => setTitle(event.target.value)} maxLength={1024} required placeholder="Add title" /></label>
        <label className="calendar-event-all-day-field"><input type="checkbox" checked={allDay} onChange={(change) => changeAllDay(change.target.checked)} />All day</label>
        <div className="calendar-event-time-fields">
          <label>Starts<input type={allDay ? "date" : "datetime-local"} value={startValue} onChange={(event) => setStartValue(event.target.value)} required /></label>
          <label>Ends<input type={allDay ? "date" : "datetime-local"} value={endValue} onChange={(event) => setEndValue(event.target.value)} required /></label>
        </div>
        <label>Location<input value={location} onChange={(event) => setLocation(event.target.value)} maxLength={4096} placeholder="Add location" /></label>
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
        {writable.some((calendar) => `${calendar.accountId}\n${calendar.id}` === selectedCalendarKey && !calendar.selected)
          ? <p className="modal-form-context">This calendar will be shown in your schedule.</p> : null}
        {writable.length === 0 ? <p className="form-error">Connect or reconnect a Google Calendar account with event access to create meetings.</p> : null}
        <EmailBadgeField label="Invite People" values={invitees} disabled={saving} onChange={setInvitees} onPendingChange={setPendingInvitee} />
        <label>Description<textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={5} maxLength={32768} placeholder="Add meeting details" /></label>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={saving || writable.length === 0}>{saving ? "Creating…" : "Create event"}</button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * Edits an event the user organizes. All-day events edit inclusive dates, while
 * Google stores an exclusive end date, so the last day is shifted on the way in
 * and out. An untouched description is sent back verbatim so HTML formatting
 * from other calendar clients survives the plain-text editor.
 */
export function EditCalendarEventDialog({
  event,
  onClose,
  onSaved,
}: {
  event: ScheduleEvent;
  onClose(): void;
  onSaved(event: ScheduleEvent): void;
}) {
  const initialDescription = useMemo(() => event.description ? calendarDescriptionText(event.description) : "", [event.description]);
  const [title, setTitle] = useState(event.title);
  const [allDay, setAllDay] = useState(event.allDay);
  const [startValue, setStartValue] = useState(() => event.allDay ? event.start.slice(0, 10) : localDateTimeValue(new Date(event.start)));
  const [endValue, setEndValue] = useState(() => event.allDay
    ? dateInputValue(addDays(localDate(event.end), -1))
    : localDateTimeValue(new Date(event.end)));
  const [location, setLocation] = useState(event.location ?? "");
  const [invitees, setInvitees] = useState<string[]>(() => event.attendees ?? []);
  const [pendingInvitee, setPendingInvitee] = useState("");
  const [description, setDescription] = useState(initialDescription);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function changeAllDay(next: boolean) {
    setAllDay(next);
    if (next) {
      setStartValue(startValue.slice(0, 10));
      setEndValue(endValue.slice(0, 10) < startValue.slice(0, 10) ? startValue.slice(0, 10) : endValue.slice(0, 10));
    } else {
      setStartValue(`${startValue.slice(0, 10)}T09:00`);
      setEndValue(`${startValue.slice(0, 10)}T10:00`);
    }
  }

  async function submit(formEvent: FormEvent<HTMLFormElement>) {
    formEvent.preventDefault();
    setError(null);
    if (blockedByPendingInvitee(formEvent.currentTarget, pendingInvitee)) return;
    const attendees = invitees;
    if (!event.calendarId) { setError("This event's calendar is unknown. Refresh the calendar and try again."); return; }
    if (!title.trim()) { setError("Enter an event title."); return; }
    let start: string;
    let end: string;
    if (allDay) {
      const startDay = localDate(startValue);
      const lastDay = localDate(endValue);
      if (!Number.isFinite(startDay.getTime()) || !Number.isFinite(lastDay.getTime()) || lastDay < startDay) {
        setError("The end date must be on or after the start date."); return;
      }
      start = dateInputValue(startDay);
      end = dateInputValue(addDays(lastDay, 1));
    } else {
      const startsAt = new Date(startValue);
      const endsAt = new Date(endValue);
      if (!Number.isFinite(startsAt.getTime()) || !Number.isFinite(endsAt.getTime()) || endsAt <= startsAt) {
        setError("The end time must be after the start time."); return;
      }
      start = startsAt.toISOString();
      end = endsAt.toISOString();
    }
    const invalid = attendees.filter((email) => !isEmailAddress(email));
    if (invalid.length) {
      setError(`Check these invitee email addresses: ${invalid.join(", ")}.`); return;
    }
    const request: UpdateCalendarEventRequest = {
      accountId: event.accountId,
      calendarId: event.calendarId,
      eventId: event.id,
      title: title.trim(),
      start,
      end,
      allDay,
      location,
      description: description === initialDescription ? event.description ?? "" : description,
      attendees: [...new Set(attendees.map((email) => email.toLowerCase()))],
    };
    setSaving(true);
    try {
      onSaved(await mailClient.updateCalendarEvent(request));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  const timeType = allDay ? "date" : "datetime-local";
  return (
    <Modal title="Edit event" className="calendar-event-modal" onClose={onClose} dismissible={!saving}>
      <form className="modal-form" onSubmit={(formEvent) => { void submit(formEvent); }} onKeyDown={(keyEvent) => {
        if ((keyEvent.metaKey || keyEvent.ctrlKey) && keyEvent.key === "Enter") {
          keyEvent.preventDefault();
          keyEvent.currentTarget.requestSubmit();
        }
      }}>
        <label>Title<input value={title} onChange={(change) => setTitle(change.target.value)} maxLength={1024} required placeholder="Add title" /></label>
        <label className="calendar-event-all-day-field"><input type="checkbox" checked={allDay} onChange={(change) => changeAllDay(change.target.checked)} />All day</label>
        <div className="calendar-event-time-fields">
          <label>Starts<input type={timeType} value={startValue} onChange={(change) => setStartValue(change.target.value)} required /></label>
          <label>Ends<input type={timeType} value={endValue} onChange={(change) => setEndValue(change.target.value)} required /></label>
        </div>
        <label>Location<input value={location} onChange={(change) => setLocation(change.target.value)} maxLength={4096} placeholder="Add location" /></label>
        <EmailBadgeField label="Invite People" values={invitees} disabled={saving} onChange={setInvitees} onPendingChange={setPendingInvitee} />
        <label>Description<textarea value={description} onChange={(change) => setDescription(change.target.value)} rows={5} maxLength={32768} placeholder="Add meeting details" /></label>
        <p className="modal-form-context">Guests are notified of changes.</p>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={saving}>{saving ? "Saving…" : "Save changes"}</button>
        </div>
      </form>
    </Modal>
  );
}
