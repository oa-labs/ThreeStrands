import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { EmailBadgeField } from "./ContactAddressField";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { eventDate } from "./calendarTime";
import { errorMessage, logBackgroundFailure } from "./errors";
import { normalizeContactAddress } from "./noteEntities";
import { MAX_MEETING_ATTENDEES, meetingNoteTitle, type MeetingEntry } from "./noteMeetings";
import { meetingAttendees, meetingDate, meetingEntry, meetingTime } from "./meetingNoteEntry";

/** How long the dialog waits after typing before checking the meeting note's title. */
const TITLE_CHECK_DELAY_MS = 150;

/**
 * Add Note for a calendar event: the meeting's name and attendees, filled in
 * from the event, and whether to link a note named after the meeting. The
 * event itself never changes.
 */
export function MeetingNoteDialog({ event, ownAddresses, onClose, onSubmit, onOpenNote }: {
  event: ScheduleEvent;
  /** The user's account addresses, left out of the attendees. */
  ownAddresses: readonly string[];
  onClose(): void;
  /** Adds the meeting to `date`'s note; a rejection keeps the dialog open with its message. */
  onSubmit(date: string, entry: MeetingEntry): Promise<void>;
  /** Opens `date`'s note, offered when adding was refused. */
  onOpenNote?(date: string): void;
}) {
  const nameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(event.title);
  const [attendees, setAttendees] = useState(() => meetingAttendees(event, ownAddresses));
  const [names, setNames] = useState<ReadonlyMap<string, string>>(new Map());
  const [pending, setPending] = useState("");
  const [linkNote, setLinkNote] = useState(false);
  const [existing, setExisting] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const title = meetingNoteTitle(name);

  // Names of the people the event lists, from contacts and the people the user emails.
  useEffect(() => {
    let active = true;
    const listed = meetingAttendees(event, ownAddresses).slice(0, MAX_MEETING_ATTENDEES);
    void Promise.all(listed.map((address) => mailClient.listContactProfiles(address, 5).then((found) => {
      const person = found.find((profile) => profile.addresses.some((other) => normalizeContactAddress(other) === address));
      return [address, person?.displayName?.trim() ?? ""] as const;
    }))).then((found) => {
      if (active) setNames((current) => new Map([...found.filter(([, label]) => label), ...current]));
    }, logBackgroundFailure("Meeting attendee names"));
    return () => { active = false; };
  }, [event, ownAddresses]);

  useEffect(() => {
    if (!linkNote) { setExisting(null); return; }
    let active = true;
    const timer = window.setTimeout(() => {
      void mailClient.findNoteByTitle(title).then((found) => { if (active) setExisting(Boolean(found && !found.dailyDate)); }, () => { if (active) setExisting(null); });
    }, TITLE_CHECK_DELAY_MS);
    return () => { active = false; window.clearTimeout(timer); };
  }, [linkNote, title]);

  const tooMany = attendees.length > MAX_MEETING_ATTENDEES;
  const submit = async () => {
    if (pending.trim()) { setError("Finish or remove the attendee you're typing first."); return; }
    if (tooMany) return;
    setSaving(true); setError(null);
    try {
      const contactIds = attendees.length ? await mailClient.resolveContactIds(attendees.map(normalizeContactAddress)) : {};
      await onSubmit(meetingDate(event), meetingEntry(event, { name, attendees, names, contactIds, linkNote }));
    } catch (reason) {
      setError(errorMessage(reason));
      setSaving(false);
    }
  };

  const day = eventDate(event).toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });
  return <Modal title="Add Note" onClose={onClose} initialFocusRef={nameRef}>
    <form className="modal-form" onSubmit={(change) => { change.preventDefault(); void submit(); }}>
      <p className="modal-form-context">{meetingTime(event)} · {day}</p>
      <label>Meeting name<input ref={nameRef} value={name} onChange={(change) => setName(change.target.value)} maxLength={1024} placeholder="Meeting" disabled={saving} /></label>
      <EmailBadgeField label="Attendees" values={attendees} calendarAccount={event.accountId} disabled={saving} onChange={setAttendees} onPendingChange={setPending}
        knownNames={names} onNamesChange={(chosen) => setNames((current) => new Map([...current, ...chosen]))} />
      {tooMany ? <p className="form-error">A meeting line can name at most {MAX_MEETING_ATTENDEES} attendees.</p> : null}
      <label className="meeting-note-link-field"><input type="checkbox" checked={linkNote} disabled={saving} onChange={(change) => setLinkNote(change.target.checked)} />Create backlinked note</label>
      {linkNote ? <p className="modal-form-context" role="status">{existing ? `Links the existing note “${title}”.` : `Links a new note, “${title}”.`}</p> : null}
      {error ? <p className="form-error" role="alert">{error}{onOpenNote ? <> <button type="button" className="btn-link" onClick={() => onOpenNote(meetingDate(event))}>Open Note</button></> : null}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={saving || tooMany}>{saving ? "Adding…" : "Add to Daily Note"}</button>
      </div>
    </form>
  </Modal>;
}
