import { Copy, Plus, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { Modal } from "./AppChrome";
import { EmailBadgeField } from "./ContactAddressField";
import { mailClient } from "./data/client";
import type { DraftHandoff, DraftHandoffRequest, FollowThroughItem, FollowThroughSave, MeetingContext, MeetingNoteSource, ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import {
  discardFollowThroughRecovery, followThroughSectionText, followUpBody, followUpRecipients, MAX_FOLLOW_THROUGH_ITEMS,
  readFollowThroughRecovery, readFollowUpHandoff, rememberFollowUpHandoff, saveFollowThroughRecovery, type FollowThroughRecovery, type ReviewItem,
} from "./meetingFollowThrough";
import { meetingEventReference } from "./meetingNoteEntry";
import { saveFollowThrough } from "./noteCapture";
import { notePlainText } from "./notePreview";
import type { MailAccountChoice } from "./MeetingPreparation";

type Kind = FollowThroughItem["kind"];

const KIND_LABEL: Record<Exclude<Kind, "link">, string> = {
  decision: "Decision",
  task: "Task",
  checklist: "Checklist",
  action: "Not Tracked",
};
const dayLabel = (date: string) => new Date(`${date}T00:00:00`).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
const destinationLabel = (source: Pick<MeetingNoteSource, "dailyDate" | "title">) => source.dailyDate ? `Daily note, ${dayLabel(source.dailyDate)}` : source.title;
const newId = () => crypto.randomUUID();
const submitOnModEnter = (event: KeyboardEvent<HTMLFormElement>) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); event.currentTarget.requestSubmit(); }
};

/** The notes a follow-through can be saved to: the ones "This meeting" lists, once each. */
export function followThroughDestinations(context: MeetingContext | null): MeetingNoteSource[] {
  const seen = new Set<string>();
  return (context?.thisMeeting.items ?? []).filter((source) => !seen.has(source.noteId) && Boolean(seen.add(source.noteId)));
}

/** Plain lines of the excerpt that could be picked as decisions: its nested list items, without task markers. */
function excerptLines(source: MeetingNoteSource | undefined): string[] {
  if (!source || source.kind === "linkedNote") return [];
  return source.text.split("\n").slice(1)
    .map((line) => /^[ \t]*(?:[-+*]|\d+[.)])[ \t]+(.*)$/.exec(line)?.[1] ?? "")
    .map((line) => notePlainText(line.replace(/<!--[\s\S]*?-->/g, "").replace(/^[([][ xX]?[)\]]\s*/, "")).trim())
    .filter(Boolean);
}

/**
 * Review Follow-through: the user records the meeting's decisions and
 * actions, typed or picked from its notes, and saves them as one dated
 * section in a note about this occurrence. Nothing is inferred: every item
 * is the user's. Edits are kept on this device until saved or discarded,
 * and restored after a restart without repeating a save.
 */
export function MeetingFollowThroughDialog({ event, context, today, onClose, onSaved, onAddNote }: {
  event: ScheduleEvent;
  context: MeetingContext;
  /** The review's date, written on the section. */
  today: string;
  onClose(): void;
  onSaved(saved: FollowThroughSave & { undo(): Promise<void> }): void;
  /** Runs Add Note when the meeting has no note to save into. */
  onAddNote?(): void;
}) {
  const reference = meetingEventReference(event)!;
  const destinations = useMemo(() => followThroughDestinations(context), [context]);
  const [restored] = useState(() => readFollowThroughRecovery(reference));
  const [record, setRecord] = useState<FollowThroughRecovery>(() => restored ?? {
    reference, reviewId: newId(), operationId: newId(), items: [], updatedAt: Date.now(),
    destinationNoteId: destinations.find((source) => source.kind === "excerpt")?.noteId ?? destinations[0]?.noteId ?? null,
  });
  const [sourceIndex, setSourceIndex] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstRef = useRef<HTMLButtonElement>(null);
  const lastText = useRef<HTMLInputElement>(null);
  const [focusNew, setFocusNew] = useState(false);
  useEffect(() => { if (focusNew) { lastText.current?.focus(); setFocusNew(false); } }, [focusNew]);

  const edit = (change: (current: FollowThroughRecovery) => FollowThroughRecovery) => setRecord((current) => {
    const next = change(current);
    saveFollowThroughRecovery(next);
    return next;
  });
  const setItems = (change: (items: ReviewItem[]) => ReviewItem[]) => edit((current) => ({ ...current, items: change(current.items) }));
  const add = (item: FollowThroughItem) => { setItems((items) => [...items, { ...item, id: newId() } as ReviewItem]); if (item.kind !== "link") setFocusNew(true); };
  const discard = () => {
    discardFollowThroughRecovery(record.reviewId);
    setRecord({ ...record, reviewId: newId(), operationId: newId(), items: [] });
  };
  const close = () => {
    // A review with nothing in it isn't worth restoring.
    if (!record.items.length) discardFollowThroughRecovery(record.reviewId);
    onClose();
  };

  const source = context.thisMeeting.items[sourceIndex];
  const destination = destinations.find((candidate) => candidate.noteId === record.destinationNoteId) ?? null;
  const items: FollowThroughItem[] = record.items.map(({ id: _id, ...item }) => item as FollowThroughItem);
  const filled = items.filter((item) => item.kind === "link" || item.text.trim());
  const linkedIds = new Set(record.items.flatMap((item) => item.kind === "link" ? [item.task.note ? `note:${item.task.note}:${item.task.id}` : item.task.id] : []));
  const commitments = context.commitments.items.filter((item) => !item.candidate && !linkedIds.has(item.id));
  const preview = (() => {
    try { return filled.length ? followThroughSectionText(reference, event.title, today, filled) : ""; } catch { return ""; }
  })();
  const placement = destination ? destination.kind === "excerpt" ? `Adds after this meeting's line in ${destinationLabel(destination)}.` : `Adds at the end of “${destination.title}”, linking this meeting.` : "";

  const submit = async (submitted: FormEvent) => {
    submitted.preventDefault();
    if (!destination || !filled.length || saving) return;
    setSaving(true); setError(null);
    try {
      const saved = await saveFollowThrough(reference, { noteId: destination.noteId, dailyDate: destination.dailyDate, revision: destination.revision }, record.operationId, { date: today, meeting: event.title, items: filled });
      discardFollowThroughRecovery(record.reviewId);
      onSaved(saved);
    } catch (reason) {
      setError(errorMessage(reason));
      setSaving(false);
    }
  };

  return <Modal title="Review Follow-through" className="follow-through-modal" initialFocusRef={firstRef} onClose={close}>
    <form className="modal-form" onSubmit={(submitted) => void submit(submitted)} onKeyDown={submitOnModEnter}>
      <p className="modal-form-context">{event.title} · {dayLabel(today)}</p>
      <p className="modal-form-hint">Write down what was decided and what happens next. Each action can be a task in Tasks, a checklist item that stays in the note, or a note that isn't tracked.</p>
      {restored ? <p className="modal-form-hint" role="status">Restored the review you hadn't saved. <button type="button" className="btn-link" onClick={discard}>Discard</button></p> : null}
      {context.followThrough.items.length ? <p className="modal-form-hint">Already reviewed {context.followThrough.items.map((item) => item.date ? dayLabel(item.date) : destinationLabel(item)).join(", ")}. Saving adds another section.</p> : null}

      {context.thisMeeting.items.length ? <div className="modal-form-evidence">
        <label>Source
          <select value={sourceIndex} onChange={(change) => setSourceIndex(Number(change.target.value))} disabled={saving}>
            {context.thisMeeting.items.map((item, index) => <option key={`${item.noteId}:${item.start}`} value={index}>{destinationLabel(item)}</option>)}
          </select>
        </label>
        {source ? <blockquote aria-label="Source excerpt">{notePlainText(source.text.replace(/<!--[\s\S]*?-->/g, ""))}</blockquote> : null}
        {excerptLines(source).length ? <ul className="follow-through-picks" aria-label="Lines in the source">
          {excerptLines(source).map((line, index) => <li key={`${index}:${line}`}><span>{line}</span>
            <button type="button" className="btn btn-sm" disabled={saving} onClick={() => add({ kind: "decision", text: line })}>Add as Decision</button>
            <button type="button" className="btn btn-sm" disabled={saving} onClick={() => add({ kind: "task", text: line })}>Add as Task</button>
          </li>)}
        </ul> : null}
        {commitments.length ? <ul className="follow-through-picks" aria-label="Existing tasks">
          {commitments.map((task) => {
            const note = /^note:(.+):([^:]+)$/.exec(task.id);
            return <li key={task.id}><span>{task.title}</span>
              <button type="button" className="btn btn-sm" disabled={saving} onClick={() => add({ kind: "link", title: task.title, task: note ? { version: 1, kind: "task", id: note[2], note: note[1] } : { version: 1, kind: "task", id: task.id } })}>Link Task</button>
            </li>;
          })}
        </ul> : null}
      </div> : null}

      <fieldset className="follow-through-items" disabled={saving}>
        <legend>Decisions and Actions</legend>
        {record.items.length === 0 ? <p className="modal-form-hint">Nothing added yet.</p> : null}
        {record.items.map((item, index) => <div key={item.id} className="follow-through-item">
          {item.kind === "link"
            ? <><span className="follow-through-kind">Linked Task</span><span className="follow-through-linked">{item.title}</span></>
            : <>
              <select aria-label={`Kind of Item ${index + 1}`} value={item.kind} onChange={(change) => setItems((all) => all.map((other) => other.id === item.id && other.kind !== "link" ? { ...other, kind: change.target.value as Exclude<Kind, "link"> } : other))}>
                {(Object.keys(KIND_LABEL) as Exclude<Kind, "link">[]).map((kind) => <option key={kind} value={kind}>{KIND_LABEL[kind]}</option>)}
              </select>
              <input ref={index === record.items.length - 1 ? lastText : undefined} aria-label={`Item ${index + 1}`} value={item.text} maxLength={2000}
                onChange={(change) => setItems((all) => all.map((other) => other.id === item.id && other.kind !== "link" ? { ...other, text: change.target.value } : other))} />
            </>}
          <button type="button" className="btn-icon btn-icon-sm" aria-label={`Remove Item ${index + 1}`} title="Remove" onClick={() => setItems((all) => all.filter((other) => other.id !== item.id))}><X size={ICON_SIZE.sm} aria-hidden="true" /></button>
        </div>)}
        <div className="follow-through-add">
          <button ref={firstRef} type="button" className="btn btn-sm" disabled={record.items.length >= MAX_FOLLOW_THROUGH_ITEMS} onClick={() => add({ kind: "decision", text: "" })}><Plus size={ICON_SIZE.sm} aria-hidden="true" />Add Decision</button>
          <button type="button" className="btn btn-sm" disabled={record.items.length >= MAX_FOLLOW_THROUGH_ITEMS} onClick={() => add({ kind: "task", text: "" })}><Plus size={ICON_SIZE.sm} aria-hidden="true" />Add Action</button>
        </div>
      </fieldset>

      {destinations.length ? <label>Save To
        <select value={record.destinationNoteId ?? ""} disabled={saving} onChange={(change) => edit((current) => ({ ...current, destinationNoteId: change.target.value }))}>
          {destinations.map((item) => <option key={item.noteId} value={item.noteId}>{destinationLabel(item)}</option>)}
        </select>
      </label> : <p className="modal-form-hint">This meeting has no note yet. {onAddNote ? <><button type="button" className="btn-link" onClick={() => { onClose(); onAddNote(); }}>Add Note</button> first; your review is kept.</> : "Add a note for it first; your review is kept."}</p>}
      {destination && preview ? <div className="modal-form-evidence">
        <span>{placement}</span>
        <pre className="follow-through-preview" aria-label="Insertion preview">{preview}</pre>
      </div> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={close} disabled={saving}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={saving || !destination || !filled.length}>{saving ? "Saving…" : "Save Follow-through"}</button>
      </div>
    </form>
  </Modal>;
}

const HANDOFF_STATE: Record<Exclude<DraftHandoff["state"], "open">, string> = {
  outbox: "The follow-up for this meeting is in Outbox.",
  sent: "The follow-up for this meeting was sent.",
  missing: "The follow-up draft for this meeting is no longer in Drafts. It was discarded, or sent or deleted on another device.",
};

/**
 * Draft Follow-up: reviews the sending account, recipients, subject, and
 * body, then opens or resumes one durable draft in the composer. Nothing is
 * sent or queued; the user sends from the composer. Without a mail account
 * the text can only be copied.
 */
export function MeetingFollowUpDialog({ event, context, items, mailAccounts, ownAddresses, onClose, onOpen }: {
  event: ScheduleEvent;
  context: MeetingContext;
  /** The reviewed decisions and actions the body starts from. */
  items: readonly FollowThroughItem[];
  mailAccounts: readonly MailAccountChoice[];
  ownAddresses: readonly string[];
  onClose(): void;
  /** Saves the open composer, then opens or resumes the handoff's draft. */
  onOpen(request: DraftHandoffRequest): Promise<DraftHandoff>;
}) {
  const reference = meetingEventReference(event)!;
  const [handoffId, setHandoffId] = useState(() => readFollowUpHandoff(reference));
  const [existing, setExisting] = useState<DraftHandoff | null | "checking">(handoffId ? "checking" : null);
  const matching = mailAccounts.find((account) => account.email.toLowerCase() === event.accountId.toLowerCase());
  const [account, setAccount] = useState(matching?.email ?? "");
  const [to, setTo] = useState(() => followUpRecipients(context, ownAddresses));
  const [replyTo, setReplyTo] = useState("");
  const [subject, setSubject] = useState(`Follow-up: ${event.title}`);
  const [body, setBody] = useState(() => followUpBody(event.title, items));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const first = useRef<HTMLSelectElement>(null);
  const subjectRef = useRef<HTMLInputElement>(null);
  const openRef = useRef<HTMLButtonElement>(null);
  const conversations = context.conversations.items.filter((item) => item.related);
  const conversation = conversations.find((item) => item.threadId === replyTo);

  useEffect(() => {
    if (!handoffId) return;
    let live = true;
    // Query the handoff before anything else, so a lost response is never retried blindly.
    mailClient.draftHandoffStatus(handoffId).then(
      (status) => { if (live) setExisting(status); },
      (reason) => { if (live) { setExisting(null); setError(errorMessage(reason)); } },
    );
    return () => { live = false; };
  }, [handoffId]);

  const open = async (id: string) => {
    setBusy(true); setError(null);
    try {
      let mode: DraftHandoffRequest["mode"] = "new", sourceId: string | null = null, from = account;
      if (conversation) {
        const detail = await mailClient.getThread(conversation.threadId);
        const latest = detail.messages.at(-1);
        if (!latest) throw new Error("That conversation has no messages on this device.");
        mode = "replyAll"; sourceId = latest.id; from = conversation.accountId;
      }
      // Remember the handoff before asking, so reopening after a lost response resumes it.
      rememberFollowUpHandoff(reference, id);
      const result = await onOpen({ handoffId: id, account: from, mode, sourceId, to: to.join(", "), cc: "", subject, body });
      if (result.state === "open") onClose();
      else { setExisting(result); setBusy(false); }
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };
  const copy = async () => {
    try { await navigator.clipboard.writeText(`${subject}\n\n${body}`); setCopied(true); }
    catch (reason) { setError(errorMessage(reason)); }
  };
  const startNew = () => { rememberFollowUpHandoff(reference, null); setHandoffId(null); setExisting(null); };

  if (existing === "checking") return <Modal title="Draft Follow-up" onClose={onClose}><p className="modal-form-hint" role="status">Checking for an earlier follow-up…</p></Modal>;
  if (existing && handoffId) {
    return <Modal title="Draft Follow-up" initialFocusRef={openRef} onClose={onClose}>
      <div className="modal-form">
        <p className="modal-form-hint" role="status">{existing.state === "open" ? "A follow-up draft for this meeting is in Drafts." : HANDOFF_STATE[existing.state]}</p>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose} disabled={busy}>Cancel</button>
          {existing.state === "open"
            ? <><button type="button" className="btn" onClick={startNew} disabled={busy}>New Draft</button><button ref={openRef} type="button" className="btn btn-primary" disabled={busy} onClick={() => void open(handoffId)}>Open Draft</button></>
            : <button ref={openRef} type="button" className="btn btn-primary" onClick={startNew}>New Draft</button>}
        </div>
      </div>
    </Modal>;
  }

  const canDraft = mailAccounts.length > 0;
  return <Modal title="Draft Follow-up" className="follow-through-modal" initialFocusRef={canDraft ? first : subjectRef} onClose={onClose}>
    <form className="modal-form" onKeyDown={submitOnModEnter} onSubmit={(submitted) => { submitted.preventDefault(); if (canDraft && !busy) void open(handoffId ?? newId()); }}>
      <p className="modal-form-hint">Opens a draft in the composer for you to finish and send. Nothing is sent until you send it.</p>
      {canDraft ? <>
        <label>Reply To
          <select ref={first} value={replyTo} disabled={busy} onChange={(change) => setReplyTo(change.target.value)}>
            <option value="">New message</option>
            {conversations.map((item) => <option key={item.threadId} value={item.threadId}>{item.subject || "(No subject)"} · {item.accountId}</option>)}
          </select>
        </label>
        {conversation ? <p className="modal-form-hint">The reply goes to that conversation's people from {conversation.accountId}, with its subject.</p> : <>
          <label>From
            <select value={account} disabled={busy} onChange={(change) => setAccount(change.target.value)}>
              {matching ? null : <option value="">Choose an account</option>}
              {mailAccounts.map((choice) => <option key={choice.email} value={choice.email}>{choice.label}</option>)}
            </select>
          </label>
          <EmailBadgeField label="To" values={to} calendarAccount={event.accountId} disabled={busy} onChange={setTo} />
        </>}
      </> : <p className="modal-form-hint">No mail account is connected on this device, so the follow-up can be copied but not drafted.</p>}
      {conversation ? null : <label>Subject<input ref={subjectRef} value={subject} disabled={busy} maxLength={998} onChange={(change) => setSubject(change.target.value)} /></label>}
      <label>Message<textarea rows={8} value={body} disabled={busy} onChange={(change) => setBody(change.target.value)} /></label>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {copied ? <p className="modal-form-hint" role="status">Copied.</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose} disabled={busy}>Cancel</button>
        <button type="button" className="btn" onClick={() => void copy()} disabled={busy}><Copy size={ICON_SIZE.sm} aria-hidden="true" />Copy Text</button>
        {canDraft ? <button type="submit" className="btn btn-primary" disabled={busy || (!conversation && (!account || !to.length))}>{busy ? "Opening…" : "Open Draft"}</button> : null}
      </div>
    </form>
  </Modal>;
}
