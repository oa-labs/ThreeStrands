import { Copy, Plus, Sparkles, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { cancelAiRequest, usePendingAiRequests } from "./aiRequests";
import { readAiRequestConfig } from "./aiSettings";
import { formatEventTime } from "./calendarTime";
import { Modal } from "./AppChrome";
import { EmailBadgeField } from "./ContactAddressField";
import { mailClient } from "./data/client";
import type {
  AcceptedFollowThrough, DraftHandoff, DraftHandoffRequest, FollowThroughItem, FollowThroughSave,
  MeetingContext, MeetingNoteSource, MeetingRecap, MeetingRecording, RecordingReviewContext,
  RecordingSettings, ScheduleEvent,
} from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import {
  adhocFollowThroughSectionText, discardFollowThroughRecovery, followThroughSectionText, followUpBody, followUpRecipients, MAX_FOLLOW_THROUGH_ITEMS,
  readFollowThroughRecovery, readFollowUpHandoff, rememberFollowUpHandoff, saveFollowThroughRecovery, type FollowThroughRecovery, type ReviewItem,
} from "./meetingFollowThrough";
import { meetingEventReference } from "./meetingNoteEntry";
import { saveAdhocFollowThrough, saveFollowThrough } from "./noteCapture";
import { notePlainText } from "./notePreview";
import { announceRecordingSettingsChanged } from "./recordingEvents";
import { formatElapsed } from "./recordingPolicy";
import type { MailAccountChoice } from "./MeetingPreparation";

type Kind = FollowThroughItem["kind"];

const KIND_LABEL: Record<Exclude<Kind, "link">, string> = {
  decision: "Decision",
  task: "Task",
  checklist: "Checklist",
  action: "Not Tracked",
  question: "Open Question",
};
const dayLabel = (date: string) => new Date(`${date}T00:00:00`).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
const destinationLabel = (source: Pick<MeetingNoteSource, "dailyDate" | "title">) => source.dailyDate ? `Daily note, ${dayLabel(source.dailyDate)}` : source.title;
const newId = () => crypto.randomUUID();
const submitOnModEnter = (event: KeyboardEvent<HTMLFormElement>) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); event.currentTarget.requestSubmit(); }
};

function itemsFromRecap(recap: MeetingRecap): ReviewItem[] {
  const out: ReviewItem[] = [];
  for (const decision of recap.decisions) {
    out.push({ id: newId(), kind: "decision", text: decision.text, suggested: true, evidence: decision.evidence });
  }
  for (const action of recap.actions) {
    const kind = action.owner === "you" ? "task" : "action";
    out.push({ id: newId(), kind, text: action.text, suggested: true, evidence: action.evidence });
  }
  for (const question of recap.openQuestions) {
    out.push({ id: newId(), kind: "question", text: question.text, suggested: true, evidence: question.evidence });
  }
  return out;
}

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
 * Review Follow-through: the user records the meeting's summary, decisions,
 * actions, and open questions, typed, picked from its notes, or optionally
 * summarized from a recording transcript, and saves them as one dated section.
 * Edits are kept on this device until saved or discarded, and restored after a
 * restart without repeating a save.
 */
export function MeetingFollowThroughDialog({
  event,
  context,
  today,
  onClose,
  onSaved,
  onAddNote,
  recording = null,
  todaysEvents = [],
  onSelectEvent,
}: {
  event: ScheduleEvent | null;
  context: MeetingContext | null;
  /** The review's date, written on the section. */
  today: string;
  onClose(): void;
  onSaved(saved: FollowThroughSave & { undo(): Promise<void> }): void;
  /** Runs Add Note when the meeting has no note to save into. */
  onAddNote?(): void;
  recording?: MeetingRecording | null;
  todaysEvents?: readonly ScheduleEvent[];
  onSelectEvent?(event: ScheduleEvent | null): void;
}) {
  const reference = event ? meetingEventReference(event) : null;
  const destinations = useMemo(() => followThroughDestinations(context), [context]);
  const [restored] = useState(() => readFollowThroughRecovery(reference, Date.now(), recording?.id ?? null));
  const [record, setRecord] = useState<FollowThroughRecovery>(() => restored ?? {
    reference,
    reviewId: newId(),
    operationId: newId(),
    items: [],
    summary: "",
    attachTranscript: false,
    recordingId: recording?.id ?? null,
    adhocTitle: recording?.title ?? "",
    updatedAt: Date.now(),
    destinationNoteId: destinations.find((source) => source.kind === "excerpt")?.noteId ?? destinations[0]?.noteId ?? null,
  });
  const [sourceIndex, setSourceIndex] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reviewCtx, setReviewCtx] = useState<RecordingReviewContext | null>(null);
  const [recap, setRecap] = useState<MeetingRecap | null>(null);
  const [includeMyNotes, setIncludeMyNotes] = useState(false);
  const [summarizing, setSummarizing] = useState(false);
  const [aiError, setAiError] = useState<string | null>(null);
  const [rangeStartMin, setRangeStartMin] = useState(0);
  const [rangeEndMin, setRangeEndMin] = useState(() => Math.min(30, Math.max(1, Math.ceil((recording?.durationMs ?? 1_800_000) / 60_000))));
  const [savedSettings, setSavedSettings] = useState<RecordingSettings | null>(null);
  const [attachTranscript, setAttachTranscript] = useState(() => restored?.attachTranscript ?? false);
  const dirtyRef = useRef(Boolean(restored));
  const firstRef = useRef<HTMLButtonElement>(null);
  const lastText = useRef<HTMLInputElement>(null);
  const [focusNew, setFocusNew] = useState(false);
  const pendingAi = usePendingAiRequests();
  useEffect(() => { if (focusNew) { lastText.current?.focus(); setFocusNew(false); } }, [focusNew]);

  const recordingId = recording?.id ?? null;
  const restoredAttach = restored?.attachTranscript;
  useEffect(() => {
    if (!recordingId) return;
    let active = true;
    mailClient.recordingReviewContext(recordingId).then((found) => {
      if (!active) return;
      setReviewCtx(found);
      if (found.recap) {
        setRecap(found.recap);
        if (!dirtyRef.current) {
          setRecord((current) => {
            if (current.items.length || current.summary?.trim()) return current;
            const next = { ...current, summary: found.recap!.summary, items: itemsFromRecap(found.recap!) };
            return next;
          });
        }
      }
    }, (reason) => {
      if (active) setAiError(errorMessage(reason));
    });
    mailClient.recordingSettings().then((settings) => {
      if (!active) return;
      setSavedSettings(settings);
      if (restoredAttach === undefined) {
        setAttachTranscript(settings.attachTranscript);
      }
    }, () => {});
    return () => { active = false; };
  }, [recordingId, restoredAttach]);

  const edit = (change: (current: FollowThroughRecovery) => FollowThroughRecovery) => {
    dirtyRef.current = true;
    setRecord((current) => {
      const next = { ...change(current), reference, recordingId: recording?.id ?? current.recordingId ?? null };
      saveFollowThroughRecovery(next);
      return next;
    });
  };
  const setItems = (change: (items: ReviewItem[]) => ReviewItem[]) => edit((current) => ({ ...current, items: change(current.items) }));
  const add = (item: FollowThroughItem) => { setItems((items) => [...items, { ...item, id: newId() } as ReviewItem]); if (item.kind !== "link") setFocusNew(true); };
  const discard = () => {
    discardFollowThroughRecovery(record.reviewId);
    setRecord({
      ...record,
      reviewId: newId(),
      operationId: newId(),
      summary: "",
      items: [],
    });
  };
  const close = () => {
    // A review with nothing in it isn't worth restoring.
    if (!record.items.length && !record.summary?.trim()) discardFollowThroughRecovery(record.reviewId);
    onClose();
  };

  const toggleAttachTranscript = (checked: boolean) => {
    setAttachTranscript(checked);
    edit((current) => ({ ...current, attachTranscript: checked }));
    if (savedSettings) {
      const next = { ...savedSettings, attachTranscript: checked };
      setSavedSettings(next);
      void mailClient.setRecordingSettings(next).then(() => announceRecordingSettingsChanged()).catch(() => {});
    }
  };

  const applyRecap = (nextRecap: MeetingRecap) => {
    setRecap(nextRecap);
    edit((current) => ({
      ...current,
      summary: nextRecap.summary,
      items: [...current.items.filter((item) => !item.suggested), ...itemsFromRecap(nextRecap)],
    }));
  };

  const runSummarize = async () => {
    if (!recording || summarizing) return;
    setSummarizing(true);
    setAiError(null);
    try {
      const cfg = readAiRequestConfig("summarizing this recording", "meetingRecap");
      const range = reviewCtx?.overBudget ? { startMs: rangeStartMin * 60_000, endMs: rangeEndMin * 60_000 } : null;
      const result = await mailClient.summarizeRecording(
        recording.id,
        cfg.provider,
        cfg.model,
        cfg.endpoint,
        cfg.reasoning,
        Boolean(reviewCtx?.hasUserNotes && includeMyNotes),
        range,
      );
      applyRecap(result);
    } catch (reason) {
      setAiError(errorMessage(reason));
    } finally {
      setSummarizing(false);
    }
  };

  const activeAiRequest = recording ? pendingAi.find((req) => req.command === "summarize_recording" && req.owner === recording.id) : undefined;
  const cancelSummarize = () => {
    if (activeAiRequest) void cancelAiRequest(activeAiRequest.requestId);
  };

  const source = context?.thisMeeting.items[sourceIndex];
  const defaultDestinationId = destinations.find((candidate) => candidate.kind === "excerpt")?.noteId ?? destinations[0]?.noteId ?? null;
  const effectiveDestinationId = (record.destinationNoteId && destinations.some((d) => d.noteId === record.destinationNoteId))
    ? record.destinationNoteId
    : defaultDestinationId;
  const destination = destinations.find((candidate) => candidate.noteId === effectiveDestinationId) ?? null;
  const items: FollowThroughItem[] = record.items.map(({ id: _id, suggested: _suggested, evidence: _evidence, ...item }) => item as FollowThroughItem);
  const filled = items.filter((item) => item.kind === "link" || item.text.trim());
  const summaryText = record.summary ?? "";
  const cleanSummary = summaryText.trim();
  const meetingTitle = event?.title ?? ((record.adhocTitle ?? "").trim() || recording?.title || "Meeting");
  const timeLabel = recording ? new Date(recording.startedAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : "Meeting";
  const linkedIds = new Set(record.items.flatMap((item) => item.kind === "link" ? [item.task.note ? `note:${item.task.note}:${item.task.id}` : item.task.id] : []));
  const commitments = (context?.commitments.items ?? []).filter((item) => !item.candidate && !linkedIds.has(item.id));
  const hasContent = filled.length > 0 || Boolean(cleanSummary) || Boolean(recording && attachTranscript);
  const transcriptPreview = recording && attachTranscript ? { label: `Transcript – ${meetingTitle} – ${today}.md`, href: "attachment:pending" } : null;
  const preview = (() => {
    if (!hasContent) return "";
    try {
      if (reference) {
        return followThroughSectionText(reference, meetingTitle, today, filled, { summary: cleanSummary || null, transcript: transcriptPreview });
      }
      return adhocFollowThroughSectionText(timeLabel, meetingTitle, filled, { summary: cleanSummary || null, transcript: transcriptPreview });
    } catch {
      return "";
    }
  })();
  const placement = reference
    ? (destination
      ? (destination.kind === "excerpt"
        ? `Adds after this meeting's line in ${destinationLabel(destination)}.`
        : `Adds at the end of “${destination.title}”, linking this meeting.`)
      : "")
    : `Adds under ## Meetings in Daily note, ${dayLabel(today)}.`;

  const canSave = hasContent && (reference ? Boolean(destination) : Boolean(recording));

  const submit = async (submitted: FormEvent) => {
    submitted.preventDefault();
    if (saving || !canSave) return;
    setSaving(true); setError(null);
    const accepted: AcceptedFollowThrough = {
      date: today,
      meeting: meetingTitle,
      ...(cleanSummary ? { summary: cleanSummary } : {}),
      items: filled,
      ...(recording ? { recordingId: recording.id, attachTranscript } : {}),
    };
    try {
      let saved: FollowThroughSave & { undo(): Promise<void> };
      if (reference && destination) {
        saved = await saveFollowThrough(reference, { noteId: destination.noteId, dailyDate: destination.dailyDate, revision: destination.revision }, record.operationId, accepted);
      } else {
        const daily = await mailClient.getDailyNote(today);
        saved = await saveAdhocFollowThrough(today, timeLabel, daily?.updatedAt ?? null, record.operationId, accepted);
      }
      discardFollowThroughRecovery(record.reviewId);
      onSaved(saved);
    } catch (reason) {
      setError(errorMessage(reason));
      setSaving(false);
    }
  };

  return <Modal title="Review Follow-through" className="follow-through-modal" initialFocusRef={firstRef} onClose={close}>
    <form className="modal-form" onSubmit={(submitted) => void submit(submitted)} onKeyDown={submitOnModEnter}>
      <p className="modal-form-context">{meetingTitle} · {dayLabel(today)}</p>
      <p className="modal-form-hint">Write down what was decided and what happens next. Each action can be a task in Tasks, a checklist item that stays in the note, or a note that isn't tracked.</p>
      {restored ? <p className="modal-form-hint" role="status">Restored the review you hadn't saved. <button type="button" className="btn-link" onClick={discard}>Discard</button></p> : null}
      {context?.followThrough.items.length ? <p className="modal-form-hint">Already reviewed {context.followThrough.items.map((item) => item.date ? dayLabel(item.date) : destinationLabel(item)).join(", ")}. Saving adds another section.</p> : null}

      {recording && !recording.eventRef ? <>
        <label>Meeting
          <select value={event?.id ?? ""} disabled={saving || summarizing} onChange={(change) => {
            const chosen = todaysEvents.find((candidate) => candidate.id === change.target.value) ?? null;
            onSelectEvent?.(chosen);
          }}>
            <option value="">None (save to daily note)</option>
            {todaysEvents.map((candidate) => <option key={candidate.id} value={candidate.id}>{formatEventTime(candidate)} · {candidate.title}</option>)}
          </select>
        </label>
        {!event ? <label>Title
          <input value={record.adhocTitle ?? ""} maxLength={200} disabled={saving} onChange={(change) => edit((current) => ({ ...current, adhocTitle: change.target.value }))} />
        </label> : null}
      </> : null}

      {recording ? <div className="follow-through-recap">
        <div className="follow-through-recap-controls">
          {!summarizing ? (
            <button type="button" className="btn btn-sm" disabled={saving || Boolean(reviewCtx?.overBudget && rangeEndMin <= rangeStartMin)} onClick={() => void runSummarize()}>
              <Sparkles size={ICON_SIZE.sm} aria-hidden="true" />
              {recap || recording.hasRecap ? "Re-summarize" : "Summarize"}
            </button>
          ) : (
            <>
              <span className="modal-form-hint" role="status">Summarizing…</span>
              {activeAiRequest ? <button type="button" className="btn btn-sm" onClick={cancelSummarize}>Cancel</button> : null}
            </>
          )}
          {reviewCtx?.hasUserNotes ? <label className="modal-form-check">
            <input type="checkbox" checked={includeMyNotes} disabled={saving || summarizing} onChange={(change) => setIncludeMyNotes(change.target.checked)} />
            Include My Notes
          </label> : null}
        </div>
        {reviewCtx?.notesExcludedFromAi ? <p className="modal-form-hint">
          {reviewCtx.hasUserNotes ? "One or more notes for this meeting are excluded from AI." : "This meeting's note is excluded from AI."}
        </p> : null}
        {reviewCtx?.overBudget ? <div className="follow-through-recap-range">
          <p className="modal-form-hint" role="note">This transcript is longer than one summary can hold. Choose a minute range to summarize.</p>
          <label>Start (min)<input type="number" min={0} max={Math.max(0, rangeEndMin - 1)} value={rangeStartMin} disabled={saving || summarizing} onChange={(e) => setRangeStartMin(Math.max(0, Number(e.target.value) || 0))} /></label>
          <label>End (min)<input type="number" min={rangeStartMin + 1} value={rangeEndMin} disabled={saving || summarizing} onChange={(e) => setRangeEndMin(Math.max(1, Number(e.target.value) || 1))} /></label>
        </div> : null}
        {recap?.withheldCount ? <p className="modal-form-hint" role="status">
          {recap.withheldCount} {recap.withheldCount === 1 ? "suggestion was" : "suggestions were"} withheld because {recap.withheldCount === 1 ? "it wasn't" : "they weren't"} grounded in the transcript.
        </p> : null}
        {aiError ? <p className="form-error" role="alert">{aiError}</p> : null}
      </div> : null}

      <label>Summary
        <textarea
          className="follow-through-summary"
          rows={3}
          value={summaryText}
          maxLength={4000}
          disabled={saving}
          onChange={(change) => edit((current) => ({ ...current, summary: change.target.value }))}
        />
      </label>

      {context?.thisMeeting.items.length ? <div className="modal-form-evidence">
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
        {record.items.map((item, index) => <div key={item.id} className="follow-through-item-wrap">
          {item.suggested ? <span className="follow-through-suggested-badge">Suggested</span> : null}
          <div className="follow-through-item">
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
          </div>
          {item.evidence?.length ? <div className="follow-through-evidence">
            {item.evidence.map((ev, evIndex) => (
              <span key={`${ev.segmentId}:${evIndex}`}>[{formatElapsed(ev.startMs)}] {ev.speakerLabel}: “{ev.text}”</span>
            ))}
          </div> : null}
        </div>)}
        <div className="follow-through-add">
          <button ref={firstRef} type="button" className="btn btn-sm" disabled={record.items.length >= MAX_FOLLOW_THROUGH_ITEMS} onClick={() => add({ kind: "decision", text: "" })}><Plus size={ICON_SIZE.sm} aria-hidden="true" />Add Decision</button>
          <button type="button" className="btn btn-sm" disabled={record.items.length >= MAX_FOLLOW_THROUGH_ITEMS} onClick={() => add({ kind: "task", text: "" })}><Plus size={ICON_SIZE.sm} aria-hidden="true" />Add Action</button>
          <button type="button" className="btn btn-sm" disabled={record.items.length >= MAX_FOLLOW_THROUGH_ITEMS} onClick={() => add({ kind: "question", text: "" })}><Plus size={ICON_SIZE.sm} aria-hidden="true" />Add Question</button>
        </div>
      </fieldset>

      {recording ? <label className="modal-form-check">
        <input type="checkbox" checked={attachTranscript} disabled={saving} onChange={(change) => toggleAttachTranscript(change.target.checked)} />
        Attach Transcript
      </label> : null}

      {reference ? (
        destinations.length ? <label>Save To
          <select value={effectiveDestinationId ?? ""} disabled={saving} onChange={(change) => edit((current) => ({ ...current, destinationNoteId: change.target.value }))}>
            {destinations.map((item) => <option key={item.noteId} value={item.noteId}>{destinationLabel(item)}</option>)}
          </select>
        </label> : <p className="modal-form-hint">This meeting has no note yet. {onAddNote ? <><button type="button" className="btn-link" onClick={() => { onClose(); onAddNote(); }}>Add Note</button> first; your review is kept.</> : "Add a note for it first; your review is kept."}</p>
      ) : null}

      {(destination || !reference) && preview ? <div className="modal-form-evidence">
        <span>{placement}</span>
        <pre className="follow-through-preview" aria-label="Insertion preview">{preview}</pre>
      </div> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={close} disabled={saving}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={saving || !canSave}>{saving ? "Saving…" : "Save Follow-through"}</button>
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
