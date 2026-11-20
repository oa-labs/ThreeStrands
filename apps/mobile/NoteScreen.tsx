import { useEffect, useRef, useState, type FormEvent } from "react";
import { ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { skyPhase } from "../../src/backdropPreference";
import { TodayBackdrop } from "../../src/TodayBackdrop";
import type { Note } from "../../src/domain";
import { parseNoteTargetKey, todayTarget, type NoteTarget } from "../../src/noteDrafts";
import type { NoteItem } from "../../src/noteEntities";
import { ActionsMenu } from "./ActionsMenu";
import { errorText, mobile, type NoteAction } from "./api";
import { NotePreview } from "./NotePreview";
import { useMobileNote } from "./useMobileNote";
import { useResource } from "./useResource";
import { useDurableDraft } from "./useDurableDraft";

function nearbyDate(date: string, delta: number) {
  const parsed = new Date(`${date}T12:00:00`);
  parsed.setDate(parsed.getDate() + delta);
  return todayTarget(parsed).date;
}

const fullDate = new Intl.DateTimeFormat(undefined, { dateStyle: "full" });
const dayOfWeek = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dayOfMonth = new Intl.DateTimeFormat(undefined, { day: "numeric" });

const compactDay = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "short", day: "numeric" });
const compactDayWithYear = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric" });

export function formatDailyHeading(date: string, now: Date) {
  const day = new Date(`${date}T12:00:00`);
  return (day.getFullYear() === now.getFullYear() ? compactDay : compactDayWithYear).format(day);
}

export function DailyNavigation({ date, today, now, backdrop, onDate }: {
  date: string; today: string; now: Date; backdrop: boolean; onDate(date: string): void;
}) {
  return <section className="daily-heading" aria-label="Daily note dates" data-sky={backdrop ? skyPhase(now) : undefined}>
    {backdrop && <TodayBackdrop now={now} />}
    <div className="date-strip">
      <button aria-label="Previous day" onClick={() => onDate(nearbyDate(date, -1))}><ChevronLeft size={18} aria-hidden="true" /></button>
      {[-2, -1, 0, 1, 2].map(delta => {
        const day = nearbyDate(date, delta);
        const parsed = new Date(`${day}T12:00:00`);
        return <button key={day} aria-label={fullDate.format(parsed)} aria-current={day === today ? "date" : undefined}
          aria-pressed={day === date} onClick={() => onDate(day)}>
          <span>{dayOfWeek.format(parsed)}</span><strong>{dayOfMonth.format(parsed)}</strong>
        </button>;
      })}
      <button aria-label="Next day" onClick={() => onDate(nearbyDate(date, 1))}><ChevronRight size={18} aria-hidden="true" /></button>
    </div>
    {date !== today && <button className="back-to-today" onClick={() => onDate(today)}>Back to Today</button>}
  </section>;
}

export function NoteScreen({ target, revision, focused, startEditing, onOpen, onChange, onTitle }: {
  target: NoteTarget; revision: number; focused?: string; startEditing?: boolean;
  onOpen(target: NoteTarget): void; onChange(): void; onTitle?(title: string): void;
}) {
  const [editing, setEditing] = useState(startEditing ?? false);
  const [composing, setComposing] = useState(false);
  const editor = useMobileNote(target, revision, composing);
  const [linkPicker, setLinkPicker] = useState(false);
  const [linkQuery, setLinkQuery] = useState("");
  const [working, setWorking] = useState(false);
  const [actionError, setActionError] = useState<string>();
  const [actionMessage, setActionMessage] = useState<string>();
  const [trashedHere, setTrashedHere] = useState(false);
  const textArea = useRef<HTMLTextAreaElement>(null);
  const linkRange = useRef<{ start: number; end: number } | null>(null);
  const savedCopies = useRef(new Set<string>());
  const titles = useResource(async () => linkPicker ? mobile.titles(linkQuery) : [], [linkPicker, linkQuery]);
  const { note, data, body, title, draft, stale, saving } = editor;
  const conflicts = data?.conflicts ?? [];
  const daily = target.kind === "daily";
  const isEditable = editor.draftReady && !!data && (daily || !!note) && !note?.trashed && conflicts.length === 0;

  useEffect(() => {
    if (note?.title) onTitle?.(note.title);
  }, [note?.title, onTitle]);

  async function action(run: () => Promise<void>) {
    setWorking(true);
    setActionError(undefined);
    setActionMessage(undefined);
    try { await run(); } catch (error) { setActionError(errorText(error)); }
    finally { setWorking(false); }
  }

  async function openWiki(name: string) {
    await action(async () => {
      const found = await mobile.findNote(name);
      if (found) onOpen(parseNoteTargetKey(found.id));
      else setActionMessage(`No note named “${name}”.`);
    });
  }

  function insert(value: string, asList: boolean) {
    const el = textArea.current;
    const start = linkRange.current?.start ?? el?.selectionStart ?? body.length;
    const end = linkRange.current?.end ?? el?.selectionEnd ?? start;
    const before = body.slice(0, start);
    const newline = asList && before && !before.endsWith("\n") ? "\n" : "";
    const next = `${newline}${value}`;
    editor.edit({ body: before + next + body.slice(end) });
    linkRange.current = null;
    setLinkPicker(false);
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(start + next.length, start + next.length);
    });
  }

  async function copyVersion(key: string, copyTitle: string, copyBody: string) {
    if (savedCopies.current.has(key)) return;
    for (let index = 1; ; index += 1) {
      const suffix = index === 1 ? " (copy)" : ` (copy ${index})`;
      const stem = [...copyTitle].slice(0, 200 - suffix.length).join("");
      const candidate = stem + suffix;
      if (await mobile.findNote(candidate)) continue;
      await mobile.createNote(candidate, copyBody);
      savedCopies.current.add(key);
      return;
    }
  }

  function check(item: NoteItem) {
    if (!note || working || draft || conflicts.length) return;
    void action(async () => {
      await mobile.checkNoteItem(note.id, item.id, note.updatedAt, !item.checked);
      editor.refresh();
      onChange();
    });
  }

  function changeNote(actionName: NoteAction) {
    if (!note) return;
    void action(async () => {
      const updated = await mobile.noteAction(note.id, actionName);
      editor.setData(current => current && { ...current, note: updated });
      if (actionName === "trash") {
        setEditing(false);
        setTrashedHere(true);
      }
      if (actionName === "restore") setTrashedHere(false);
      editor.refresh();
      onChange();
    });
  }

  return <section className="note-screen" aria-label={daily ? "Daily Note" : "Note"}>
    {editor.error && <p role="alert">{editor.error} <button onClick={editor.refresh}>Retry</button></p>}
    {!data && !editor.error && <p role="status">Loading note…</p>}
    {data && !daily && !note && !draft && <p>This note is unavailable.</p>}
    <div className="note-actions note-toolbar">
      {isEditable && !editing && <button className="quiet" onClick={() => setEditing(true)}>{daily && !body ? "Write in Daily Note" : "Edit"}</button>}
      {editing && isEditable && !stale && <button className="primary" disabled={saving || composing} onClick={async () => {
        if (await editor.save()) { setEditing(false); onChange(); }
      }}>Done</button>}
      {note && !note.trashed && <ActionsMenu label="Note actions">
        <button disabled={working} onClick={() => changeNote(note.starred ? "unstar" : "star")}>{note.starred ? "Unstar" : "Star"}</button>
        <button className="danger" disabled={working || !!draft || saving} onClick={() => changeNote("trash")}>Move to Trash</button>
      </ActionsMenu>}
      {note?.trashed && <div className="action-feedback">
        <span role="status">{trashedHere ? "Note moved to Trash." : "This note is in Trash."}</span>
        <button className="quiet" disabled={working} onClick={() => changeNote("restore")}>{trashedHere ? "Undo" : "Restore"}</button>
      </div>}
    </div>
    {(actionError || editor.saveError) && <p role="alert">{actionError || editor.saveError}</p>}
    {editor.draftError && <p role="alert">
      {editor.draftReady ? "This device could not keep the draft. Save your changes before leaving." : editor.draftError}
      {!editor.draftReady && <button onClick={editor.retryDraft}>Retry</button>}
    </p>}
    {actionMessage && <p role="status">{actionMessage}</p>}
    {stale && draft && <section className="callout" aria-label="Recovered draft">
      <h2>Recovered draft</h2>
      <p>The saved note changed while this draft was open.</p>
      <details><summary>Compare with saved note</summary><pre>{note?.body ?? "The saved note is unavailable."}</pre><pre>{draft.body}</pre></details>
      {isEditable && <button disabled={saving} onClick={() => void editor.save(true)}>Keep Draft</button>}
      <button disabled={working} onClick={() => void action(async () => {
        await copyVersion(`local:${draft.base}`, draft.title || note?.title || "Daily Note", draft.body);
        editor.discard();
        onChange();
      })}>Keep Both</button>
      <button disabled={saving} onClick={editor.discard}>Use Saved Note</button>
    </section>}
    {conflicts.map(conflict => <section className="callout" aria-label="Sync conflict" key={conflict.field}>
      <h2>{conflict.field === "title" ? "Two titles to review" : "Changes from another device"}</h2>
      {conflict.candidates.map((candidate, index) => {
        const value = String(candidate.value ?? "");
        return <details key={candidate.operationId}>
          <summary>Version {index + 1}</summary>
          <pre>{value}</pre>
          {note && <div className="note-actions">
            <button disabled={working} onClick={() => void action(async () => {
              await mobile.resolveNote(note.id, conflict.field, candidate.operationId);
              editor.refresh();
              onChange();
            })}>Use This Version</button>
            <button disabled={working} onClick={() => void action(async () => {
              const current = conflict.field === "title" ? note.authoredTitle ?? note.title : note.body;
              const other = conflict.candidates.filter(version => version.value !== current);
              for (const version of other) {
                await copyVersion(version.operationId, conflict.field === "title" ? String(version.value) : note.title,
                  conflict.field === "body" ? String(version.value) : note.body);
              }
              const keep = conflict.candidates.find(version => version.value === current) ?? candidate;
              await mobile.resolveNote(note.id, conflict.field, keep.operationId);
              editor.refresh();
              onChange();
            })}>Keep Both</button>
          </div>}
        </details>;
      })}
    </section>)}
    {editing && isEditable ? <div className="editor">
      {!daily && <input aria-label="Note title" value={title} onChange={event => editor.edit({ title: event.target.value })} />}
      <div className="note-actions" aria-label="Insert in note">
        <button onClick={() => insert("- ", true)}>List</button>
        <button onClick={() => insert("- [ ] ", true)}>Checklist</button>
        <button onClick={() => insert("- ( ) ", true)}>Task</button>
        <button onClick={() => {
          const el = textArea.current;
          if (el) linkRange.current = { start: el.selectionStart, end: el.selectionEnd };
          setLinkPicker(value => !value);
          setLinkQuery("");
        }}>Note Link</button>
      </div>
      {linkPicker && <section className="callout">
        <input type="search" aria-label="Find a note to link" value={linkQuery} onChange={event => setLinkQuery(event.target.value)} />
        {titles.error && <p role="alert">{titles.error}</p>}
        {titles.data?.map(value => <button key={value.id} onClick={() => insert(`[[${value.title}]]`, false)}>{value.title}</button>)}
        <button onClick={() => { setLinkPicker(false); linkRange.current = null; }}>Cancel</button>
      </section>}
      <textarea ref={textArea} aria-label="Note text" spellCheck autoFocus value={body}
        onChange={event => editor.edit({ body: event.target.value })}
        onBlur={() => {
          if (!composing && (daily || title === note?.title)) void editor.save();
        }}
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        placeholder="Write here…" />
      <p role="status" className="muted">{saving ? "Saving…" : editor.draftPending ? "Saving draft…" : draft ? "Draft on this device" : "Saved on this device"}</p>
    </div> : body && <NotePreview body={body} items={!draft ? data?.document?.items ?? [] : []} disabled={!!draft || working || !!note?.trashed || !!conflicts.length}
      daily={daily} focused={focused} onCheck={check} onWiki={name => void openWiki(name)}
      onUrl={url => void action(() => mobile.openUrl(url))} />}
    {data?.document?.warning && <p role="status">{data.document.warning}</p>}
    {!!data?.links?.backlinks.length && <details className="backlinks">
      <summary>Backlinks <span>{data.links.backlinks.length}</span><ChevronDown size={18} aria-hidden="true" /></summary>
      <ul className="rows">{data.links.backlinks.map((item, index) => <li key={`${item.noteId}-${index}`}>
        <button className="row-button" onClick={() => onOpen(parseNoteTargetKey(item.noteId))}>
          <strong>{item.title}</strong><span>{item.context}</span>
        </button>
      </li>)}</ul>
    </details>}
  </section>;
}

const NEW_NOTE_DRAFT = "mobile:new-note";

export function NewNote({ onSave, onCancel }: { onSave(note: Note): void; onCancel(): void }) {
  const journal = useDurableDraft(NEW_NOTE_DRAFT);
  const draft = journal.draft ?? { title: "", body: "", base: null };
  const [saving, setSaving] = useState(false);
  const [issue, setIssue] = useState<string>();
  function edit(value: Partial<typeof draft>) {
    setIssue(undefined);
    void journal.write({ ...draft, ...value });
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    try {
      const note = await mobile.createNote(draft.title, draft.body);
      void journal.write(null, draft);
      onSave(note);
    } catch (error) { setIssue(errorText(error)); }
    finally { setSaving(false); }
  }
  return <form className="editor" onSubmit={submit}>
    <label>Title<input disabled={!journal.ready || saving} value={draft.title} autoFocus required onChange={event => edit({ title: event.target.value })} /></label>
    <label>Note text<textarea disabled={!journal.ready || saving} value={draft.body} onChange={event => edit({ body: event.target.value })} /></label>
    {(issue || journal.error) && <p role="alert">{issue || journal.error} {!journal.ready && <button type="button" onClick={journal.retry}>Retry</button>}</p>}
    <div className="note-actions">
      <button type="submit" className="primary" disabled={!draft.title.trim() || saving}>Save Note</button>
      <button type="button" onClick={onCancel}>Back</button>
    </div>
  </form>;
}
