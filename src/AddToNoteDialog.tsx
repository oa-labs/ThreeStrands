import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { addDays, eventDate, formatUpcomingDate, startOfLocalDay } from "./calendarTime";
import { formatClockTime, isDeclined } from "./agendaEvents";
import { upcomingMeetingsWith } from "./ContactMeetings";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { mailClient } from "./data/client";
import type { Account, Message, NoteBacklink, NoteSummary, ScheduleEvent, Thread } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";
import { defaultMeetingEntry, isPreparable } from "./meetingPrep";
import { meetingDate, meetingEventReference, primaryMeetingNote } from "./meetingNoteEntry";
import { formatNoteDate } from "./noteDates";
import { noteTargetKey, todayTarget } from "./noteDrafts";
import type { AddToNoteChoice } from "./noteCapture";
import { formatAttachmentSize } from "./threadPresentation";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import {
  ADD_TO_NOTE_MEETING_DAYS, MAX_ADDITION_FILES, MAX_MEETING_NOTE_SUGGESTIONS, MAX_NEW_NOTE_TITLE_CHARS,
  MAX_NOTE_ATTACHMENT_BYTES, MAX_NOTE_SEARCH_SUGGESTIONS, MAX_RECENT_NOTE_SUGGESTIONS,
} from "./addToNotePolicy";
import { isImageAttachment, messageFiles, messageSource, quoteProblem, wholeMessageText } from "./addToNote";

/** How long the picker waits after typing before searching notes. */
const SEARCH_DELAY_MS = 150;

type Option = {
  key: string;
  label: string;
  detail?: string;
  /** What choosing writes to. A meeting's note is created on submit, from the event. */
  choice?: AddToNoteChoice;
  meeting?: ScheduleEvent;
  /** Why the option can't be chosen. */
  problem?: string;
};

/** Why `text` can't name a new note, by the native rules, or null. */
function newTitleProblem(text: string): string | null {
  if ([...text].length > MAX_NEW_NOTE_TITLE_CHARS) return `A note title can be at most ${MAX_NEW_NOTE_TITLE_CHARS} characters`;
  if (/[[\]|]/.test(text)) return "A note title can't contain [, ], or |";
  if (/^\d{4}-\d{2}-\d{2}$/.test(text)) return "A date names that day's note; choose another title";
  return null;
}

const noteOption = (note: Pick<NoteSummary, "id" | "title" | "dailyDate">, detail?: string): Option => ({
  key: `note:${note.id}`,
  label: note.dailyDate ? formatNoteDate(note.dailyDate) : note.title,
  detail,
  choice: { kind: "note", id: note.id },
});

/**
 * Add to Note: choose a note, see what will be added, and copy files, all
 * from one message. Nothing is written until the user submits.
 */
export function AddToNoteDialog({ thread, message, quote: selected, accounts, ownAddresses, people, calendarConnected, timeZone, now: start, onClose, onSubmit }: {
  thread: Thread;
  message: Message;
  /** The passage the user had selected in the message, if any. */
  quote: string | null;
  accounts: readonly Account[];
  /** The user's addresses, left out of a meeting note's attendees. */
  ownAddresses: readonly string[];
  /** The people on the conversation other than the user. */
  people: readonly { email: string; name: string }[];
  calendarConnected: boolean;
  timeZone: string;
  now?: Date;
  onClose(): void;
  /** Adds; a rejection keeps the dialog open with its message. */
  onSubmit(input: { choice: AddToNoteChoice; quote: string | null; dateLabel: string; files: string[] }): Promise<void>;
}) {
  // The clock is read once, so the day's note and the meetings stay those of the moment the dialog opened.
  const [now] = useState(() => start ?? new Date());
  const listId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [highlighted, setHighlighted] = useState(0);
  const [chosen, setChosen] = useState<Option | null>(null);
  const [includeText, setIncludeText] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Meetings in the next few days with someone on the conversation. A range
  // fixed for the dialog's lifetime keeps the schedule cache warm.
  const [range] = useState(() => {
    const start = startOfLocalDay(now);
    return { timeMin: start.toISOString(), timeMax: addDays(start, ADD_TO_NOTE_MEETING_DAYS).toISOString() };
  });
  const { events } = useCalendarSchedule({ ...range, timeZone }, false, calendarConnected);
  const meetings = useMemo(() => !calendarConnected ? [] : upcomingMeetingsWith(
    // The range is requested, but a cached schedule may hold more than it: the window is enforced here too.
    events.filter((event) => isPreparable(event) && !isDeclined(event) && eventDate(event).getTime() < Date.parse(range.timeMax)),
    people, now.getTime(), MAX_MEETING_NOTE_SUGGESTIONS,
  ), [calendarConnected, events, people, now, range.timeMax]);
  const meetingKey = meetings.map(({ event }) => `${event.accountId}:${event.id}`).join("\n");
  const [meetingNotes, setMeetingNotes] = useState<{ key: string; notes: Map<string, NoteBacklink | null> } | null>(null);
  useEffect(() => {
    if (!meetings.length) return;
    let active = true;
    void Promise.all(meetings.map(async ({ event }) => {
      const reference = meetingEventReference(event);
      const note = reference ? primaryMeetingNote(await mailClient.noteBacklinks(reference)) : undefined;
      return [`${event.accountId}:${event.id}`, note && !note.dailyDate ? note : null] as const;
    })).then((rows) => { if (active) setMeetingNotes({ key: meetingKey, notes: new Map(rows) }); }, logBackgroundFailure("Meeting notes for Add to Note"));
    return () => { active = false; };
    // The meetings are identified by `meetingKey`; the array is rebuilt on every schedule refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meetingKey]);

  const [recent, setRecent] = useState<NoteSummary[]>([]);
  useEffect(() => {
    let active = true;
    void mailClient.listRecentNotes(MAX_RECENT_NOTE_SUGGESTIONS + 1).then((notes) => { if (active) setRecent(notes); }, logBackgroundFailure("Recent notes for Add to Note"));
    return () => { active = false; };
  }, []);

  const term = query.trim();
  const [found, setFound] = useState<{ term: string; hits: NoteSummary[] } | null>(null);
  useEffect(() => {
    if (!term) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void mailClient.searchNotes(term, MAX_NOTE_SEARCH_SUGGESTIONS).then((hits) => {
        if (active) setFound({ term, hits: hits.filter((hit) => !hit.trashed).map((hit) => ({ id: hit.id, title: hit.title, dailyDate: hit.dailyDate, preview: "", updatedAt: "", visitedAt: null, trashedAt: null })) });
      }, (reason) => { if (active) setError(errorMessage(reason)); });
    }, SEARCH_DELAY_MS);
    return () => { active = false; window.clearTimeout(timer); };
  }, [term]);

  const today = noteTargetKey(todayTarget(now));
  const options = useMemo<Option[]>(() => {
    if (term) {
      const hits = found?.term === term ? found.hits : [];
      const problem = newTitleProblem(term);
      const exact = hits.some((hit) => !hit.dailyDate && hit.title.trim().toLocaleLowerCase() === term.toLocaleLowerCase());
      return [
        ...hits.map((hit) => noteOption(hit)),
        ...(exact ? [] : [{ key: "new", label: `Create note “${term}”`, choice: problem ? undefined : { kind: "new" as const, title: term }, problem: problem ?? undefined }]),
      ];
    }
    const rows: Option[] = [];
    for (const { event } of meetings) {
      const note = meetingNotes?.key === meetingKey ? meetingNotes.notes.get(`${event.accountId}:${event.id}`) : undefined;
      if (note === undefined) continue;
      const when = `meeting ${formatUpcomingDate(event, now).toLocaleLowerCase()}${event.allDay ? "" : ` ${formatClockTime(eventDate(event))}`}`;
      rows.push(note
        ? { key: `meeting:${event.id}`, label: note.title, detail: when, choice: { kind: "note", id: note.noteId } }
        : { key: `meeting:${event.id}`, label: `Create a note for “${event.title}”`, detail: when, meeting: event });
    }
    rows.push({ key: "today", label: "Today's note", detail: formatNoteDate(today.slice("daily:".length)), choice: { kind: "note", id: today } });
    const taken = new Set(rows.flatMap((row) => row.choice?.kind === "note" ? [row.choice.id] : []));
    rows.push(...recent.filter((note) => !taken.has(note.id)).slice(0, MAX_RECENT_NOTE_SUGGESTIONS).map((note) => noteOption(note, "Recently opened")));
    return rows;
  }, [term, found, meetings, meetingNotes, meetingKey, recent, today, now]);
  const active = options.length ? Math.min(highlighted, options.length - 1) : -1;

  const source = messageSource(message, accounts, now);
  const text = wholeMessageText(message);
  const quote = selected ?? (includeText && text ? text : null);
  const problem = quoteProblem(quote);
  const { files, webImages } = useMemo(() => messageFiles(message), [message]);
  const imageCount = files.filter(isImageAttachment).length;
  const tooManyFiles = picked.size > MAX_ADDITION_FILES;
  const canSubmit = Boolean(chosen) && !problem && !tooManyFiles && !saving;

  const choose = (option: Option) => {
    if (!option.choice && !option.meeting) return;
    setChosen(option);
    setError(null);
  };
  const submit = async () => {
    if (!chosen || !canSubmit) return;
    setSaving(true); setError(null);
    try {
      const choice: AddToNoteChoice | undefined = chosen.meeting
        ? { kind: "meeting", date: meetingDate(chosen.meeting), entry: await defaultMeetingEntry(chosen.meeting, ownAddresses) }
        : chosen.choice;
      if (!choice) throw new Error("Choose a note first.");
      await onSubmit({ choice, quote, dateLabel: source.dateLabel, files: files.filter((file) => picked.has(file.id)).map((file) => file.id) });
    } catch (reason) {
      setError(errorMessage(reason));
      setSaving(false);
    }
  };

  const toggleFile = (id: string, on: boolean) => setPicked((current) => { const next = new Set(current); if (on) next.add(id); else next.delete(id); return next; });
  const countText = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;
  const otherFiles = files.length - imageCount;
  const summary = [imageCount ? countText(imageCount, "image", "images") : "", otherFiles ? countText(otherFiles, "other file", "other files") : ""].filter(Boolean).join(" and ");

  return <Modal title="Add to Note" className="add-to-note-modal" initialFocusRef={inputRef} onClose={onClose}>
    <form className="modal-form" onSubmit={(change) => { change.preventDefault(); void submit(); }}
      onKeyDown={(event) => { if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); void submit(); } }}>
      <div className="add-to-note-picker">
        <label htmlFor={`${listId}-input`}>Note</label>
        <input {...SEARCH_INPUT_ATTRIBUTES} id={`${listId}-input`} ref={inputRef} role="combobox" aria-expanded="true" aria-controls={listId} aria-autocomplete="list"
          aria-activedescendant={active >= 0 ? `${listId}-option-${active}` : undefined} aria-describedby={chosen ? `${listId}-chosen` : undefined}
          placeholder="Search notes or create one" value={query} disabled={saving}
          onChange={(change) => { setQuery(change.target.value); setHighlighted(0); }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") { event.preventDefault(); setHighlighted(Math.min(active + 1, options.length - 1)); }
            else if (event.key === "ArrowUp") { event.preventDefault(); setHighlighted(Math.max(active - 1, 0)); }
            else if (event.key === "Enter" && !event.metaKey && !event.ctrlKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (options[active]) choose(options[active]); }
          }} />
        <ul id={listId} role="listbox" aria-label="Notes" className="add-to-note-options">
          {options.map((option, index) => <li key={option.key} id={`${listId}-option-${index}`} role="option" aria-selected={chosen?.key === option.key} aria-disabled={option.choice || option.meeting ? undefined : true}
            className={`add-to-note-option${index === active ? " active" : ""}`} onMouseEnter={() => setHighlighted(index)} onMouseDown={(event) => event.preventDefault()} onClick={() => choose(option)}>
            <span>{option.label}</span>
            {option.problem ? <small>{option.problem}</small> : option.detail ? <small>{option.detail}</small> : null}
          </li>)}
        </ul>
        <p id={`${listId}-chosen`} className="modal-form-hint" role="status">{chosen ? `Adding to ${chosen.label}` : "Choose a note"}</p>
      </div>

      <section className="add-to-note-adding" aria-label="Adding">
        <h3>Adding</h3>
        {selected !== null || (includeText && text) ? <>
          <blockquote className="add-to-note-quote">{quote}</blockquote>
          <p className="modal-form-hint">From {source.label}, {source.dateLabel}</p>
        </> : <p className="modal-form-hint">A link to the conversation, “{thread.subject.trim() || "(No subject)"}”</p>}
        {selected === null ? <label className="modal-form-check"><input type="checkbox" checked={includeText} disabled={saving || !text} aria-describedby={text ? undefined : `${listId}-no-text`} onChange={(change) => setIncludeText(change.target.checked)} />Include message text</label> : null}
        {selected === null && !text ? <p id={`${listId}-no-text`} className="modal-form-hint">This message has no text.</p> : null}
        {problem ? <p className="form-error" role="alert">{problem}</p> : null}
      </section>

      {files.length || webImages ? <fieldset className="add-to-note-files">
        <legend>Files</legend>
        {summary ? <p className="modal-form-hint">{summary} from this message</p> : null}
        {files.map((file) => <label key={file.id} className="modal-form-check">
          <input type="checkbox" checked={picked.has(file.id)} disabled={saving || !file.fits} aria-describedby={file.fits ? undefined : `${listId}-${file.id}-big`} onChange={(change) => toggleFile(file.id, change.target.checked)} />
          <span>{file.filename}</span><small>{formatAttachmentSize(file.size)}</small>
          {file.fits ? null : <small id={`${listId}-${file.id}-big`}>Larger than {MAX_NOTE_ATTACHMENT_BYTES / (1024 * 1024)} MB, the most a note can hold</small>}
        </label>)}
        {webImages ? <p className="modal-form-hint">{countText(webImages, "image loads", "images load")} from the web and can't be copied, so {webImages === 1 ? "it was" : "they were"} left out.</p> : null}
        {tooManyFiles ? <p className="form-error" role="alert">A note can take at most {MAX_ADDITION_FILES} files at a time.</p> : null}
      </fieldset> : null}

      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose} disabled={saving}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!canSubmit}>{saving ? "Adding…" : "Add to Note"}</button>
      </div>
    </form>
  </Modal>;
}
